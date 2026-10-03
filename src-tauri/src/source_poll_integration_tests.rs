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
