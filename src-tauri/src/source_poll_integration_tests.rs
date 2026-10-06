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
            persist_github_effect(conn, &source, list, &result, true).unwrap();
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
    // Small atomic baseline; large completion later proves traversal coverage only.
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
            coverage: Coverage::Complete,
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
            == crate::inventory::ObservationState::Observed
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

async fn stable_queue(total: usize) -> (MockServer, GitHubClient) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let start = body["variables"]["after"].as_str()
                .and_then(|s| s.strip_prefix("cursor-"))
                .and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            let end = (start + 25).min(total);
            ResponseTemplate::new(200).set_body_json(json!({"data":{
                "viewer":{"login":"synthetic-viewer"},
                "authored":{"issueCount":total,"nodes":(start+1..=end).map(|n|node(n,false)).collect::<Vec<_>>(),
                "pageInfo":{"hasNextPage":end<total,"endCursor":format!("cursor-{end}")}}
            }}))
        }).mount(&server).await;
    let client = github_client(&server);
    client.fetch_viewer().await.unwrap();
    (server, client)
}

async fn scan_and_publish(
    polls: &SourcePolls,
    conn: &rusqlite::Connection,
    client: &GitHubClient,
    now: i64,
) {
    let loaded = queue_scan::load(
        conn,
        &Source::default(),
        CachedList::Reviewing,
        "synthetic-viewer",
    )
    .unwrap();
    let previous =
        match source_cache::load_source_snapshot(conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .data
        {
            SnapshotData::Available { prs, .. } => prs,
            _ => vec![],
        };
    let result = client
        .advance_scan(CachedList::Reviewing, loaded, &previous, now)
        .await
        .unwrap();
    publish(polls, conn, Ok(result)).await;
}

#[tokio::test]
async fn healthy_terminal_delta_establishes_coverage_without_losing_prior_pages() {
    let (_server, client) = stable_queue(75).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    let first = saved(&conn);
    scan_and_publish(&polls, &conn, &client, 1015).await;
    let rows = saved(&conn);
    assert_eq!(
        rows.len(),
        75,
        "terminal delta cannot replace earlier pages"
    );
    assert_eq!(
        polls
            .get(&Source::default(), CachedList::Reviewing)
            .coverage,
        Some(Coverage::Complete)
    );
    for old in first {
        let row = rows
            .iter()
            .find(|r| r.identity() == old.identity())
            .unwrap();
        assert_eq!(
            row.observation, old.observation,
            "unrelated tail page cannot restamp or downgrade evidence"
        );
    }
}

#[tokio::test]
async fn cooldown_no_work_preserves_receipt_and_observation_freshness() {
    let (server, client) = stable_queue(20).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    let rows = saved(&conn);
    let status = polls.get(&Source::default(), CachedList::Reviewing);
    let before = server.received_requests().await.unwrap().len();
    scan_and_publish(&polls, &conn, &client, 1001).await;
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(saved(&conn), rows);
    let after = polls.get(&Source::default(), CachedList::Reviewing);
    assert_eq!(after.phase, Phase::Ready);
    assert_eq!(after.last_received_at, status.last_received_at);
    assert_eq!(after.receipt_revision, status.receipt_revision);
}

#[tokio::test]
async fn first_step_does_not_repeat_the_head_page_it_just_observed() {
    let (server, client) = stable_queue(75).await;
    let before = server.received_requests().await.unwrap().len();
    let result = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded {
                revision: 0,
                state: queue_scan::State::default(),
            },
            &[],
            1000,
        )
        .await
        .unwrap();
    assert_eq!(result.prs.len(), 50);
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        2,
        "two unique pages, no redundant head refresh"
    );
}

#[tokio::test]
async fn failed_page_is_qualified_and_backoff_preserves_failure_receipt() {
    let (server, client) = stable_queue(75).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    conn.execute("UPDATE snapshot SET fetched_at='2020-01-01 00:00:00'", [])
        .unwrap();
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    scan_and_publish(&polls, &conn, &client, 1015).await;
    let failed = polls.get(&Source::default(), CachedList::Reviewing);
    let fetched_at: String = conn
        .query_row("SELECT fetched_at FROM snapshot", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        fetched_at, "2020-01-01 00:00:00",
        "a failed read cannot make the saved snapshot younger"
    );
    assert!(
        matches!(failed.phase, Phase::Retrying | Phase::Failed),
        "real page failure must not report successful Partial"
    );
    assert!(failed.error.is_some());
    let before = server.received_requests().await.unwrap().len();
    let rows = saved(&conn);
    scan_and_publish(&polls, &conn, &client, 1016).await;
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(saved(&conn), rows);
    let after = polls.get(&Source::default(), CachedList::Reviewing);
    assert_eq!(after.last_received_at, failed.last_received_at);
    assert_eq!(after.consecutive_failures, failed.consecutive_failures);
    assert_eq!(after.phase, failed.phase);
}

#[tokio::test]
async fn configured_new_pass_cadence_is_measured_from_completion() {
    let (server, client) = stable_queue(20).await;
    let result = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded {
                revision: 0,
                state: queue_scan::State {
                    pass_delay: 300,
                    ..queue_scan::State::default()
                },
            },
            &[],
            1000,
        )
        .await
        .unwrap();
    let state = result.scan.unwrap().state;
    let before = server.received_requests().await.unwrap().len();
    let early = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded { revision: 1, state },
            &result.prs,
            1299,
        )
        .await
        .unwrap();
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        before,
        "300-second setting must not restart at60"
    );
    let ready = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded {
                revision: 1,
                state: early.scan.unwrap().state,
            },
            &result.prs,
            1300,
        )
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before + 1);
    assert_eq!(ready.coverage, Coverage::Complete);
}

#[tokio::test]
async fn successive_large_traversals_keep_evidence_and_bound_requests() {
    let (server, client) = stable_queue(275).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    for pass in 0..3 {
        let before = server.received_requests().await.unwrap().len();
        for step in 0..6 {
            let rows_before = match source_cache::load_source_snapshot(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
            )
            .unwrap()
            .data
            {
                SnapshotData::Available { prs, .. } => prs,
                _ => vec![],
            };
            scan_and_publish(&polls, &conn, &client, 1000 + pass * 195 + step * 15).await;
            let rows = saved(&conn);
            assert!(rows.iter().all(|r| r.observation.as_ref().unwrap().state
                == crate::inventory::ObservationState::Observed));
            for old in rows_before {
                let next = rows
                    .iter()
                    .find(|r| r.identity() == old.identity())
                    .unwrap();
                assert!(
                    next.observation.as_ref().unwrap().last_observed_at
                        >= old.observation.as_ref().unwrap().last_observed_at
                );
            }
            if pass > 0 {
                assert_eq!(
                    polls
                        .get(&Source::default(), CachedList::Reviewing)
                        .coverage,
                    Some(Coverage::Complete)
                );
            }
        }
        assert_eq!(saved(&conn).len(), 275);
        assert_eq!(
            polls.get(&Source::default(), CachedList::Reviewing).phase,
            Phase::Ready
        );
        assert!(server.received_requests().await.unwrap().len() - before <= 16);
        let state = queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap()
        .state;
        assert!(state.done);
        assert_eq!(state.pages, 11);
    }
}

#[tokio::test]
async fn increasing_configured_interval_delays_an_already_finished_pass() {
    let (server, client) = stable_queue(20).await;
    let result = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded {
                revision: 0,
                state: queue_scan::State {
                    pass_delay: 120,
                    ..Default::default()
                },
            },
            &[],
            1000,
        )
        .await
        .unwrap();
    let mut state = result.scan.unwrap().state;
    state.pass_delay = 300;
    let before = server.received_requests().await.unwrap().len();
    let result = client
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::Loaded { revision: 1, state },
            &result.prs,
            1299,
        )
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert!(result.scan.unwrap().state.no_work);
}

#[tokio::test]
async fn changed_count_qualifies_coverage_and_missing_member_without_removing_it() {
    let (_server, client) = stable_queue(75).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    scan_and_publish(&polls, &conn, &client, 1015).await;
    let old = saved(&conn).into_iter().find(|r| r.number == 75).unwrap();
    let (_server, changed) = stable_queue(74).await;
    scan_and_publish(&polls, &conn, &changed, 1200).await;
    assert_eq!(
        polls
            .get(&Source::default(), CachedList::Reviewing)
            .coverage,
        Some(Coverage::Partial { total: Some(74) })
    );
    scan_and_publish(&polls, &conn, &changed, 1215).await;
    let rows = saved(&conn);
    assert_eq!(
        rows.len(),
        75,
        "non-atomic absence is not authority to remove a member"
    );
    let omitted = rows
        .iter()
        .find(|r| r.number == 75)
        .unwrap()
        .observation
        .as_ref()
        .unwrap();
    assert_eq!(omitted.state, crate::inventory::ObservationState::Retained);
    assert_eq!(
        omitted.last_observed_at,
        old.observation.unwrap().last_observed_at
    );
    let state = queue_scan::load(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        "synthetic-viewer",
    )
    .unwrap()
    .state;
    assert_eq!(state.candidates.len(), 1);
    assert_eq!(state.candidates[0].identity.number, 75);
}

#[tokio::test]
async fn provider_cooldown_refusal_does_not_downgrade_a_saved_receipt() {
    let (server, client) = stable_queue(20).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    let rows = saved(&conn);
    let status = polls.get(&Source::default(), CachedList::Reviewing);
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(json!({"message":"Synthetic provider cooldown"})),
        )
        .mount(&server)
        .await;
    assert!(client
        .fetch_viewer_metered(&client.request_budget())
        .await
        .is_err());
    let before = server.received_requests().await.unwrap().len();
    scan_and_publish(&polls, &conn, &client, 1200).await;
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(saved(&conn), rows);
    let after = polls.get(&Source::default(), CachedList::Reviewing);
    assert_eq!(after.phase, status.phase);
    assert_eq!(after.coverage, status.coverage);
    assert_eq!(after.last_received_at, status.last_received_at);
    assert_eq!(after.receipt_revision, status.receipt_revision);
}

#[tokio::test]
async fn head_refresh_failure_is_qualified_without_losing_successful_tail_pages() {
    let (server, client) = stable_queue(275).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    server.reset().await;
    Mock::given(method("POST")).respond_with(|request:&wiremock::Request| {
        let body:Value=request.body_json().unwrap();
        let Some(start)=body["variables"]["after"].as_str().and_then(|s|s.strip_prefix("cursor-")).and_then(|s|s.parse::<usize>().ok()) else {
            return ResponseTemplate::new(503);
        };
        let end=start+25;
        ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"synthetic-viewer"},"authored":{"issueCount":275,"nodes":(start+1..=end).map(|n|node(n,false)).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":true,"endCursor":format!("cursor-{end}")}}}}))
    }).mount(&server).await;
    scan_and_publish(&polls, &conn, &client, 1015).await;
    assert_eq!(saved(&conn).len(), 100);
    assert!(matches!(
        polls.get(&Source::default(), CachedList::Reviewing).phase,
        Phase::Retrying | Phase::Failed
    ));
    assert!(server.received_requests().await.unwrap().len() <= 3);
}

async fn publish_scan_attempt(
    polls: &SourcePolls,
    conn: &rusqlite::Connection,
    attempt: &Attempt,
    result: FetchedList,
) -> Option<Update> {
    let permit = polls.success_publication(attempt).await.unwrap();
    let result = reconcile_github_snapshot(
        conn,
        &Source::default(),
        CachedList::Reviewing,
        result,
        None,
    )
    .unwrap_or_else(|f| panic!("{}", f.message));
    let mut frame = None;
    polls.complete(permit, Ok(result), |status| {
        frame = Some(polls.update(status, None))
    });
    frame
}

async fn coalesced_outcome_is_adopted_once(recovery: bool) {
    let (server, client) = stable_queue(75).await;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let polls = SourcePolls::default();
    let source = Source::default();
    let list = CachedList::Reviewing;
    scan_and_publish(&polls, &conn, &client, 1000).await;
    if recovery {
        publish(
            &polls,
            &conn,
            Err(Failure {
                message: "Synthetic earlier failure".into(),
                transient: false,
                not_asked: false,
            }),
        )
        .await;
    } else {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
    }
    let baseline = polls.get(&source, list);
    conn.execute_batch("CREATE TABLE snapshot_writes (id INTEGER); CREATE TRIGGER audit_snapshot AFTER UPDATE ON snapshot BEGIN INSERT INTO snapshot_writes VALUES(1); END;").unwrap();
    let (a, _) = polls.begin_attempt(source.clone(), list).await;
    let (b, _) = polls.begin_attempt(source.clone(), list).await;
    let loaded = queue_scan::load(&conn, &source, list, "synthetic-viewer").unwrap();
    let rows = saved(&conn);
    let before = server.received_requests().await.unwrap().len();
    let (first, second) = tokio::join!(
        client.advance_scan(
            list,
            queue_scan::Loaded {
                revision: loaded.revision,
                state: loaded.state.clone()
            },
            &rows,
            1015
        ),
        client.advance_scan(list, loaded, &rows, 1015),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        first.scan, second.scan,
        "actual overlapping reads share an operation"
    );
    assert!(server.received_requests().await.unwrap().len() - before <= 3);
    publish_scan_attempt(&polls, &conn, &a, first).await;
    let first_status = polls.get(&source, list);
    let first_rows = saved(&conn);
    let revision = queue_scan::load(&conn, &source, list, "synthetic-viewer")
        .unwrap()
        .revision;
    let writes: i64 = conn
        .query_row("SELECT count(*) FROM snapshot_writes", [], |r| r.get(0))
        .unwrap();
    let adopted = publish_scan_attempt(&polls, &conn, &b, second.clone())
        .await
        .unwrap();
    assert_eq!(
        adopted.status.phase,
        if recovery {
            Phase::Ready
        } else {
            Phase::Retrying
        }
    );
    assert_eq!(
        adopted.status.consecutive_failures,
        if recovery { 0 } else { 1 }
    );
    assert_eq!(adopted.status.error.is_some(), !recovery);
    assert!(
        adopted.status.receipt_revision > baseline.receipt_revision,
        "qualified rows must reach modern receipt-gated consumers"
    );
    assert_eq!(adopted.prs.as_ref().unwrap(), &first_rows);
    assert_eq!(
        adopted.status.last_received_at, first_status.last_received_at,
        "adoption cannot restamp a shared operation"
    );
    if !recovery {
        assert_eq!(adopted.status.last_received_at, baseline.last_received_at);
    }
    assert_eq!(
        queue_scan::load(&conn, &source, list, "synthetic-viewer")
            .unwrap()
            .revision,
        revision
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM snapshot_writes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        writes
    );
    let (duplicate, _) = polls.begin_attempt(source.clone(), list).await;
    let repeated = publish_scan_attempt(&polls, &conn, &duplicate, second)
        .await
        .unwrap();
    assert_eq!(
        repeated.status.consecutive_failures,
        adopted.status.consecutive_failures
    );
    assert_eq!(repeated.status.phase, adopted.status.phase);
    assert_eq!(
        repeated.status.receipt_revision,
        adopted.status.receipt_revision
    );
    assert_eq!(
        repeated.status.last_received_at,
        adopted.status.last_received_at
    );
    assert_eq!(saved(&conn), first_rows);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM snapshot_writes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        writes
    );
}

#[tokio::test]
async fn older_first_coalesced_failure_publishes_qualification_and_one_failure() {
    coalesced_outcome_is_adopted_once(false).await;
}

#[tokio::test]
async fn older_first_coalesced_recovery_clears_failure_without_restamping() {
    coalesced_outcome_is_adopted_once(true).await;
}

#[tokio::test]
async fn file_snapshot_age_survives_reopen_failure_no_work_and_recovers_on_observation() {
    let (server, client) = stable_queue(75).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("age.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    // Age the fixture, then exercise only production snapshot reads/refreshes.
    conn.execute("UPDATE snapshot SET fetched_at='2020-01-01 00:00:00'", [])
        .unwrap();
    drop(conn);
    let read_age = |conn: &rusqlite::Connection| {
        let SnapshotData::Available {
            fetched_at,
            stale_secs,
            prs,
            ..
        } = source_cache::load_source_snapshot(conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .data
        else {
            panic!("missing snapshot")
        };
        (fetched_at, stale_secs, prs)
    };
    let conn = crate::store::open_db(&path).unwrap();
    let old = read_age(&conn);
    assert!(old.1.is_some_and(|age| age > 86_400));
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    scan_and_publish(&polls, &conn, &client, 1015).await;
    drop(conn);
    let conn = crate::store::open_db(&path).unwrap();
    let failed = read_age(&conn);
    assert_eq!(failed.0, old.0);
    assert!(failed.1 >= old.1);
    assert_eq!(failed.2.len(), 50);
    let before = server.received_requests().await.unwrap().len();
    scan_and_publish(&polls, &conn, &client, 1016).await;
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    drop(conn);
    let conn = crate::store::open_db(&path).unwrap();
    let deferred = read_age(&conn);
    assert_eq!(deferred.0, old.0);
    assert!(deferred.1 >= failed.1);
    assert_eq!(deferred.2, failed.2);
    let (_recovery_server, recovery) = stable_queue(75).await;
    scan_and_publish(&polls, &conn, &recovery, 1100).await;
    drop(conn);
    let conn = crate::store::open_db(&path).unwrap();
    let recovered = read_age(&conn);
    assert_ne!(recovered.0, old.0);
    assert_eq!(recovered.1, None);
    assert_eq!(recovered.2.len(), 75);
    for old in failed.2 {
        assert_eq!(
            recovered
                .2
                .iter()
                .find(|r| r.identity() == old.identity())
                .unwrap()
                .observation,
            old.observation
        );
    }
}

#[tokio::test]
async fn coalesced_adoption_cannot_settle_an_independent_newer_failure() {
    for scan_fails in [false, true] {
        let (server, client) = stable_queue(75).await;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let polls = SourcePolls::default();
        let source = Source::default();
        let list = CachedList::Reviewing;
        scan_and_publish(&polls, &conn, &client, 1000).await;
        if scan_fails {
            server.reset().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(503))
                .mount(&server)
                .await;
        }
        let (a, _) = polls.begin_attempt(source.clone(), list).await;
        let (b, _) = polls.begin_attempt(source.clone(), list).await;
        let result = client
            .advance_scan(
                list,
                queue_scan::load(&conn, &source, list, "synthetic-viewer").unwrap(),
                &saved(&conn),
                1015,
            )
            .await
            .unwrap();
        let (independent, _) = polls.begin_attempt(source.clone(), list).await;
        let permit = polls.publication(&independent).await.unwrap();
        polls.complete(
            permit,
            Err(Failure {
                message: "Independent newer failure".into(),
                transient: false,
                not_asked: false,
            }),
            |_| {},
        );
        publish_scan_attempt(&polls, &conn, &a, result.clone()).await;
        let first = polls.get(&source, list);
        assert_eq!(first.phase, Phase::Failed);
        assert_eq!(first.error.as_deref(), Some("Independent newer failure"));
        assert_eq!(first.consecutive_failures, 1);
        assert!(publish_scan_attempt(&polls, &conn, &b, result)
            .await
            .is_none());
        let after = polls.get(&source, list);
        assert_eq!(after.phase, first.phase);
        assert_eq!(after.error, first.error);
        assert_eq!(after.consecutive_failures, first.consecutive_failures);
        assert_eq!(after.last_received_at, first.last_received_at);
        assert_eq!(after.receipt_revision, first.receipt_revision);
    }
}

// Separate the virtual cadence assertion from real HTTP/SQLite completion.
// Entry must occur before another real 15-second sleep can rescue a missed tick.
#[derive(Debug, PartialEq)]
struct ContinuationEvent {
    list: CachedList,
    logical_now: i64,
    entered: bool,
    at: tokio::time::Instant,
}
async fn continuation_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ContinuationEvent>,
    worker: &mut tokio::task::JoinHandle<()>,
    list: CachedList,
    logical_now: i64,
    entered: bool,
    not_before: tokio::time::Instant,
) -> Result<tokio::time::Instant, String> {
    // Completion includes the existing 30s request context plus a 5s SQLite
    // busy allowance. This is a diagnostic watchdog, not a new product SLA.
    let limit = Duration::from_secs(if entered { 3 } else { 35 });
    let deadline = not_before + limit;
    let event = tokio::select! {
        // A timely queued event remains valid if the test task is polled late.
        // Worker failure wins; then event timestamps decide before the watchdog.
        biased;
        result = worker => return Err(format!("continuation worker ended before expected event: {result:?}")),
        event = rx.recv() => event.ok_or_else(|| "continuation event channel closed".to_string())?,
        _ = tokio::time::sleep_until(deadline) => return Err(format!("continuation {} timed out: list={list:?} logical_now={logical_now}", if entered { "entry" } else { "completion" })),
    };
    if event.list != list
        || event.logical_now != logical_now
        || event.entered != entered
        || event.at < not_before
        || event.at > deadline
    {
        return Err(format!("unexpected continuation event: {event:?}; expected list={list:?} logical_now={logical_now} entered={entered} not_before={not_before:?} deadline={deadline:?}"));
    }
    Ok(event.at)
}
async fn continuation_round(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ContinuationEvent>,
    worker: &mut tokio::task::JoinHandle<()>,
    logical_now: i64,
    tick: tokio::time::Instant,
) -> Result<(), String> {
    let mut phase_start = tick;
    for list in [CachedList::Authored, CachedList::Reviewing] {
        let entered = continuation_event(rx, worker, list, logical_now, true, phase_start).await?;
        phase_start = continuation_event(rx, worker, list, logical_now, false, entered).await?;
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn continuation_phase_guard_rejects_missing_tick_before_next_wake() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let start = tokio::time::Instant::now();
    let mut worker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        tx.send(ContinuationEvent {
            list: CachedList::Authored,
            logical_now: 1015,
            entered: true,
            at: tokio::time::Instant::now(),
        })
        .unwrap();
        std::future::pending::<()>().await;
    });
    let error = continuation_event(
        &mut rx,
        &mut worker,
        CachedList::Authored,
        1015,
        true,
        start,
    )
    .await
    .unwrap_err();
    assert!(error.contains("entry timed out"), "{error}");
    assert_eq!(start.elapsed(), Duration::from_secs(3));
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn continuation_phase_guard_rejects_wrong_identity_phase_and_out_of_window_event() {
    let start = tokio::time::Instant::now();
    for (list, logical_now, entered, at) in [
        (CachedList::Reviewing, 1015, true, start),
        (CachedList::Authored, 1030, true, start),
        (CachedList::Authored, 1015, false, start),
        (
            CachedList::Authored,
            1015,
            true,
            start - Duration::from_secs(1),
        ),
        (
            CachedList::Authored,
            1015,
            true,
            start + Duration::from_secs(4),
        ),
    ] {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(ContinuationEvent {
            list,
            logical_now,
            entered,
            at,
        })
        .unwrap();
        let mut worker = tokio::spawn(std::future::pending::<()>());
        let error = continuation_event(
            &mut rx,
            &mut worker,
            CachedList::Authored,
            1015,
            true,
            start,
        )
        .await
        .unwrap_err();
        assert!(error.contains("unexpected continuation event"), "{error}");
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn continuation_phase_guard_uses_event_time_when_observed_after_deadline() {
    for entered in [true, false] {
        for timely in [true, false] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let start = tokio::time::Instant::now();
            let limit = Duration::from_secs(if entered { 3 } else { 35 });
            let at = start
                + if timely {
                    limit
                } else {
                    limit + Duration::from_millis(1)
                };
            tx.send(ContinuationEvent {
                list: CachedList::Authored,
                logical_now: 1015,
                entered,
                at,
            })
            .unwrap();
            let mut worker = tokio::spawn(std::future::pending::<()>());
            tokio::time::advance(limit + Duration::from_secs(1)).await;
            let result = continuation_event(
                &mut rx,
                &mut worker,
                CachedList::Authored,
                1015,
                entered,
                start,
            )
            .await;
            if timely {
                assert_eq!(result.unwrap(), at);
            } else {
                assert!(result
                    .unwrap_err()
                    .contains("unexpected continuation event"));
            }
            worker.abort();
            assert!(worker.await.unwrap_err().is_cancelled());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn continuation_phase_guard_reports_worker_failure_without_waiting() {
    let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let start = tokio::time::Instant::now();
    let mut worker = tokio::spawn(async { panic!("controlled worker failure") });
    let error = continuation_event(
        &mut rx,
        &mut worker,
        CachedList::Authored,
        1015,
        true,
        start,
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("worker ended") && error.contains("controlled worker failure"),
        "{error}"
    );
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn continuation_phase_guard_allows_io_after_verified_entry() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let start = tokio::time::Instant::now();
    let mut worker = tokio::spawn(async move {
        for list in [CachedList::Authored, CachedList::Reviewing] {
            tx.send(ContinuationEvent {
                list,
                logical_now: 1015,
                entered: true,
                at: tokio::time::Instant::now(),
            })
            .unwrap();
            tokio::time::sleep(Duration::from_millis(3200)).await;
            tx.send(ContinuationEvent {
                list,
                logical_now: 1015,
                entered: false,
                at: tokio::time::Instant::now(),
            })
            .unwrap();
        }
        std::future::pending::<()>().await;
    });
    continuation_round(&mut rx, &mut worker, 1015, start)
        .await
        .unwrap();
    assert_eq!(start.elapsed(), Duration::from_millis(6400));
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
}

/// This drives the same sleeping dispatcher and persisted fetch path used by
/// poll::spawn, rather than calling advance_scan at a list of timestamps.
#[tokio::test(start_paused = true)]
async fn production_continuation_timer_respects_visibility_and_finishes_durable_passes() {
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    tokio::time::resume();
    let (server, client) = stable_queue(275).await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = Arc::new(SourcePolls::default());
    let client = Arc::new(client);
    let now = Arc::new(AtomicI64::new(1000));
    let focused = Arc::new(AtomicBool::new(true));
    let needed = Arc::new(AtomicBool::new(true));
    let enabled = Arc::new(AtomicBool::new(true));
    let mut pass_start = server.received_requests().await.unwrap().len();
    let first = fetch_github_step_at(
        path.clone(),
        &client,
        CachedList::Reviewing,
        Duration::from_secs(30),
        false,
        || (120, 1000),
    )
    .await
    .unwrap();
    publish(&polls, &conn, Ok(first)).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut worker = tokio::spawn(crate::poll::run_queue_continuations(
        focused.clone(),
        needed.clone(),
        enabled.clone(),
        {
            let client = client.clone();
            let polls = polls.clone();
            let path = path.clone();
            let now = now.clone();
            move |list| {
                let client = client.clone();
                let polls = polls.clone();
                let path = path.clone();
                let now = now.clone();
                let tx = tx.clone();
                async move {
                    let dispatch_now = now.load(Ordering::SeqCst);
                    tx.send(ContinuationEvent {
                        list,
                        logical_now: dispatch_now,
                        entered: true,
                        at: tokio::time::Instant::now(),
                    })
                    .unwrap();
                    // The production fetch adapter rechecks the durable owner,
                    // continuation eligibility and attempt limit at dispatch.
                    let scoped = client.with_read_context(ReadContext::new(
                        ReadClass::Background,
                        Duration::from_secs(30),
                    ));
                    let (attempt, _) = polls.begin_attempt(Source::default(), list).await;
                    let result = fetch_github_step_at(
                        path.clone(),
                        &scoped,
                        list,
                        Duration::from_secs(30),
                        true,
                        || (120, dispatch_now),
                    )
                    .await
                    .unwrap();
                    let permit = polls.success_publication(&attempt).await.unwrap();
                    let result = reconcile_github_at(path, &polls, &permit, result).await;
                    polls.complete(permit, result, |_| {});
                    tx.send(ContinuationEvent {
                        list,
                        logical_now: dispatch_now,
                        entered: false,
                        at: tokio::time::Instant::now(),
                    })
                    .unwrap();
                }
            }
        },
    ));
    tokio::task::yield_now().await;
    tokio::time::pause();
    for flag in [&focused, &needed, &enabled] {
        flag.store(false, Ordering::SeqCst);
        let before = server.received_requests().await.unwrap().len();
        now.fetch_add(15, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), before);
        flag.store(true, Ordering::SeqCst);
    }
    for pass in 0..3 {
        if pass > 0 {
            pass_start = server.received_requests().await.unwrap().len();
            now.fetch_add(120, Ordering::SeqCst);
            tokio::time::resume();
            let first = fetch_github_step_at(
                path.clone(),
                &client,
                CachedList::Reviewing,
                Duration::from_secs(30),
                false,
                || (120, now.load(Ordering::SeqCst)),
            )
            .await
            .unwrap();
            publish(&polls, &conn, Ok(first)).await;
            tokio::time::pause();
        }
        for _ in 0..5 {
            let before = server.received_requests().await.unwrap().len();
            now.fetch_add(15, Ordering::SeqCst);
            let tick = tokio::time::Instant::now() + Duration::from_secs(15);
            tokio::time::advance(Duration::from_secs(15)).await;
            // Real sockets and blocking SQLite are allowed to complete. Virtual
            // time drives the timer; it does not stand in for provider completion.
            tokio::time::resume();
            continuation_round(&mut rx, &mut worker, now.load(Ordering::SeqCst), tick)
                .await
                .unwrap();
            tokio::time::pause();
            assert!(server.received_requests().await.unwrap().len() - before <= 3);
            if pass > 0 {
                assert_eq!(saved(&conn).len(), 275);
                assert_eq!(
                    polls
                        .get(&Source::default(), CachedList::Reviewing)
                        .coverage,
                    Some(Coverage::Complete)
                );
            }
        }
        assert!(server.received_requests().await.unwrap().len() - pass_start <= 16);
        assert_eq!(saved(&conn).len(), 275);
        assert_eq!(
            polls
                .get(&Source::default(), CachedList::Reviewing)
                .coverage,
            Some(Coverage::Complete)
        );
        assert_eq!(
            saved(&conn)
                .iter()
                .map(|row| row.identity())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            275
        );
    }
    let before = server.received_requests().await.unwrap().len();
    let rows = saved(&conn);
    let status_before = polls.get(&Source::default(), CachedList::Reviewing);
    // Even well after the next-pass deadline, the continuation timer must
    // not start a fresh scan. Only an ordinary/foreground refresh can do so.
    now.fetch_add(3600, Ordering::SeqCst);
    let tick = tokio::time::Instant::now() + Duration::from_secs(15);
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();
    continuation_round(&mut rx, &mut worker, now.load(Ordering::SeqCst), tick)
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(saved(&conn), rows);
    assert_eq!(
        polls
            .get(&Source::default(), CachedList::Reviewing)
            .coverage,
        Some(Coverage::Complete)
    );
    let status_after = polls.get(&Source::default(), CachedList::Reviewing);
    assert_eq!(status_after.phase, Phase::Ready);
    assert_eq!(
        status_after.last_received_at,
        status_before.last_received_at
    );
    worker.abort();
    let _ = worker.await;
}

#[tokio::test(start_paused = true)]
async fn pending_continuation_allows_real_detail_and_review_and_cannot_erase_receipt() {
    use crate::github::mutate::{BoundReviewOutcome, BoundReviewRequest};
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
    tokio::time::resume();
    let hold = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let router = axum::Router::new().route("/graphql", axum::routing::post({
        let hold = hold.clone(); let entered = entered.clone(); let release = release.clone();
        let active = active.clone(); let peak = peak.clone(); let requests = requests.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let hold = hold.clone(); let entered = entered.clone(); let release = release.clone();
            let active = active.clone(); let peak = peak.clone(); let requests = requests.clone();
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                requests.lock().unwrap().push(body.clone());
                let query = body["query"].as_str().unwrap();
                let data = if query.contains("addPullRequestReview") {
                    json!({"addPullRequestReview":{"pullRequestReview":{"id":"REVIEW-1","state":"APPROVED","submittedAt":"2026-10-01T12:00:00Z","author":{"login":"synthetic-viewer"},"commit":{"oid":"head-1"},"pullRequest":{"id":"PR_1","number":1,"repository":{"nameWithOwner":"octocat/repo-1"}}}}})
                } else if query.contains("isMergeQueueEnabled") {
                    json!({"repository":{"pullRequest":{"id":"PR_1","number":1,"title":"Synthetic review 1","body":"Read while continuation is pending","headRefOid":"head-1","baseRefName":"main","mergeStateStatus":"CLEAN","commits":{"nodes":[]},"comments":{"totalCount":1,"nodes":[{"id":"C1","body":"Synthetic comment","createdAt":"2026-10-01T12:00:00Z","author":{"login":"synthetic-author"}}]}}}})
                } else if query.contains("search(") {
                    if hold.swap(false, Ordering::SeqCst) { entered.notify_one(); release.notified().await; }
                    let start = body["variables"]["after"].as_str().and_then(|s|s.strip_prefix("cursor-")).and_then(|s|s.parse::<usize>().ok()).unwrap_or(0);
                    let end = (start+25).min(275);
                    json!({"viewer":{"login":"synthetic-viewer"},"authored":{"issueCount":275,"nodes":(start+1..=end).map(|n|node(n,false)).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":end<275,"endCursor":format!("cursor-{end}")}}})
                } else { json!({"viewer":{"login":"synthetic-viewer"}}) };
                active.fetch_sub(1, Ordering::SeqCst);
                axum::Json(json!({"data":data}))
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = Arc::new(GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(format!("http://{address}"))
            .unwrap()
            .personal_token("synthetic-token")
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .build()
            .unwrap(),
    ));
    client.fetch_viewer().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("overlap.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = Arc::new(SourcePolls::default());
    for step in 0..6 {
        let fetched = fetch_github_step_at(
            path.clone(),
            &client,
            CachedList::Reviewing,
            Duration::from_secs(30),
            false,
            || (120, 1000 + 15 * step),
        )
        .await
        .unwrap();
        publish(&polls, &conn, Ok(fetched)).await;
    }
    assert_eq!(saved(&conn).len(), 275);
    let first = fetch_github_step_at(
        path.clone(),
        &client,
        CachedList::Reviewing,
        Duration::from_secs(30),
        false,
        || (120, 1195),
    )
    .await
    .unwrap();
    publish(&polls, &conn, Ok(first)).await;
    let baseline = saved(&conn);
    let before = requests.lock().unwrap().len();
    hold.store(true, Ordering::SeqCst);
    let now = Arc::new(AtomicI64::new(1210));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let visible = Arc::new(AtomicBool::new(true));
    let worker = tokio::spawn(crate::poll::run_queue_continuations(
        visible.clone(),
        visible.clone(),
        visible,
        {
            let client = client.clone();
            let path = path.clone();
            let polls = polls.clone();
            move |list| {
                let client = client.clone();
                let path = path.clone();
                let polls = polls.clone();
                let now = now.clone();
                let tx = tx.clone();
                async move {
                    if list == CachedList::Authored {
                        return;
                    }
                    let (attempt, _) = polls.begin_attempt(Source::default(), list).await;
                    let scoped = client.with_read_context(ReadContext::new(
                        ReadClass::Background,
                        Duration::from_secs(30),
                    ));
                    let fetched = fetch_github_step_at(
                        path.clone(),
                        &scoped,
                        list,
                        Duration::from_secs(30),
                        true,
                        || (120, now.load(Ordering::SeqCst)),
                    )
                    .await
                    .unwrap();
                    let published = if let Some(permit) = polls.success_publication(&attempt).await
                    {
                        let result = reconcile_github_at(path, &polls, &permit, fetched).await;
                        polls.complete(permit, result, |_| {});
                        true
                    } else {
                        false
                    };
                    tx.send(published).unwrap();
                }
            }
        },
    ));
    tokio::task::yield_now().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 1);
    let foreground = client.with_read_context(ReadContext::new(
        ReadClass::Foreground,
        Duration::from_secs(3),
    ));
    let detail = tokio::time::timeout(
        Duration::from_secs(3),
        foreground.fetch_pr_detail("octocat/repo-1", 1),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(detail.body, "Read while continuation is pending");
    assert_eq!(detail.comments.len(), 1);
    assert_eq!(
        active.load(Ordering::SeqCst),
        1,
        "background response is still held"
    );
    let outcome = foreground
        .add_review_at_head(&BoundReviewRequest {
            id: detail.id,
            repo: "octocat/repo-1".into(),
            number: 1,
            verdict: "approve".into(),
            body: String::new(),
            expected_head: detail.head_oid,
            expected_viewer: "synthetic-viewer".into(),
        })
        .await;
    let BoundReviewOutcome::Acknowledged { receipt } = outcome else {
        panic!("review not acknowledged: {outcome:?}");
    };
    assert_eq!(receipt.commit_oid, "head-1");
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Review(crate::inventory::ConfirmedReview {
            head_oid: receipt.commit_oid.clone(),
            review: crate::github::model::ReviewState::Approved,
            confirmed_at: chrono::Utc::now(),
            receipt: Some(receipt),
            unresolved: false,
            confirmed_by_read: false,
        }),
        |event| {
            assert!(event.is_ok());
        },
    )
    .await;
    let confirmed = saved(&conn);
    assert!(confirmed[0]
        .observation
        .as_ref()
        .unwrap()
        .confirmed_review
        .is_some());
    assert_eq!(confirmed.len(), baseline.len());
    assert_eq!(
        confirmed[0].observation.as_ref().unwrap().last_observed_at,
        baseline[0].observation.as_ref().unwrap().last_observed_at
    );
    let held_calls = requests.lock().unwrap().len();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        requests.lock().unwrap().len(),
        held_calls,
        "no second timer dispatch while provider is pending"
    );
    tokio::time::resume();
    release.notify_one();
    assert!(
        !tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        "pre-review continuation must not publish over the receipt"
    );
    assert_eq!(saved(&conn), confirmed);
    {
        let calls = requests.lock().unwrap();
        assert!(
            calls.len() - before <= 5,
            "three background attempts plus one detail and one write"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|b| b["query"]
                    .as_str()
                    .unwrap()
                    .contains("addPullRequestReview"))
                .count(),
            1
        );
        let write = calls
            .iter()
            .find(|body| {
                body["query"]
                    .as_str()
                    .unwrap()
                    .contains("addPullRequestReview")
            })
            .unwrap();
        assert_eq!(write["variables"]["head"], "head-1");
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "actual simultaneous provider handlers"
        );
    }
    let completed_calls = requests.lock().unwrap().len();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(14)).await;
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err());
    assert_eq!(
        requests.lock().unwrap().len(),
        completed_calls,
        "sleep is measured after completion, not original dispatch"
    );
    tokio::time::resume();
    worker.abort();
    let _ = worker.await;
    server.abort();
    let _ = server.await;
}

/// B starts behind A but cannot load its checkpoint until A has committed.
/// Unlike coalesced reads, B then sees a NEW revision and legitimately does no HTTP.
async fn delayed_no_work_readback(
    recovery: bool,
    independent_failure: bool,
    settle_before_a: bool,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let server = MockServer::start().await;
    let fail = Arc::new(AtomicBool::new(false));
    let served_fail = fail.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if served_fail.load(Ordering::SeqCst) {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"data":{
                    "viewer":{"login":"synthetic-viewer"},
                    "authored":{"issueCount":20,"nodes":(1..=20).map(|n|node(n,false)).collect::<Vec<_>>(),
                    "pageInfo":{"hasNextPage":false,"endCursor":"cursor-20"}}
                }}))
            }
        })
        .mount(&server)
        .await;
    let client = Arc::new(github_client(&server));
    client.fetch_viewer().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("delayed-readback.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let source = Source::default();
    let list = CachedList::Reviewing;
    let first = fetch_github_step_at(
        path.clone(),
        &client,
        list,
        Duration::from_secs(30),
        false,
        || (120, 1000),
    )
    .await
    .unwrap();
    publish(&polls, &conn, Ok(first)).await;
    if recovery {
        fail.store(true, Ordering::SeqCst);
        // Actual ownership-read failures settle Failed without tainting the
        // previously completed durable traversal. A can then recover Complete.
        let unverified = github_client(&server);
        for now in [1200, 1400] {
            let error = fetch_github_step_at(
                path.clone(),
                &unverified,
                list,
                Duration::from_secs(30),
                false,
                || (120, now),
            )
            .await
            .err()
            .expect("actual provider failure");
            publish(&polls, &conn, Err(Failure::from(&error))).await;
        }
    }
    let baseline = polls.get(&source, list);
    assert_eq!(
        baseline.phase,
        if recovery {
            Phase::Failed
        } else {
            Phase::Ready
        }
    );
    fail.store(!recovery, Ordering::SeqCst);
    conn.execute_batch("CREATE TABLE readback_writes (kind TEXT); CREATE TRIGGER audit_readback_snapshot AFTER UPDATE ON snapshot BEGIN INSERT INTO readback_writes VALUES('snapshot'); END; CREATE TRIGGER audit_readback_checkpoint AFTER UPDATE ON queue_scan BEGIN INSERT INTO readback_writes VALUES('checkpoint'); END;").unwrap();
    let (a, _) = polls.begin_attempt(source.clone(), list).await;
    let b = polls
        .begin_and_emit(source.clone(), list, Some("delayed-B".into()), |_| {})
        .await;
    let (release, held) = tokio::sync::oneshot::channel();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let delayed = tokio::spawn({
        let client = client.clone();
        let path = path.clone();
        async move {
            entered.send(()).unwrap();
            held.await.unwrap();
            fetch_github_step_at(path, &client, list, Duration::from_secs(30), true, || {
                (120, 2001)
            })
            .await
            .unwrap()
        }
    });
    waiting.await.unwrap();
    let before_http = server.received_requests().await.unwrap().len();
    let real = fetch_github_step_at(
        path.clone(),
        &client,
        list,
        Duration::from_secs(30),
        false,
        || (120, 2000),
    )
    .await
    .unwrap();
    assert!(!real.scan.as_ref().unwrap().state.no_work);
    assert_eq!(
        real.scan.as_ref().unwrap().state.step_failure.is_some(),
        !recovery
    );
    let mut independent = None;
    if independent_failure && settle_before_a {
        independent = Some(settle_independent_readback_failure(&polls).await);
    }
    let permit = polls.success_publication(&a).await.unwrap();
    let real = reconcile_github_at(path.clone(), &polls, &permit, real)
        .await
        .unwrap_or_else(|f| panic!("{}", f.message));
    polls.complete(permit, Ok(real), |_| {});
    let published = polls.get(&source, list);
    assert_eq!(
        published.phase,
        if independent_failure && settle_before_a {
            Phase::Failed
        } else {
            Phase::Fetching
        },
        "A publishes beside the newer pending or independently settled request"
    );
    assert!(published.receipt_revision > baseline.receipt_revision);
    let checkpoint = queue_scan::load(&conn, &source, list, "synthetic-viewer").unwrap();
    let rows = saved(&conn);
    let snapshot = source_cache::load_source_snapshot(&conn, &source, list).unwrap();
    let fetched_at = match &snapshot.data {
        SnapshotData::Available { fetched_at, .. } => fetched_at.clone(),
        _ => panic!("missing snapshot"),
    };
    let writes: i64 = conn
        .query_row("SELECT count(*) FROM readback_writes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM readback_writes WHERE kind='checkpoint'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    // Pure failure saves qualified rows then restores the existing fetched_at.
    assert_eq!(writes, if recovery { 2 } else { 3 });
    let after_http = server.received_requests().await.unwrap().len();
    assert!((1..=3).contains(&(after_http - before_http)));
    if independent_failure && !settle_before_a {
        independent = Some(settle_independent_readback_failure(&polls).await);
    }
    release.send(()).unwrap();
    let readback = delayed.await.unwrap();
    assert!(readback.scan.as_ref().unwrap().state.no_work);
    assert_eq!(
        readback.scan.as_ref().unwrap().expected_revision,
        checkpoint.revision
    );
    assert_eq!(
        readback.scan.as_ref().unwrap().state.receipt_id,
        checkpoint.state.receipt_id
    );
    let permit = polls.success_publication(&b).await.unwrap();
    let prepared = reconcile_github_at(path.clone(), &polls, &permit, readback)
        .await
        .unwrap_or_else(|f| panic!("{}", f.message));
    let mut emitted = None;
    polls.complete(permit, Ok(prepared), |status| emitted = Some(status));
    assert_eq!(emitted.is_some(), !independent_failure);
    let expected = if let Some(status) = independent {
        status
    } else {
        let status = polls.get(&source, list);
        assert_eq!(
            status.phase,
            if recovery {
                Phase::Ready
            } else {
                Phase::Retrying
            }
        );
        assert_eq!(status.error.is_some(), !recovery);
        assert_eq!(status.consecutive_failures, if recovery { 0 } else { 1 });
        assert_eq!(status.request_id.as_deref(), Some("delayed-B"));
        status
    };
    // A fresh current no-work request must not double count A or displace a
    // separately settled newer failure. Both publication orders retain age.
    for _ in 0..2 {
        let (next, _) = polls.begin_attempt(source.clone(), list).await;
        let noop = fetch_github_step_at(
            path.clone(),
            &client,
            list,
            Duration::from_secs(30),
            true,
            || (120, 2001),
        )
        .await
        .unwrap();
        let permit = polls.success_publication(&next).await.unwrap();
        let noop = reconcile_github_at(path.clone(), &polls, &permit, noop)
            .await
            .unwrap_or_else(|f| panic!("{}", f.message));
        polls.complete(permit, Ok(noop), |_| {});
        let status = polls.get(&source, list);
        assert_eq!(status.phase, expected.phase);
        assert_eq!(status.error, expected.error);
        assert_eq!(status.consecutive_failures, expected.consecutive_failures);
        assert_eq!(status.receipt_revision, published.receipt_revision);
        assert_eq!(status.last_received_at, published.last_received_at);
    }
    assert_eq!(saved(&conn), rows);
    assert_eq!(
        queue_scan::load(&conn, &source, list, "synthetic-viewer")
            .unwrap()
            .revision,
        checkpoint.revision
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM readback_writes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        writes
    );
    if let SnapshotData::Available {
        fetched_at: after, ..
    } = source_cache::load_source_snapshot(&conn, &source, list)
        .unwrap()
        .data
    {
        assert_eq!(after, fetched_at);
    }
    assert_eq!(server.received_requests().await.unwrap().len(), after_http);
    if !recovery {
        assert_eq!(published.last_received_at, baseline.last_received_at);
    }
}

#[tokio::test]
async fn delayed_no_work_readback_adopts_failure_once() {
    delayed_no_work_readback(false, false, false).await;
}
#[tokio::test]
async fn delayed_no_work_readback_adopts_recovery_without_restamping() {
    delayed_no_work_readback(true, false, false).await;
}
#[tokio::test]
async fn delayed_no_work_readback_preserves_independent_newer_failure() {
    for settle_before_a in [false, true] {
        delayed_no_work_readback(false, true, settle_before_a).await;
        delayed_no_work_readback(true, true, settle_before_a).await;
    }
}

async fn settle_independent_readback_failure(polls: &SourcePolls) -> Status {
    let source = Source::default();
    let list = CachedList::Reviewing;
    let (newer, _) = polls.begin_attempt(source.clone(), list).await;
    let permit = polls.publication(&newer).await.unwrap();
    polls.complete(
        permit,
        Err(Failure {
            message: "Independent newer failure".into(),
            transient: false,
            not_asked: false,
        }),
        |_| {},
    );
    polls.get(&source, list)
}

// These regressions use the same durable dispatch/publication boundary as desktop
// and paired callers. Only the external provider is synthetic.
async fn terminal_provider(fail_first: bool) -> (MockServer, GitHubClient) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let server = MockServer::start().await;
    let fail = AtomicBool::new(fail_first);
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let mut data = json!({"viewer":{"login":"synthetic-viewer"}});
        if body["variables"]["n0"].is_number() {
            if fail.swap(false, Ordering::SeqCst) {
                return ResponseTemplate::new(503);
            }
            for i in 0..4 {
                if let Some(n) = body["variables"][format!("n{i}")].as_u64() {
                    let mut direct = node(n as usize, false);
                    direct["state"] = json!("CLOSED");
                    data[format!("p{i}")] = json!({"pullRequest":direct});
                }
            }
        } else {
            data["authored"] = json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}});
        }
        ResponseTemplate::new(200).set_body_json(json!({"data":data}))
    }).mount(&server).await;
    let client = github_client(&server);
    client.fetch_viewer().await.unwrap();
    (server, client)
}
fn seed_terminal_inventory(
    conn: &rusqlite::Connection,
    list: CachedList,
    count: usize,
    done: bool,
) {
    let raw = json!({"authored":{"nodes":(1..=count).map(|n|node(n,false)).collect::<Vec<_>>()}});
    let rows = crate::github::map::map_list(&raw, "authored");
    assert_eq!(rows.len(), count);
    let candidates = rows
        .iter()
        .map(|r| queue_scan::Candidate {
            isolated: false,
            identity: r.identity(),
            id: r.id.clone(),
            head: r.head_oid.clone(),
            created_at: r.created_at,
            negative_at: None,
            eligible_at: 1000,
            failures: 0,
        })
        .collect();
    reconcile_github_snapshot(
        conn,
        &Source::default(),
        list,
        FetchedList {
            viewer: Some("synthetic-viewer".into()),
            prs: rows,
            total: Some(0),
            coverage: Coverage::Complete,
            scan: Some(queue_scan::Commit {
                expected_revision: 0,
                removals: vec![],
                state: queue_scan::State {
                    candidates,
                    done,
                    started_at: Some(900),
                    finished_at: done.then_some(1000),
                    completed_at: Some(1000),
                    completed_total: Some(0),
                    coverage_valid: true,
                    total: Some(0),
                    count_seen: true,
                    eligible_at: 1000,
                    ..Default::default()
                },
            }),
        },
        None,
    )
    .unwrap_or_else(|e| panic!("{}", e.message));
}
async fn durable_terminal_step(
    path: std::path::PathBuf,
    polls: &SourcePolls,
    client: &GitHubClient,
    list: CachedList,
    now: i64,
    continuation: bool,
) {
    let (attempt, _) = polls.begin_attempt(Source::default(), list).await;
    let result = fetch_github_step_at(
        path.clone(),
        client,
        list,
        Duration::from_secs(30),
        continuation,
        || (120, now),
    )
    .await
    .unwrap();
    let permit = polls.success_publication(&attempt).await.unwrap();
    let result = reconcile_github_at(path, polls, &permit, result).await;
    assert!(result.is_ok(), "durable publication failed");
    polls.complete(permit, result, |_| {});
}
fn terminal_inventory(
    conn: &rusqlite::Connection,
    list: CachedList,
) -> Vec<crate::github::model::PullRequest> {
    let SnapshotData::Available { prs, .. } =
        source_cache::load_source_snapshot(conn, &Source::default(), list)
            .unwrap()
            .data
    else {
        panic!("missing inventory")
    };
    prs
}
fn confirmation_numbers(request: &wiremock::Request) -> Vec<u64> {
    let body: Value = request.body_json().unwrap();
    (0..4)
        .filter_map(|i| body["variables"][format!("n{i}")].as_u64())
        .collect()
}
#[tokio::test]
async fn terminal_proofs_are_not_reseeded_at_discovery_completion() {
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("terminal.sqlite");
    let polls = SourcePolls::default();
    let list = CachedList::Reviewing;
    seed_terminal_inventory(&crate::store::open_db(&path).unwrap(), list, 8, false);
    for now in [1000, 1120] {
        durable_terminal_step(path.clone(), &polls, &client, list, now, false).await;
        let conn = crate::store::open_db(&path).unwrap();
        let rows = terminal_inventory(&conn, list);
        let loaded = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
        assert!(
            loaded
                .state
                .candidates
                .iter()
                .all(|c| rows.iter().any(|r| r.identity() == c.identity)),
            "confirmed removals must never survive in durable candidates"
        );
    }
    assert!(terminal_inventory(&crate::store::open_db(&path).unwrap(), list).is_empty());
    let numbers: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .flat_map(confirmation_numbers)
        .collect();
    assert_eq!(numbers, (1..=8).collect::<Vec<_>>());
}
#[tokio::test]
async fn transient_confirmation_failure_recovers_batching_after_reload() {
    for continuation in [false, true] {
        let (server, client) = terminal_provider(true).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.sqlite");
        let polls = SourcePolls::default();
        let list = CachedList::Reviewing;
        seed_terminal_inventory(&crate::store::open_db(&path).unwrap(), list, 150, true);
        // Refresh crosses fresh_pass; Continue exercises the 15-second cadence.
        for step in 1..=42 {
            durable_terminal_step(
                path.clone(),
                &polls,
                &client,
                list,
                1000 + step * if continuation { 15 } else { 60 },
                continuation,
            )
            .await;
        }
        assert!(
            terminal_inventory(&crate::store::open_db(&path).unwrap(), list).is_empty(),
            "healthy batching must recover within 42 opportunities"
        );
        let requests = server.received_requests().await.unwrap();
        let batches: Vec<_> = requests
            .iter()
            .map(confirmation_numbers)
            .filter(|n| !n.is_empty())
            .collect();
        assert_eq!(batches.len(), 42);
        assert_eq!(batches.iter().filter(|n| n.len() == 1).count(), 4);
        let healthy: Vec<_> = batches.iter().skip(1).flatten().copied().collect();
        assert_eq!(healthy.len(), 150);
        assert_eq!(
            healthy
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            150
        );
    }
}
#[tokio::test(start_paused = true)]
async fn completed_discovery_continuations_drain_terminal_inventory_within_budget() {
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    tokio::time::resume();
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("timer-terminal.sqlite");
    for list in [CachedList::Authored, CachedList::Reviewing] {
        seed_terminal_inventory(&crate::store::open_db(&path).unwrap(), list, 150, true);
    }
    let baseline = server.received_requests().await.unwrap().len();
    let polls = Arc::new(SourcePolls::default());
    let client = Arc::new(client);
    let now = Arc::new(AtomicI64::new(1000));
    let flags = [
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(true)),
    ];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut worker = tokio::spawn(crate::poll::run_queue_continuations(
        flags[0].clone(),
        flags[1].clone(),
        flags[2].clone(),
        {
            let polls = polls.clone();
            let path = path.clone();
            let now = now.clone();
            move |list| {
                let polls = polls.clone();
                let client = client.clone();
                let path = path.clone();
                let now = now.clone();
                let tx = tx.clone();
                async move {
                    let dispatch_now = now.load(Ordering::SeqCst);
                    tx.send(ContinuationEvent {
                        list,
                        logical_now: dispatch_now,
                        entered: true,
                        at: tokio::time::Instant::now(),
                    })
                    .unwrap();
                    let scoped = client.with_read_context(ReadContext::new(
                        ReadClass::Background,
                        Duration::from_secs(30),
                    ));
                    durable_terminal_step(path, &polls, &scoped, list, dispatch_now, true).await;
                    tx.send(ContinuationEvent {
                        list,
                        logical_now: dispatch_now,
                        entered: false,
                        at: tokio::time::Instant::now(),
                    })
                    .unwrap();
                }
            }
        },
    ));
    tokio::task::yield_now().await;
    tokio::time::pause();
    for flag in &flags {
        flag.store(false, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), baseline);
        flag.store(true, Ordering::SeqCst);
    }
    for round in 1..=39 {
        now.store(1000 + round * 15, Ordering::SeqCst);
        let tick = tokio::time::Instant::now() + Duration::from_secs(15);
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::time::resume();
        continuation_round(&mut rx, &mut worker, now.load(Ordering::SeqCst), tick)
            .await
            .unwrap();
        tokio::time::pause();
        let conn = crate::store::open_db(&path).unwrap();
        for list in [CachedList::Authored, CachedList::Reviewing] {
            let rows = terminal_inventory(&conn, list);
            let loaded =
                queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
            assert_eq!(
                rows.len(),
                150usize.saturating_sub(round as usize * 4),
                "continuation round {round}"
            );
            assert_eq!(loaded.state.candidates.len(), rows.len());
            assert_eq!(
                loaded.state.completed_at,
                Some(1000),
                "confirmation must not refresh discovery age"
            );
        }
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len() - baseline,
        76,
        "38 bounded documents per list; no extra discovery or exhausted work"
    );
    for reviewing in [false, true] {
        let numbers: Vec<_> = requests[baseline..]
            .iter()
            .filter(|r| {
                let b: Value = r.body_json().unwrap();
                b["variables"]["q0"]
                    .as_str()
                    .unwrap()
                    .contains("review-requested")
                    == reviewing
            })
            .flat_map(confirmation_numbers)
            .collect();
        assert_eq!(numbers, (1..=150).collect::<Vec<_>>());
    }
    worker.abort();
    let _ = worker.await;
}

fn rewrite_terminal_checkpoint(
    conn: &rusqlite::Connection,
    list: CachedList,
    edit: impl FnOnce(&mut Value),
) {
    let payload: String = conn
        .query_row(
            "SELECT payload FROM queue_scan WHERE list=?1",
            [list.id()],
            |r| r.get(0),
        )
        .unwrap();
    let mut payload: Value = serde_json::from_str(&payload).unwrap();
    edit(&mut payload);
    conn.execute(
        "UPDATE queue_scan SET payload=?1 WHERE list=?2",
        rusqlite::params![payload.to_string(), list.id()],
    )
    .unwrap();
}
#[tokio::test]
async fn completed_continue_is_confirmation_only_but_standalone_refresh_discovers() {
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mode.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Authored;
    let polls = SourcePolls::default();
    seed_terminal_inventory(&conn, list, 8, true);
    let before = server.received_requests().await.unwrap().len();
    durable_terminal_step(path.clone(), &polls, &client, list, 2000, true).await;
    assert_eq!(server.received_requests().await.unwrap().len() - before, 1);
    assert_eq!(
        queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
            .unwrap()
            .state
            .completed_at,
        Some(1000)
    );
    durable_terminal_step(path.clone(), &polls, &client, list, 2001, false).await;
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        3,
        "Refresh authorizes a new discovery document"
    );
    assert_eq!(
        queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
            .unwrap()
            .state
            .completed_at,
        Some(2001)
    );
    durable_terminal_step(path, &polls, &client, list, 4000, true).await;
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        3,
        "exhausted Continue is no work even after pass delay"
    );
}
#[tokio::test]
async fn due_confirmation_ignores_discovery_backoff_and_future_candidates_make_no_requests() {
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("due.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    let polls = SourcePolls::default();
    seed_terminal_inventory(&conn, list, 8, false);
    rewrite_terminal_checkpoint(&conn, list, |p| {
        p["eligible_at"] = json!(5000);
        for c in p["candidates"].as_array_mut().unwrap() {
            c["eligible_at"] = json!(3000);
        }
    });
    let before = server.received_requests().await.unwrap().len();
    let initial = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    durable_terminal_step(path.clone(), &polls, &client, list, 2000, true).await;
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(
        queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
            .unwrap()
            .state,
        initial.state
    );
    durable_terminal_step(path, &polls, &client, list, 3000, true).await;
    assert_eq!(server.received_requests().await.unwrap().len() - before, 1);
    assert_eq!(terminal_inventory(&conn, list).len(), 4);
}
#[tokio::test]
async fn confirmation_refusal_preserves_exact_durable_retry_and_proof_state() {
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refused.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    let polls = SourcePolls::default();
    seed_terminal_inventory(&conn, list, 8, true);
    rewrite_terminal_checkpoint(&conn, list, |p| {
        p["candidates"][0]["isolated"] = json!(true);
        p["candidates"][0]["negative_at"] = json!(900);
        p["candidates"][0]["failures"] = json!(2);
    });
    let initial = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    let rows = terminal_inventory(&conn, list);
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(json!({"message":"Synthetic provider cooldown"})),
        )
        .mount(&server)
        .await;
    assert!(client
        .fetch_viewer_metered(&client.request_budget())
        .await
        .is_err());
    let before = server.received_requests().await.unwrap().len();
    durable_terminal_step(path, &polls, &client, list, 2000, true).await;
    let after = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(after.revision, initial.revision);
    assert_eq!(after.state, initial.state);
    assert_eq!(terminal_inventory(&conn, list), rows);
}
#[tokio::test]
async fn legacy_isolation_preserves_proofs_and_poison_stays_singleton_after_one_probe() {
    let (server, client) = terminal_provider(false).await;
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let mut data = json!({"viewer":{"login":"synthetic-viewer"}});
            let mut errors = vec![];
            for i in 0..4 {
                if let Some(n) = body["variables"][format!("n{i}")].as_u64() {
                    if n == 1 {
                        errors.push(
                            json!({"message":"Synthetic poisoned alias","path":[format!("p{i}")]}),
                        );
                    } else {
                        let mut direct = node(n as usize, false);
                        direct["state"] = json!("CLOSED");
                        data[format!("p{i}")] = json!({"pullRequest":direct});
                    }
                }
            }
            ResponseTemplate::new(200).set_body_json(json!({"data":data,"errors":errors}))
        })
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    let polls = SourcePolls::default();
    seed_terminal_inventory(&conn, list, 150, true);
    rewrite_terminal_checkpoint(&conn, list, |p| {
        p["isolate"] = json!(true);
        for c in p["candidates"].as_array_mut().unwrap() {
            c.as_object_mut().unwrap().remove("isolated");
        }
        p["candidates"][0]["negative_at"] = json!(900);
        p["candidates"][0]["failures"] = json!(2);
        p["candidates"][149]["eligible_at"] = json!(1015);
    });
    let migrated = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
        .unwrap()
        .state;
    assert!(!migrated.isolate);
    assert!(migrated.candidates.iter().all(|c| !c.isolated));
    assert_eq!(migrated.candidates[0].negative_at, Some(900));
    assert_eq!(migrated.candidates[0].failures, 2);
    assert_eq!(migrated.candidates[149].eligible_at, 1015);
    for step in 0..50 {
        durable_terminal_step(path.clone(), &polls, &client, list, 1000 + step * 15, true).await;
        let state = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
            .unwrap()
            .state;
        if step == 0 {
            assert_eq!(state.candidates.iter().filter(|c| c.isolated).count(), 4);
            assert!(state
                .candidates
                .iter()
                .filter(|c| c.isolated)
                .all(|c| c.negative_at.is_none()));
        }
    }
    let rows = terminal_inventory(&conn, list);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].number, 1);
    let requests = server.received_requests().await.unwrap();
    let poisoned: Vec<_> = requests
        .iter()
        .map(confirmation_numbers)
        .filter(|n| n.contains(&1))
        .collect();
    assert_eq!(poisoned[0], vec![1, 2, 3, 4]);
    assert!(poisoned[1..].iter().all(|n| n == &vec![1]));
    let healthy: Vec<_> = requests
        .iter()
        .skip(1)
        .flat_map(confirmation_numbers)
        .filter(|n| *n != 1)
        .collect();
    assert_eq!(healthy.len(), 149);
    assert_eq!(
        healthy
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        149
    );
}

#[tokio::test]
async fn partition_completion_excludes_removed_candidates_and_positive_observation_wins() {
    for positive in [false, true] {
        let (server, client) = terminal_provider(false).await;
        if positive {
            server.reset().await;
            Mock::given(method("POST")).respond_with(|request:&wiremock::Request| {
                let body:Value=request.body_json().unwrap();let mut data=json!({"viewer":{"login":"synthetic-viewer"}});
                if body["variables"]["n0"].is_number() {
                    for i in 0..4 {if let Some(n)=body["variables"][format!("n{i}")].as_u64(){let mut direct=node(n as usize,false);direct["state"]=json!("CLOSED");data[format!("p{i}")]=json!({"pullRequest":direct});}}
                } else {data["authored"]=json!({"issueCount":1,"nodes":[node(1,false)],"pageInfo":{"hasNextPage":false,"endCursor":null}});}
                ResponseTemplate::new(200).set_body_json(json!({"data":data}))
            }).mount(&server).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partition.sqlite");
        let conn = crate::store::open_db(&path).unwrap();
        let list = CachedList::Reviewing;
        let polls = SourcePolls::default();
        seed_terminal_inventory(&conn, list, 8, false);
        rewrite_terminal_checkpoint(&conn, list, |p| {
            p["github_partition"] = serde_json::to_value(queue_scan::GithubPartition {
                phase: queue_scan::PartitionPhase::FinalCheck,
                lower: Some(0),
                upper: Some(2_000_000_000),
                windows_started: 1,
                ..Default::default()
            })
            .unwrap();
            p["total"] = json!(usize::from(positive));
        });
        durable_terminal_step(path, &polls, &client, list, 1000, false).await;
        let rows = terminal_inventory(&conn, list);
        let state = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
            .unwrap()
            .state;
        assert!(state.done);
        assert_eq!(state.candidates.len(), 4);
        assert!(state.candidates.iter().all(|c| c.identity.number >= 5));
        assert_eq!(rows.len(), 4 + usize::from(positive));
        assert_eq!(rows.iter().any(|r| r.number == 1), positive);
    }
}
#[tokio::test]
async fn healthy_isolated_absence_recovers_batching_without_shortening_negative_spacing() {
    let (server, client) = terminal_provider(false).await;
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let mut data = json!({"viewer":{"login":"synthetic-viewer"}});
            for i in 0..4 {
                if let Some(n) = body["variables"][format!("n{i}")].as_u64() {
                    let mut direct = node(n as usize, false);
                    direct["state"] = json!("OPEN");
                    data[format!("p{i}")] = json!({"pullRequest":direct});
                    data[format!("m{i}")] =
                        json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}});
                }
            }
            ResponseTemplate::new(200).set_body_json(json!({"data":data}))
        })
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("negative.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    let polls = SourcePolls::default();
    seed_terminal_inventory(&conn, list, 4, true);
    rewrite_terminal_checkpoint(&conn, list, |p| {
        for c in p["candidates"].as_array_mut().unwrap() {
            c["isolated"] = json!(true);
        }
    });
    for now in [1015, 1030, 1045, 1060, 1074] {
        durable_terminal_step(path.clone(), &polls, &client, list, now, true).await;
        assert_eq!(
            terminal_inventory(&conn, list).len(),
            4,
            "first negative or premature retry must retain"
        );
    }
    let state = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
        .unwrap()
        .state;
    assert!(state.candidates.iter().all(|c| !c.isolated));
    assert_eq!(state.candidates[0].negative_at, Some(1015));
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        4,
        "no due proof means zero HTTP"
    );
    durable_terminal_step(path, &polls, &client, list, 1120, true).await;
    assert!(terminal_inventory(&conn, list).is_empty());
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| confirmation_numbers(r).len())
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 1, 4]
    );
}

#[tokio::test]
async fn queued_continuation_promotes_foreground_refresh_and_publishes_checkpoint_once() {
    let (server, client) = terminal_provider(false).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("promoted.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    seed_terminal_inventory(&conn, list, 8, true);
    let baseline = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer")
        .unwrap()
        .revision;
    conn.execute_batch("CREATE TABLE checkpoint_writes (id INTEGER); CREATE TRIGGER audit_checkpoint AFTER UPDATE ON queue_scan BEGIN INSERT INTO checkpoint_writes VALUES(1); END;").unwrap();
    Mock::given(wiremock::matchers::body_partial_json(
        json!({"query":"blocker"}),
    ))
    .respond_with(
        ResponseTemplate::new(200)
            .set_delay(Duration::from_secs(30))
            .set_body_json(json!({"data":{}})),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    let before = server.received_requests().await.unwrap().len();
    let mut blockers = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let scoped = client.with_read_context(ReadContext::new(
            ReadClass::Background,
            Duration::from_secs(30),
        ));
        blockers.spawn(async move { scoped.stats_graphql(&json!({"query":"blocker"})).await });
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.received_requests().await.unwrap().len() < before + 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let loaded = std::sync::Arc::new(tokio::sync::Notify::new());
    let entered = loaded.clone();
    let background = client.with_read_context(ReadContext::new(
        ReadClass::Background,
        Duration::from_secs(3),
    ));
    let background_path = path.clone();
    let leader = tokio::spawn(async move {
        fetch_github_step_at(
            background_path,
            &background,
            list,
            Duration::from_secs(3),
            true,
            || {
                entered.notify_one();
                (120, 2000)
            },
        )
        .await
    });
    // This current-thread runtime observes the notification after the source
    // future has synchronously registered its shared scan and yielded.
    loaded.notified().await;
    let foreground = tokio::time::timeout(
        Duration::from_secs(1),
        fetch_github_step_at(path, &client, list, Duration::from_secs(3), false, || {
            (120, 2000)
        }),
    )
    .await
    .expect("foreground source refresh must progress with Background HTTP still held")
    .unwrap();
    let background = leader.await.unwrap().unwrap();
    assert_eq!(foreground.scan, background.scan);
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        4,
        "two blockers, one terminal proof and one promoted discovery"
    );
    let polls = SourcePolls::default();
    let (a, _) = polls.begin_attempt(Source::default(), list).await;
    let (b, _) = polls.begin_attempt(Source::default(), list).await;
    publish_scan_attempt(&polls, &conn, &a, foreground).await;
    publish_scan_attempt(&polls, &conn, &b, background).await;
    let accepted = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    assert_eq!(accepted.revision, baseline + 1);
    assert_eq!(accepted.state.completed_at, Some(2000));
    assert_eq!(terminal_inventory(&conn, list).len(), 4);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM checkpoint_writes", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "shared receipt can publish only one durable checkpoint advancement"
    );
    blockers.abort_all();
    while blockers.join_next().await.is_some() {}
}

#[tokio::test]
async fn stalled_success_body_retains_shared_receipt_and_durable_continuation() {
    stalled_body_retains_shared_receipt_and_durable_continuation("200 OK").await;
}

#[tokio::test]
async fn stalled_error_body_retains_shared_receipt_and_durable_continuation() {
    stalled_body_retains_shared_receipt_and_durable_continuation("403 Forbidden").await;
}

async fn stalled_body_retains_shared_receipt_and_durable_continuation(status: &'static str) {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stalled = Arc::new(tokio::sync::Notify::new());
    let signal = stalled.clone();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let server = tokio::spawn(async move {
        let mut held_bodies = vec![];
        for n in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            let body_start = loop {
                let mut buf = [0; 4096];
                let got = stream.read(&mut buf).await.unwrap();
                assert!(got > 0);
                request.extend_from_slice(&buf[..got]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + len {
                        break end + 4;
                    }
                }
            };
            observed
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&request[body_start..]).unwrap());
            if n == 2 {
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 999999\r\nConnection: close\r\n\r\n{{").as_bytes()).await.unwrap();
                held_bodies.push(stream);
                signal.notify_one();
                continue;
            }
            let body = if n == 0 {
                json!({"data":{"viewer":{"login":"synthetic-viewer"}}})
            } else {
                let numbers = if n == 1 { 1..=25 } else { 26..=50 };
                let nodes: Vec<_> = numbers.map(|n| node(n, false)).collect();
                json!({"data":{"viewer":{"login":"synthetic-viewer"},"authored":{"issueCount":50,"nodes":nodes,"pageInfo":{"hasNextPage":n == 1,"endCursor": if n == 1 {Some("next-page")} else {None}}}}})
            }.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        }
    });
    let client = GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(format!("http://{addr}"))
            .unwrap()
            .personal_token("synthetic")
            .build()
            .unwrap(),
    );
    client.fetch_viewer().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("body-stall.sqlite");
    let conn = crate::store::open_db(&path).unwrap();
    let list = CachedList::Reviewing;
    let spawn = || {
        let path = path.clone();
        let client = client.clone();
        tokio::spawn(async move {
            fetch_github_step_at(path, &client, list, Duration::from_secs(60), false, || {
                (120, 1000)
            })
            .await
        })
    };
    let first = spawn();
    stalled.notified().await;
    // Join after headers have arrived, while the second response is incomplete.
    let loaded = Arc::new(tokio::sync::Notify::new());
    let entered = loaded.clone();
    let joined_client = client.clone();
    let joined_path = path.clone();
    let joined = tokio::spawn(async move {
        fetch_github_step_at(
            joined_path,
            &joined_client,
            list,
            Duration::from_secs(60),
            false,
            || {
                entered.notify_one();
                (120, 1000)
            },
        )
        .await
    });
    loaded.notified().await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let completed_at_cap = first.is_finished() && joined.is_finished();
    tokio::time::resume();
    assert!(
        completed_at_cap,
        "whole-body timeout must finish within the execution cap"
    );
    let first = first
        .await
        .unwrap()
        .expect("body timeout must retain the received first page");
    let joined = joined.await.unwrap().unwrap();
    assert_eq!(first.prs.len(), 25);
    assert_eq!(first.scan, joined.scan);
    let receipt = first.scan.as_ref().unwrap();
    assert_eq!(receipt.state.after.as_deref(), Some("next-page"));
    assert!(receipt.state.received);
    assert!(!receipt.state.done);
    assert!(receipt.state.step_failure.as_ref().unwrap().transient);
    assert!(!receipt.state.step_failure.as_ref().unwrap().not_asked);
    assert_eq!(
        requests.lock().unwrap().len(),
        3,
        "viewer plus two scan attempts; no retry after cap"
    );
    let polls = SourcePolls::default();
    let (a, _) = polls.begin_attempt(Source::default(), list).await;
    let (b, _) = polls.begin_attempt(Source::default(), list).await;
    publish_scan_attempt(&polls, &conn, &a, first).await;
    publish_scan_attempt(&polls, &conn, &b, joined).await;
    let checkpoint = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    assert_eq!(checkpoint.revision, 1, "one accepted shared receipt");
    assert_eq!(checkpoint.state.after.as_deref(), Some("next-page"));
    assert_eq!(saved(&conn).len(), 25);
    let continued =
        fetch_github_step_at(path, &client, list, Duration::from_secs(60), true, || {
            (120, 2000)
        })
        .await
        .unwrap();
    assert_eq!(continued.prs.len(), 25);
    assert!(continued.scan.as_ref().unwrap().state.done);
    let (next, _) = polls.begin_attempt(Source::default(), list).await;
    publish_scan_attempt(&polls, &conn, &next, continued).await;
    assert_eq!(saved(&conn).len(), 50);
    let checkpoint = queue_scan::load(&conn, &Source::default(), list, "synthetic-viewer").unwrap();
    assert_eq!(checkpoint.revision, 2);
    assert_eq!(checkpoint.state.total, Some(50));
    assert!(
        !checkpoint.state.coverage_valid,
        "the interrupted pass cannot claim complete coverage"
    );
    assert_eq!(checkpoint.state.completed_total, None);
    assert_eq!(
        requests.lock().unwrap()[3]["variables"]["after"],
        "next-page"
    );
    server.await.unwrap();
}

// A detail's queue fact must reach both owned inventories before another scan.
#[tokio::test]
async fn detail_fact_publication_is_durable_without_a_local_review() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("detail-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let recorder = Arc::new(
        crate::measurement::Recorder::new(crate::measurement::Config {
            directory: dir.path().canonicalize().unwrap().join("measurement"),
            epoch: [45; 16],
            role: crate::measurement::Role::Desktop,
            platform: crate::measurement::Platform::Macos,
            build: "synthetic".into(),
        })
        .unwrap(),
    );
    recorder.set_enabled(true);
    let mut polls = SourcePolls::default();
    polls.7.recorder = Some(recorder.clone());
    let mut raw = node(1, false);
    raw["isInMergeQueue"] = json!(false);
    raw["updatedAt"] = json!("2026-10-01T10:00:00Z");
    let rows =
        crate::github::map::map_list(&json!({"authored":{"nodes":[raw.clone()]}}), "authored");
    for list in [CachedList::Authored, CachedList::Reviewing] {
        source_cache::save_owned_source_snapshot(
            &conn,
            &Source::default(),
            list,
            &rows,
            &Coverage::Complete,
            Some("synthetic-viewer"),
        )
        .unwrap();
        polls.begin_attempt(Source::default(), list).await;
    }
    raw["state"] = json!("OPEN");
    raw["isInMergeQueue"] = json!(true);
    raw["mergeQueueEntry"] = json!({"state":"QUEUED"});
    raw["updatedAt"] = json!("2026-10-01T11:00:00Z");
    let detail = crate::github::map::map_detail(
        &json!({"repository":{"pullRequest":raw}}),
        "octocat/repo-1",
    );
    let mut publications = Vec::new();
    crate::queue_measurement::run(
        Some(&recorder),
        crate::measurement::OperationClass::Detail,
        async {
            record_github_effect_at(
                path.clone(),
                &polls,
                "octocat/repo-1",
                1,
                "synthetic-viewer",
                GithubEffect::ReviewReadback(Box::new(detail), [0, 0]),
                |event| publications.push(event.unwrap()),
            )
            .await;
            Ok::<_, ()>(())
        },
    )
    .await
    .unwrap();
    assert_eq!(
        publications.len(),
        2,
        "detail knowledge must publish to both lists"
    );
    let receipt_ids: Vec<_> = publications
        .iter()
        .map(|update| {
            update
                .measurement_receipt
                .clone()
                .expect("actual accepted row token")
        })
        .collect();
    assert!(receipt_ids.iter().all(|id| recorder.is_live_receipt(id)));
    let export = dir.path().canonicalize().unwrap().join("report.jsonl");
    recorder.export_to(export.clone()).await.unwrap();
    let events: Vec<serde_json::Value> = std::fs::read_to_string(export)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter_map(|row| row.get("event").cloned())
        .collect();
    let started = events
        .iter()
        .find(|event| event["stage"] == "started")
        .unwrap();
    let accepted: Vec<_> = events
        .iter()
        .filter(|event| event["kind"] == "queue_receipt")
        .collect();
    assert_eq!(accepted.len(), 2);
    assert!(accepted
        .iter()
        .all(|event| event["operation"] == started["operation"]));
    assert!(events
        .iter()
        .any(|event| event["stage"] == "published" && event["operation"] == started["operation"]));
    assert!(events
        .iter()
        .any(|event| event["stage"] == "completed" && event["outcome"] == "success"));
    for update in publications {
        assert!(update.prs.unwrap()[0].in_merge_queue);
    }
    drop(conn);
    let restarted = crate::store::open_db(&path).unwrap();
    assert!(saved(&restarted)[0].in_merge_queue);
}

#[tokio::test]
async fn concurrent_distinct_pr_actions_both_publish_facts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("batch-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let rows = crate::github::map::map_list(
        &json!({"authored":{"nodes":[node(1,false),node(2,false)]}}),
        "authored",
    );
    for list in [CachedList::Authored, CachedList::Reviewing] {
        source_cache::save_owned_source_snapshot(
            &conn,
            &Source::default(),
            list,
            &rows,
            &Coverage::Complete,
            Some("synthetic-viewer"),
        )
        .unwrap();
        polls.begin_attempt(Source::default(), list).await;
    }
    // Both commands captured the same list generation before either completed.
    for n in 1..=2 {
        let mut raw = node(n, false);
        raw["isDraft"] = json!(true);
        let mut observation =
            crate::store::github_facts::Observation::from_node(&raw, chrono::Utc::now()).unwrap();
        observation
            .facts
            .retain(|f| matches!(f.value, crate::store::github_facts::Value::Draft(_)));
        record_github_effect_at(
            path.clone(),
            &polls,
            &format!("octocat/repo-{n}"),
            n as u64,
            "synthetic-viewer",
            GithubEffect::Facts(observation),
            |event| assert!(event.is_ok()),
        )
        .await;
    }
    assert!(
        saved(&conn).iter().all(|row| row.is_draft),
        "both independently acknowledged writes must survive"
    );
}

#[tokio::test]
async fn dequeue_uses_schema_id_and_publishes_verified_inverse_fact() {
    use crate::github::mutate::{ConfirmedAction, PrAction};
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request:&wiremock::Request|{
        let body:Value=request.body_json().unwrap();let query=body["query"].as_str().unwrap();
        if !query.contains("mutation(") {return ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"synthetic-viewer"}}}));}
        let mut row=node(1,false);row["state"]=json!("OPEN");row["updatedAt"]=json!("2026-10-01T11:00:00Z");
        let (field,payload)=if query.contains("dequeuePullRequest") {
            if !query.contains("input: { id: $id }") { return ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"Unknown argument pullRequestId; required id missing"}]})); }
            row["isInMergeQueue"]=json!(false);("dequeuePullRequest",json!({"mergeQueueEntry":{"state":"QUEUED","pullRequest":row}}))
        } else if query.contains("enqueuePullRequest") {
            row["isInMergeQueue"]=json!(true);row["mergeQueueEntry"]=json!({"state":"QUEUED"});("enqueuePullRequest",json!({"mergeQueueEntry":{"state":"QUEUED","pullRequest":row}}))
        } else if query.contains("convertPullRequestToDraft") {row["isDraft"]=json!(true);("convertPullRequestToDraft",json!({"pullRequest":row}))}
        else {row["isDraft"]=json!(false);("markPullRequestReadyForReview",json!({"pullRequest":row}))};
        ResponseTemplate::new(200).set_body_json(json!({"data":{field:payload}}))
    }).mount(&server).await;
    let client = github_client(&server);
    client.fetch_viewer().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mutations.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut raw = node(1, false);
    raw["isDraft"] = json!(false);
    raw["isInMergeQueue"] = json!(false);
    let rows = crate::github::map::map_list(&json!({"authored":{"nodes":[raw]}}), "authored");
    for list in [CachedList::Authored, CachedList::Reviewing] {
        source_cache::save_owned_source_snapshot(
            &conn,
            &Source::default(),
            list,
            &rows,
            &Coverage::Complete,
            Some("synthetic-viewer"),
        )
        .unwrap();
        polls.begin_attempt(Source::default(), list).await;
    }
    for (action, queued, draft) in [
        (PrAction::Enqueue, true, false),
        (PrAction::Dequeue, false, false),
        (PrAction::ConvertToDraft, false, true),
        (PrAction::MarkReady, false, false),
    ] {
        let operation = polls.fact_operation().await;
        let Some(ConfirmedAction::Facts {
            viewer,
            mut observation,
        }) = client.mutate_pr("PR_1", action).await.unwrap()
        else {
            panic!("semantic action proof missing");
        };
        for fact in &mut observation.facts {
            fact.operation = Some(operation.clone());
        }
        let mut frames = Vec::new();
        record_github_effect_at(
            path.clone(),
            &polls,
            "octocat/repo-1",
            1,
            &viewer,
            GithubEffect::Facts(observation),
            |event| frames.push(event.unwrap()),
        )
        .await;
        assert_eq!(frames.len(), 2);
        for frame in frames {
            let row = &frame.prs.unwrap()[0];
            assert_eq!(row.in_merge_queue, queued);
            assert_eq!(row.is_draft, draft);
        }
        let restarted = crate::store::open_db(&path).unwrap();
        let row = &saved(&restarted)[0];
        assert_eq!((row.in_merge_queue, row.is_draft), (queued, draft));
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        5,
        "four writes plus ownership read, no read solely for propagation"
    );
}

#[tokio::test]
async fn newer_detail_head_replaces_old_head_without_borrowing_old_checks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("head-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let rows =
        crate::github::map::map_list(&json!({"authored":{"nodes":[node(1,false)]}}), "authored");
    for list in [CachedList::Authored, CachedList::Reviewing] {
        source_cache::save_owned_source_snapshot(
            &conn,
            &Source::default(),
            list,
            &rows,
            &Coverage::Complete,
            Some("synthetic-viewer"),
        )
        .unwrap();
        polls.begin_attempt(Source::default(), list).await;
    }
    let mut raw = node(1, true);
    raw["state"] = json!("OPEN");
    raw["updatedAt"] = json!("2026-10-01T12:00:00Z");
    raw["isInMergeQueue"] = json!(true);
    raw["mergeQueueEntry"] = json!({"state":"QUEUED"});
    let mut detail = crate::github::map::map_detail(
        &json!({"repository":{"pullRequest":raw}}),
        "octocat/repo-1",
    );
    for fact in &mut detail.inventory_facts.as_mut().unwrap().facts {
        fact.operation = Some(polls.fact_operation().await);
    }
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::ReviewReadback(Box::new(detail), [0, 0]),
        |event| assert!(event.is_ok()),
    )
    .await;
    let row = &saved(&conn)[0];
    assert_eq!(row.head_oid, "head-1-new");
    assert!(row.in_merge_queue);
    assert!(row
        .observation
        .as_ref()
        .unwrap()
        .unknown_fields
        .contains(&crate::inventory::ReadinessField::Ci));
}

#[tokio::test]
async fn replaced_source_client_rejects_held_old_owner_fact_completion() {
    let (server, client) = stable_queue(1).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owner-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    scan_and_publish(&polls, &conn, &client, 1000).await;
    let mut raw = node(1, false);
    raw["isDraft"] = json!(true);
    let mut proof =
        crate::store::github_facts::Observation::from_node(&raw, chrono::Utc::now()).unwrap();
    for fact in &mut proof.facts {
        fact.operation = Some(polls.fact_operation().await);
    }
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(proof.clone()),
        |event| assert!(event.is_ok()),
    )
    .await;
    assert!(saved(&conn)[0].is_draft);
    // A held request captures its operation before the new verified client owns the source.
    for fact in &mut proof.facts {
        fact.operation = Some(polls.fact_operation().await);
    }
    server.reset().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
        "viewer":{"login":"replacement-viewer"}, "authored":{"issueCount":1,"nodes":[node(1,false)],"pageInfo":{"hasNextPage":false,"endCursor":null}}
    }}))).mount(&server).await;
    let replacement = github_client(&server);
    assert_eq!(
        replacement.fetch_viewer().await.unwrap(),
        "replacement-viewer"
    );
    let result = replacement
        .advance_scan(
            CachedList::Reviewing,
            queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "replacement-viewer",
            )
            .unwrap(),
            &[],
            1200,
        )
        .await
        .unwrap();
    publish(&polls, &conn, Ok(result)).await;
    let before = saved(&conn);
    assert!(!before[0].is_draft);
    let revision = polls
        .get(&Source::default(), CachedList::Reviewing)
        .receipt_revision;
    let mut emitted = 0;
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(proof),
        |_| emitted += 1,
    )
    .await;
    assert_eq!(emitted, 0);
    assert_eq!(saved(&conn), before);
    assert_eq!(
        polls
            .get(&Source::default(), CachedList::Reviewing)
            .receipt_revision,
        revision
    );
    assert_eq!(
        source_cache::snapshot_owner(&conn, &Source::default(), CachedList::Reviewing)
            .unwrap()
            .as_deref(),
        Some("replacement-viewer")
    );
    drop(conn);
    assert_eq!(saved(&crate::store::open_db(&path).unwrap()), before);
}

#[tokio::test]
async fn terminal_detail_then_newer_positive_list_reopens_without_stale_resurrection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reopen-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut raw = node(1, false);
    raw["state"] = json!("OPEN");
    raw["updatedAt"] = json!("2026-10-01T10:00:00Z");
    let receipt = |raw: Value| FetchedList {
        scan: None,
        viewer: Some("synthetic-viewer".into()),
        prs: crate::github::map::map_list(&json!({"authored":{"nodes":[raw]}}), "authored"),
        total: Some(1),
        coverage: Coverage::Complete,
    };
    publish(&polls, &conn, Ok(receipt(raw.clone()))).await;
    let mut terminal = raw.clone();
    terminal["state"] = json!("CLOSED");
    terminal["updatedAt"] = json!("2026-10-01T11:00:00Z");
    let proof =
        crate::store::github_facts::Observation::from_node(&terminal, chrono::Utc::now()).unwrap();
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(proof),
        |event| assert!(event.is_ok()),
    )
    .await;
    assert!(saved(&conn).is_empty());
    drop(conn);
    let conn = crate::store::open_db(&path).unwrap();
    let stale = reconcile_github_snapshot(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        receipt(raw.clone()),
        None,
    )
    .unwrap_or_else(|f| panic!("{}", f.message));
    assert!(stale.prs.is_empty());
    let mut reopened = raw.clone();
    reopened["updatedAt"] = json!("2026-10-01T12:00:00Z");
    let fresh = reconcile_github_snapshot(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        receipt(reopened),
        None,
    )
    .unwrap_or_else(|f| panic!("{}", f.message));
    assert_eq!(fresh.prs.len(), 1);
    let late = reconcile_github_snapshot(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        receipt(raw),
        None,
    )
    .unwrap_or_else(|f| panic!("{}", f.message));
    assert_eq!(
        late.prs.len(),
        1,
        "a newer acknowledged reopen supersedes the terminal floor"
    );
    assert_eq!(saved(&conn), late.prs);
}

#[tokio::test]
async fn targeted_fact_publications_keep_field_freshness_and_membership() {
    use crate::inventory::{ObservationState, ReadinessField};
    let mut frames = serde_json::Map::new();
    for case in ["clean", "unrelated", "new_head"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh-target.db");
        let conn = crate::store::open_db(&path).unwrap();
        let polls = SourcePolls::default();
        let mut raw = node(1, false);
        raw["state"] = json!("OPEN");
        raw["mergeStateStatus"] = json!("CLEAN");
        raw["updatedAt"] = json!("2026-10-01T10:00:00Z");
        let mut rows =
            crate::github::map::map_list(&json!({"authored":{"nodes":[raw.clone()]}}), "authored");
        let observation = rows[0].observation.as_mut().unwrap();
        observation.unknown_fields.clear();
        observation.retained_fields.clear();
        observation.ready_at_state = None;
        observation.last_observed_at = Some("2026-10-01T12:00:00Z".parse().unwrap());
        if case == "unrelated" {
            observation.state = ObservationState::Retained;
            observation.retained_fields = vec![ReadinessField::Ci, ReadinessField::Draft];
            observation.unknown_fields = vec![ReadinessField::Merge, ReadinessField::Draft];
        }
        let before_observation = rows[0].observation.clone().unwrap();
        publish(
            &polls,
            &conn,
            Ok(FetchedList {
                scan: None,
                viewer: Some("synthetic-viewer".into()),
                prs: rows,
                total: Some(1),
                coverage: Coverage::Complete,
            }),
        )
        .await;
        let received = polls
            .get(&Source::default(), CachedList::Reviewing)
            .last_received_at;
        let snapshot_before =
            source_cache::load_source_snapshot(&conn, &Source::default(), CachedList::Reviewing)
                .unwrap();
        raw["updatedAt"] = json!("2026-10-01T11:00:00Z");
        if case == "new_head" {
            raw["headRefOid"] = json!("head-1-new");
        }
        let detail = crate::github::map::map_detail(
            &json!({"repository":{"pullRequest":raw.clone()}}),
            "octocat/repo-1",
        );
        let mut proof = detail.inventory_facts.clone().unwrap();
        if case == "unrelated" {
            proof
                .facts
                .retain(|f| matches!(f.value, crate::store::github_facts::Value::Draft(_)));
        }
        let mut published = None;
        record_github_effect_at(
            path,
            &polls,
            "octocat/repo-1",
            1,
            "synthetic-viewer",
            GithubEffect::Facts(proof),
            |event| published = Some(event.unwrap()),
        )
        .await;
        let update = published.unwrap();
        assert_eq!(update.status.last_received_at, received);
        let row = &update.prs.as_ref().unwrap()[0];
        let observation = row.observation.as_ref().unwrap();
        assert_eq!(observation.state, before_observation.state);
        assert_eq!(
            observation.last_observed_at,
            before_observation.last_observed_at
        );
        assert!(
            !observation.retained_fields.contains(&ReadinessField::Draft),
            "fresh accepted draft fact is not last-known"
        );
        assert!(!observation.unknown_fields.contains(&ReadinessField::Draft));
        if case == "clean" {
            assert!(observation.retained_fields.is_empty());
            assert!(observation.unknown_fields.is_empty());
        }
        if case == "unrelated" {
            assert_eq!(observation.retained_fields, vec![ReadinessField::Ci]);
            assert_eq!(observation.unknown_fields, vec![ReadinessField::Merge]);
        }
        if case == "new_head" {
            assert!(observation.unknown_fields.contains(&ReadinessField::Ci));
            assert!(observation.unknown_fields.contains(&ReadinessField::Merge));
        }
        let snapshot_after =
            source_cache::load_source_snapshot(&conn, &Source::default(), CachedList::Reviewing)
                .unwrap();
        if let (
            SnapshotData::Available {
                fetched_at: before, ..
            },
            SnapshotData::Available {
                fetched_at: after, ..
            },
        ) = (snapshot_before.data, snapshot_after.data)
        {
            assert_eq!(before, after);
        }
        let mut frame = serde_json::to_value(update).unwrap();
        canonical(&mut frame);
        frames.insert(case.into(), frame);
    }
    let terminal_details: Vec<_> = ["CLOSED", "MERGED"]
        .into_iter()
        .map(|state| {
            let mut raw = node(1, false);
            raw["state"] = json!(state);
            crate::github::map::map_detail(
                &json!({"repository":{"pullRequest":raw}}),
                "octocat/repo-1",
            )
        })
        .collect();
    let artifact = json!({"frames":frames,"terminal_details":terminal_details});
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/targeted-fact-publications.json");
    if std::env::var_os("UPDATE_TARGETED_FACT_FIXTURE").is_some() {
        std::fs::write(&fixture, serde_json::to_string(&artifact).unwrap() + "\n").unwrap();
    }
    let expected: Value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    assert_eq!(artifact, expected);
}

#[tokio::test]
async fn equal_version_later_target_head_wins_but_earlier_started_head_cannot_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("equal-head.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut raw = node(1, false);
    raw["state"] = json!("OPEN");
    let rows =
        crate::github::map::map_list(&json!({"authored":{"nodes":[raw.clone()]}}), "authored");
    publish(
        &polls,
        &conn,
        Ok(FetchedList {
            scan: None,
            viewer: Some("synthetic-viewer".into()),
            prs: rows,
            total: Some(1),
            coverage: Coverage::Complete,
        }),
    )
    .await;
    let earlier = polls.fact_operation_for("octocat/repo-1", 1).await;
    let mut newer = raw.clone();
    newer["headRefOid"] = json!("equal-version-new-head");
    let mut proof =
        crate::store::github_facts::Observation::from_node(&newer, chrono::Utc::now()).unwrap();
    proof
        .facts
        .retain(|f| matches!(f.value, crate::store::github_facts::Value::Queue(_)));
    for fact in &mut proof.facts {
        fact.operation = Some(polls.fact_operation_for("octocat/repo-1", 1).await);
    }
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(proof),
        |event| assert!(event.is_ok()),
    )
    .await;
    let fresh = saved(&conn);
    assert_eq!(fresh[0].head_oid, "equal-version-new-head");
    assert!(fresh[0]
        .observation
        .as_ref()
        .unwrap()
        .unknown_fields
        .contains(&crate::inventory::ReadinessField::Ci));
    // The earlier-started response completes last, even with a later wall acquisition.
    let mut old =
        crate::store::github_facts::Observation::from_node(&raw, chrono::Utc::now()).unwrap();
    for fact in &mut old.facts {
        fact.operation = Some(earlier.clone());
    }
    let mut emitted = 0;
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(old),
        |_| emitted += 1,
    )
    .await;
    assert_eq!(emitted, 0);
    assert_eq!(saved(&conn), fresh);
    drop(conn);
    let restarted = crate::store::open_db(&path).unwrap();
    assert_eq!(saved(&restarted), fresh);
    let replay = FetchedList {
        scan: None,
        viewer: Some("synthetic-viewer".into()),
        prs: crate::github::map::map_list(&json!({"authored":{"nodes":[raw]}}), "authored"),
        total: Some(1),
        coverage: Coverage::Complete,
    };
    let replayed = reconcile_github_snapshot(
        &restarted,
        &Source::default(),
        CachedList::Reviewing,
        replay,
        None,
    )
    .unwrap_or_else(|f| panic!("{}", f.message));
    assert_eq!(
        replayed.prs[0].head_oid, "equal-version-new-head",
        "equal-version lagging list cannot undo the targeted head after restart"
    );
}

#[tokio::test]
async fn ordinary_equal_version_new_head_fences_old_detail_started_before_publication() {
    for cached_at_start in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ordinary-head.db");
        let conn = crate::store::open_db(&path).unwrap();
        let polls = SourcePolls::default();
        let mut old_raw = node(1, false);
        old_raw["state"] = json!("OPEN");
        let receipt = |raw: Value| FetchedList {
            scan: None,
            viewer: Some("synthetic-viewer".into()),
            prs: crate::github::map::map_list(&json!({"authored":{"nodes":[raw]}}), "authored"),
            total: Some(1),
            coverage: Coverage::Complete,
        };
        if cached_at_start {
            publish(&polls, &conn, Ok(receipt(old_raw.clone()))).await;
        } else {
            source_cache::save_owned_source_snapshot(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                &receipt(old_raw.clone()).prs,
                &Coverage::Complete,
                Some("synthetic-viewer"),
            )
            .unwrap();
        }
        let held_operation = polls.fact_operation_for("octocat/repo-1", 1).await;
        let mut newer = old_raw.clone();
        newer["headRefOid"] = json!("ordinary-new-head");
        let mut fresh = receipt(newer);
        fresh.prs[0].observation.as_mut().unwrap().last_observed_at = Some(chrono::Utc::now());
        let (attempt, _) = polls
            .begin_attempt(Source::default(), CachedList::Reviewing)
            .await;
        publish_scan_attempt(&polls, &conn, &attempt, fresh)
            .await
            .unwrap();
        let authoritative = saved(&conn);
        assert_eq!(authoritative[0].head_oid, "ordinary-new-head");
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM github_pr_facts", [], |r| r.get(0))
                .unwrap(),
            0,
            "ordinary head has no targeted journal to provide a fence"
        );
        let mut delayed =
            crate::store::github_facts::Observation::from_node(&old_raw, chrono::Utc::now())
                .unwrap();
        for fact in &mut delayed.facts {
            fact.operation = Some(held_operation.clone());
        }
        let before_revision = polls
            .get(&Source::default(), CachedList::Reviewing)
            .receipt_revision;
        let mut emitted = 0;
        record_github_effect_at(
            path.clone(),
            &polls,
            "octocat/repo-1",
            1,
            "synthetic-viewer",
            GithubEffect::Facts(delayed),
            |_| emitted += 1,
        )
        .await;
        assert_eq!(
            emitted, 0,
            "old detail must not publish over a newer ordinary list head; cached={cached_at_start}"
        );
        assert_eq!(saved(&conn), authoritative);
        assert_eq!(
            polls
                .get(&Source::default(), CachedList::Reviewing)
                .receipt_revision,
            before_revision
        );
        assert_eq!(
            polls
                .2
                .lock()
                .unwrap()
                .get(&(Source::default(), CachedList::Reviewing))
                .unwrap()
                .1
                .prs,
            authoritative
        );
        drop(conn);
        assert_eq!(saved(&crate::store::open_db(&path).unwrap()), authoritative);

        // A later request sees the ordinary head. Unrelated publication changes
        // the receipt revision without invalidating this identity's authority.
        let later = polls.fact_operation_for("octocat/repo-1", 1).await;
        let conn = crate::store::open_db(&path).unwrap();
        let mut same_head = old_raw.clone();
        same_head["headRefOid"] = json!("ordinary-new-head");
        let (attempt, _) = polls
            .begin_attempt(Source::default(), CachedList::Reviewing)
            .await;
        publish_scan_attempt(&polls, &conn, &attempt, receipt(same_head))
            .await
            .unwrap();
        assert_ne!(
            polls
                .get(&Source::default(), CachedList::Reviewing)
                .receipt_revision,
            later.receipts_at_start[1]
        );
        let mut next_head = old_raw.clone();
        next_head["headRefOid"] = json!("later-target-head");
        let mut observation =
            crate::store::github_facts::Observation::from_node(&next_head, chrono::Utc::now())
                .unwrap();
        for fact in &mut observation.facts {
            fact.operation = Some(later.clone());
        }
        record_github_effect_at(
            path.clone(),
            &polls,
            "octocat/repo-1",
            1,
            "synthetic-viewer",
            GithubEffect::Facts(observation),
            |_| emitted += 1,
        )
        .await;
        assert_eq!(
            emitted, 1,
            "later targeted head remains representable despite another receipt"
        );
        assert_eq!(saved(&conn)[0].head_oid, "later-target-head");
        drop(conn);
        assert_eq!(
            saved(&crate::store::open_db(&path).unwrap())[0].head_oid,
            "later-target-head"
        );
    }
}

#[tokio::test]
async fn identical_targeted_reads_preserve_repair_progress_and_completed_proof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("same-facts.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut raw = node(1, false);
    raw["state"] = json!("OPEN");
    let receipt = FetchedList {
        scan: None,
        viewer: Some("synthetic-viewer".into()),
        prs: crate::github::map::map_list(&json!({"authored":{"nodes":[raw.clone()]}}), "authored"),
        total: Some(1),
        coverage: Coverage::Complete,
    };
    publish(&polls, &conn, Ok(receipt)).await;
    let original_observation = saved(&conn)[0].observation.clone();
    for done in [false, true] {
        let loaded = crate::queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap();
        let state = crate::queue_scan::State {
            done,
            coverage_valid: done,
            started_at: Some(1000),
            finished_at: done.then_some(1000),
            after: (!done).then_some("repair-cursor".into()),
            eligible_at: if done { 1300 } else { 1015 },
            pass_delay: 300,
            ..Default::default()
        };
        assert!(crate::queue_scan::commit(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
            &crate::queue_scan::Commit {
                expected_revision: loaded.revision,
                state: state.clone(),
                removals: vec![]
            },
            |_| Ok(())
        )
        .unwrap());
        let before = crate::queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap();
        for _ in 0..3 {
            let (held, _) = polls
                .begin_attempt(Source::default(), CachedList::Reviewing)
                .await;
            let operation = polls.fact_operation_for("octocat/repo-1", 1).await;
            let mut observation =
                crate::store::github_facts::Observation::from_node(&raw, chrono::Utc::now())
                    .unwrap();
            for fact in &mut observation.facts {
                fact.operation = Some(operation.clone());
            }
            let mut emitted = 0;
            record_github_effect_at(
                path.clone(),
                &polls,
                "octocat/repo-1",
                1,
                "synthetic-viewer",
                GithubEffect::Facts(observation),
                |_| emitted += 1,
            )
            .await;
            assert_eq!(
                emitted, 1,
                "new acquisition still publishes its field qualification"
            );
            assert!(
                polls.success_publication(&held).await.is_none(),
                "old in-flight source still loses to the new journal acquisition"
            );
            let after = crate::queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "synthetic-viewer",
            )
            .unwrap();
            assert_eq!(
                after.revision, before.revision,
                "same known values cannot invalidate traversal CAS"
            );
            assert_eq!(
                after.state, state,
                "same known values cannot rearm repair or erase completed proof"
            );
            assert_eq!(saved(&conn)[0].observation, original_observation);
            let payload: String = conn
                .query_row(
                    "SELECT payload FROM github_pr_facts WHERE number=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let journal: crate::store::github_facts::Observation =
                serde_json::from_str(&payload).unwrap();
            assert!(journal
                .facts
                .iter()
                .all(|fact| fact.operation.as_ref() == Some(&operation)));
        }
    }
    // A newly observed field is meaningful even when its stored boolean agrees.
    let mut rows = saved(&conn);
    rows[0]
        .observation
        .as_mut()
        .unwrap()
        .unknown_fields
        .push(crate::inventory::ReadinessField::Queue);
    publish(
        &polls,
        &conn,
        Ok(FetchedList {
            scan: None,
            viewer: Some("synthetic-viewer".into()),
            prs: rows,
            total: Some(1),
            coverage: Coverage::Complete,
        }),
    )
    .await;
    let operation = polls.fact_operation_for("octocat/repo-1", 1).await;
    let mut observation =
        crate::store::github_facts::Observation::from_node(&raw, chrono::Utc::now()).unwrap();
    for fact in &mut observation.facts {
        fact.operation = Some(operation.clone());
    }
    record_github_effect_at(
        path.clone(),
        &polls,
        "octocat/repo-1",
        1,
        "synthetic-viewer",
        GithubEffect::Facts(observation),
        |_| {},
    )
    .await;
    let repaired_knowledge = crate::queue_scan::load(
        &conn,
        &Source::default(),
        CachedList::Reviewing,
        "synthetic-viewer",
    )
    .unwrap();
    assert!(repaired_knowledge.state.local_repair_pending);
    assert!(!repaired_knowledge.state.coverage_valid);
    assert!(!saved(&conn)[0]
        .observation
        .as_ref()
        .unwrap()
        .unknown_fields
        .contains(&crate::inventory::ReadinessField::Queue));
    // Terminal repeats do not rearm repair; a genuine reopen does, even
    // before a positive list observation can restore the missing row.
    for (index, state, pending) in [(0, "MERGED", true), (1, "MERGED", false), (2, "OPEN", true)] {
        let loaded = crate::queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap();
        let checkpoint = crate::queue_scan::State {
            done: true,
            coverage_valid: true,
            ..Default::default()
        };
        assert!(crate::queue_scan::commit(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
            &crate::queue_scan::Commit {
                expected_revision: loaded.revision,
                state: checkpoint,
                removals: vec![]
            },
            |_| Ok(())
        )
        .unwrap());
        let mut changed = raw.clone();
        changed["state"] = json!(state);
        changed["updatedAt"] = json!(format!("2099-01-0{}T00:00:00Z", index + 1));
        let operation = polls.fact_operation_for("octocat/repo-1", 1).await;
        let mut observation =
            crate::store::github_facts::Observation::from_node(&changed, chrono::Utc::now())
                .unwrap();
        for fact in &mut observation.facts {
            fact.operation = Some(operation.clone());
        }
        record_github_effect_at(
            path.clone(),
            &polls,
            "octocat/repo-1",
            1,
            "synthetic-viewer",
            GithubEffect::Facts(observation),
            |_| {},
        )
        .await;
        let loaded = crate::queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap();
        assert_eq!(
            loaded.state.local_repair_pending, pending,
            "{state} acquisition {index}"
        );
        assert!(
            saved(&conn).is_empty(),
            "a detail cannot invent list membership"
        );
    }
}
