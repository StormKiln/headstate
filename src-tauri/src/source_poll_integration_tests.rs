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
    let worker = tokio::spawn(crate::poll::run_queue_continuations(
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
                        || (120, now.load(Ordering::SeqCst)),
                    )
                    .await
                    .unwrap();
                    let permit = polls.success_publication(&attempt).await.unwrap();
                    let result = reconcile_github_at(path, &polls, &permit, result).await;
                    polls.complete(permit, result, |_| {});
                    tx.send(list).unwrap();
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
            tokio::time::advance(Duration::from_secs(15)).await;
            // Real sockets and blocking SQLite are allowed to complete. Virtual
            // time drives the timer; it does not stand in for provider completion.
            tokio::time::resume();
            for _ in 0..2 {
                tokio::time::timeout(Duration::from_secs(3), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
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
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
    }
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
