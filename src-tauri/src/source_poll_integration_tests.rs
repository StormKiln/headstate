//! Stitched synthetic provider → publication → durable rows contract, not live IPC.
use super::*;
use crate::{
    github::{
        admission::{ReadClass, ReadContext},
        client::GitHubClient,
    },
    queue_scan,
    store::source_cache::{self, SnapshotData},
};
use serde_json::{json, Value};
use std::time::Duration;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

fn node(number: usize, changed: bool) -> Value {
    let seed: Value = serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
    let mut row = seed["authored"]["nodes"][0].clone();
    let repo = format!("octocat/repo-{}", (number - 1) % 44 + 1);
    row["id"] = json!(format!("PR_{number}"));
    row["number"] = json!(number);
    row["title"] = json!(format!("Synthetic review {number}"));
    row["repository"]["nameWithOwner"] = json!(repo);
    row["url"] = json!(format!("https://github.com/{repo}/pull/{number}"));
    row["author"]["login"] = json!("synthetic-author");
    row["headRefOid"] = if number == 276 {
        Value::Null
    } else {
        json!(format!(
            "head-{number}{}",
            if changed && number == 1 { "-new" } else { "" }
        ))
    };
    row["reviewDecision"] = Value::Null;
    row["latestReviews"] = json!({"nodes":[],"totalCount":0});
    row
}
fn canonical(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "session" {
                    *value = json!("synthetic-publication");
                } else if [
                    "last_received_at",
                    "last_observed_at",
                    "confirmed_at",
                    "submitted_at",
                ]
                .contains(&key.as_str())
                    && !value.is_null()
                {
                    *value = json!("2026-10-01T12:00:00Z");
                } else {
                    canonical(value);
                }
            }
        }
        Value::Array(rows) => {
            for row in rows {
                canonical(row);
            }
        }
        _ => {}
    }
}
fn saved(conn: &rusqlite::Connection) -> Vec<crate::github::model::PullRequest> {
    let SnapshotData::Available { prs, .. } =
        source_cache::load_source_snapshot(conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .data
    else {
        panic!("missing owned snapshot")
    };
    prs
}
async fn publish(
    polls: &SourcePolls,
    conn: &rusqlite::Connection,
    result: Result<FetchedList, Failure>,
) -> Value {
    let source = Source::default();
    let list = CachedList::Reviewing;
    let (attempt, _) = polls.begin_attempt(source.clone(), list).await;
    let permit = polls.publication(&attempt).await.unwrap();
    let prepared = result.and_then(|result| {
        if result.scan.is_none() {
            // The confirmed-write path persists the existing owned rows under
            // this publication permit; it is not a fresh provider observation.
            persist_github_effect(conn, &source, list, &result).unwrap();
            Ok(result)
        } else {
            reconcile_github_snapshot(conn, &source, list, result, None)
        }
    });
    let mut frame = None;
    polls.complete(permit, prepared, |status| {
        frame = Some(polls.update(status, None));
    });
    let update = frame.unwrap();
    // Raw equality comes before canonicalizing any nondeterministic metadata.
    assert_eq!(saved(conn), update.prs.clone().unwrap());
    if let SnapshotData::Available { coverage, .. } =
        source_cache::load_source_snapshot(conn, &source, list)
            .unwrap()
            .data
    {
        assert_eq!(Some(coverage), update.status.coverage);
    }
    let mut value = serde_json::to_value(update).unwrap();
    canonical(&mut value);
    value
}
fn github_client(server: &MockServer) -> GitHubClient {
    GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("synthetic-token")
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn integrated_inventory_publications_retain_large_owned_progress_through_outage() {
    let server = MockServer::start().await;
    let mode = Arc::new(Mutex::new("small"));
    let served = mode.clone();
    let respond = move |request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let mode = *served.lock().unwrap();
        if mode == "throttle" {
            return ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({"message":"Synthetic cooldown"}));
        }
        if body["query"].as_str().unwrap().contains("mutation(") {
            return ResponseTemplate::new(200).set_body_json(json!({"data":{"addPullRequestReview":{"pullRequestReview":{
                "id":"REVIEW_1","state":"APPROVED","submittedAt":"2026-10-01T12:00:00Z","author":{"login":"synthetic-viewer"},"commit":{"oid":"head-1"},
                "pullRequest":{"id":"PR_1","number":1,"repository":{"nameWithOwner":"octocat/repo-1"}}
            }}}}));
        }
        let start = body["variables"]["after"]
            .as_str()
            .and_then(|s| s.strip_prefix("cursor-"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let total = if mode == "small" {
            20
        } else if mode == "partial" {
            276
        } else {
            275
        };
        if mode == "partial" && start > 0 {
            return ResponseTemplate::new(503);
        }
        let end = (start + 25).min(total);
        let mut nodes = if mode == "partial" {
            vec![node(276, false)]
        } else {
            (start + 1..=end)
                .map(|n| node(n, mode == "changed"))
                .collect()
        };
        if mode == "confirmed" {
            for row in &mut nodes {
                if row["number"] == 1 {
                    row["latestReviews"] = json!({"totalCount":1,"nodes":[{"id":"REVIEW_1","state":"APPROVED","submittedAt":"2026-10-01T12:00:00Z","author":{"login":"synthetic-viewer"},"commit":{"oid":"head-1"}}]});
                }
            }
        }
        ResponseTemplate::new(200).insert_header("x-ratelimit-remaining","5000").set_body_json(json!({"data":{
            "viewer":{"login":if mode=="other-owner" {"other-viewer"} else {"synthetic-viewer"}},
            "authored":{"issueCount":total,"nodes":nodes,"pageInfo":{"hasNextPage":end<total,"endCursor":format!("cursor-{end}")}}
        }}))
    };
    Mock::given(method("POST"))
        .respond_with(respond)
        .mount(&server)
        .await;
    let client = github_client(&server);
    client.fetch_viewer().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inventory.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut frames = serde_json::Map::new();
    let mut counts = vec![];
    let mut now = 1000;
    // Real coherent Complete is the small baseline, never fabricated for 275 rows.
    let first = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "synthetic-viewer",
            )
            .unwrap(),
            &[],
            now,
        )
        .await
        .unwrap();
    assert_eq!(first.coverage, Coverage::Complete);
    frames.insert("complete".into(), publish(&polls, &conn, Ok(first)).await);
    *mode.lock().unwrap() = "large";
    for _ in 0..6 {
        now += 60;
        let before = server.received_requests().await.unwrap().len();
        let rows = saved(&conn);
        let loaded = queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap();
        let result = client
            .with_read_context(ReadContext::new(
                ReadClass::Background,
                Duration::from_secs(10),
            ))
            .advance_scan(CachedList::Reviewing, loaded, &rows, now)
            .await
            .unwrap();
        let attempts = server.received_requests().await.unwrap().len() - before;
        assert!(attempts <= 3);
        counts.push(attempts);
        let frame = publish(&polls, &conn, Ok(result)).await;
        frames.insert("large".into(), frame);
    }
    let rows = saved(&conn);
    assert_eq!(rows.len(), 275);
    assert_eq!(
        rows.iter()
            .map(|r| r.repo.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        44
    );
    assert_eq!(
        rows.iter()
            .map(|r| r.number)
            .collect::<std::collections::BTreeSet<_>>(),
        (1..=275).collect()
    );
    assert!(matches!(
        source_cache::load_source_snapshot(&conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .data,
        SnapshotData::Available {
            coverage: Coverage::Partial { total: Some(275) },
            ..
        }
    ));
    let observed_before_partial = rows
        .iter()
        .find(|r| r.number == 275)
        .unwrap()
        .observation
        .as_ref()
        .unwrap()
        .last_observed_at;
    let before_partial = server.received_requests().await.unwrap().len();
    *mode.lock().unwrap() = "partial";
    now += 60;
    let result = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "synthetic-viewer",
            )
            .unwrap(),
            &rows,
            now,
        )
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().len() - before_partial <= 3);
    frames.insert("partial".into(), publish(&polls, &conn, Ok(result)).await);
    let rows = saved(&conn);
    assert_eq!(rows.len(), 276);
    assert_eq!(
        rows.iter()
            .find(|r| r.number == 275)
            .unwrap()
            .observation
            .as_ref()
            .unwrap()
            .last_observed_at,
        observed_before_partial
    );
    assert!(rows
        .iter()
        .find(|r| r.number == 276)
        .unwrap()
        .observation
        .as_ref()
        .unwrap()
        .unknown_fields
        .contains(&crate::inventory::ReadinessField::Head));
    assert!(
        rows.iter()
            .find(|r| r.number == 275)
            .unwrap()
            .observation
            .as_ref()
            .unwrap()
            .state
            == crate::inventory::ObservationState::Retained
    );
    *mode.lock().unwrap() = "throttle";
    let error = client
        .fetch_viewer_metered(&client.request_budget())
        .await
        .unwrap_err();
    let n = server.received_requests().await.unwrap().len();
    assert!(client
        .fetch_viewer_metered(&client.request_budget())
        .await
        .is_err());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        n,
        "cooldown dispatches zero HTTP"
    );
    frames.insert(
        "cooldown".into(),
        publish(&polls, &conn, Err(Failure::from(&error))).await,
    );
    *mode.lock().unwrap() = "large";
    tokio::time::sleep(Duration::from_millis(1100)).await;
    use crate::github::mutate::{BoundReviewOutcome, BoundReviewRequest};
    let before = server.received_requests().await.unwrap().len();
    let outcome = client
        .add_review_at_head(&BoundReviewRequest {
            id: "PR_1".into(),
            repo: "octocat/repo-1".into(),
            number: 1,
            verdict: "approve".into(),
            body: String::new(),
            expected_head: "head-1".into(),
            expected_viewer: "synthetic-viewer".into(),
        })
        .await;
    let BoundReviewOutcome::Acknowledged { receipt } = outcome else {
        panic!("semantic receipt")
    };
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        1,
        "one write, no preflight"
    );
    let mut result = polls
        .2
        .lock()
        .unwrap()
        .get(&(Source::default(), CachedList::Reviewing))
        .unwrap()
        .1
        .clone();
    result.scan = None;
    let effect = crate::inventory::ConfirmedReview {
        head_oid: "head-1".into(),
        review: crate::github::model::ReviewState::Approved,
        confirmed_at: chrono::Utc::now(),
        receipt: Some(receipt),
        unresolved: false,
        confirmed_by_read: false,
    };
    assert!(crate::inventory::apply_confirmed_review(
        result.prs.iter_mut().find(|r| r.number == 1).unwrap(),
        &effect
    ));
    frames.insert(
        "acknowledged".into(),
        publish(&polls, &conn, Ok(result)).await,
    );
    for (stage, mode_name) in [
        ("confirmed", "confirmed"),
        ("recovered", "large"),
        ("changed_head", "changed"),
    ] {
        *mode.lock().unwrap() = mode_name;
        now += 60;
        let result = client
            .advance_scan(
                CachedList::Reviewing,
                queue_scan::load(
                    &conn,
                    &Source::default(),
                    CachedList::Reviewing,
                    "synthetic-viewer",
                )
                .unwrap(),
                &saved(&conn),
                now,
            )
            .await
            .unwrap();
        frames.insert(stage.into(), publish(&polls, &conn, Ok(result)).await);
    }
    assert!(frames["recovered"]["prs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == 1)
        .unwrap()["observation"]["confirmed_review"]
        .is_object());
    assert!(frames["changed_head"]["prs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == 1)
        .unwrap()["observation"]
        .get("confirmed_review")
        .is_none());
    drop(conn);
    assert_eq!(
        frames["recovered"]["prs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == 1)
            .unwrap()["observation"]["confirmed_review"]["confirmed_by_read"],
        true
    );
    let conn = crate::store::open_db(&path).unwrap();
    assert_eq!(saved(&conn).len(), 276);
    // A verified new client/owner must not reuse this owner's durable union.
    *mode.lock().unwrap() = "other-owner";
    let other = github_client(&server);
    other.fetch_viewer().await.unwrap();
    let result = other
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "other-viewer",
            )
            .unwrap(),
            &[],
            now + 60,
        )
        .await
        .unwrap();
    let other_rows = reconcile_github_snapshot(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        result,
        None,
    )
    .unwrap_or_else(|error| panic!("{}", error.message));
    assert!(other_rows.prs.len() < 100);
    assert_eq!(saved(&conn), other_rows.prs);
    assert_eq!(
        source_cache::snapshot_owner(&conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .as_deref(),
        Some("other-viewer")
    );
    assert!(other_rows.prs.iter().all(|r| r
        .observation
        .as_ref()
        .unwrap()
        .confirmed_review
        .is_none()));
    let artifact = json!({"scenario":"complete-small-to-275-across-44-repositories","cycle_http_attempts":counts,"frames":frames});
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/review-reliability-publications.json");
    if std::env::var_os("UPDATE_REVIEW_RELIABILITY_FIXTURE").is_some() {
        std::fs::write(&fixture, serde_json::to_string(&artifact).unwrap() + "\n").unwrap();
    }
    let expected: Value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    assert_eq!(artifact, expected);
}
