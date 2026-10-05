//! Real finite scan → publication → SQLite regression for scoped failures.
use super::*;
use crate::{
    github::{
        admission::{ReadClass, ReadContext},
        client::GitHubClient,
        model::{CiState, MergeState, PullRequest, ReviewState},
    },
    inventory::ObservationState,
    queue_scan,
    store::source_cache::{self, SnapshotData},
};
use serde_json::{json, Value};
use std::time::Duration;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

fn node(number: usize) -> Value {
    let seed: Value = serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
    let mut row = seed["authored"]["nodes"][0].clone();
    let repo = format!("synthetic/repo-{}", (number - 1) % 51 + 1);
    row["id"] = json!(format!("PR_{number}"));
    row["number"] = json!(number);
    row["repository"]["nameWithOwner"] = json!(repo);
    row["url"] = json!(format!("https://github.com/{repo}/pull/{number}"));
    row["headRefOid"] = json!(format!("head-{number}"));
    row["isDraft"] = json!(number > 130);
    row["reviewDecision"] = Value::Null;
    row["latestReviews"] = json!({"nodes":[],"totalCount":0});
    row
}
fn saved(conn: &rusqlite::Connection) -> Vec<PullRequest> {
    match source_cache::load_source_snapshot(conn, &Source::default(), CachedList::Reviewing)
        .unwrap()
        .data
    {
        SnapshotData::Available { prs, .. } => prs,
        _ => vec![],
    }
}
fn ready_count(rows: &[PullRequest]) -> usize {
    rows.iter()
        .filter(|row| {
            !row.is_draft
                && matches!(row.ci, CiState::Success | CiState::None)
                && row.merge != MergeState::Conflicted
                && !matches!(
                    row.review,
                    ReviewState::Approved | ReviewState::ChangesRequested
                )
                && !row.in_merge_queue
                && row
                    .observation
                    .as_ref()
                    .is_some_and(|o| o.unknown_fields.is_empty())
        })
        .count()
}
async fn step(
    client: &GitHubClient,
    server: &MockServer,
    conn: &rusqlite::Connection,
    polls: &SourcePolls,
    now: i64,
) -> FetchedList {
    let source = Source::default();
    let list = CachedList::Reviewing;
    let before = server.received_requests().await.unwrap().len();
    let result = client
        .with_read_context(ReadContext::new(
            ReadClass::Background,
            Duration::from_secs(10),
        ))
        .advance_scan(
            list,
            queue_scan::load(conn, &source, list, "synthetic-viewer").unwrap(),
            &saved(conn),
            now,
        )
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().len() - before <= 3);
    let (attempt, _) = polls.begin_attempt(source.clone(), list).await;
    let publication = polls.publication(&attempt).await.unwrap();
    let result = reconcile_github_snapshot(conn, &source, list, result, None)
        .unwrap_or_else(|error| panic!("{}", error.message));
    polls.complete(publication, Ok(result.clone()), |status| {
        assert_eq!(saved(conn), polls.update(status, None).prs.unwrap());
    });
    result
}
async fn failure_cycle(optional_head: bool) {
    let server = MockServer::start().await;
    let mode = Arc::new(Mutex::new("healthy"));
    let served = mode.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let mode = *served.lock().unwrap();
            let start = body["variables"]["after"]
                .as_str()
                .and_then(|s| s.strip_prefix("cursor-"))
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            if mode == "failed" || (mode == "head-failed" && start == 0) {
                return ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-remaining", "5000")
                    .set_body_json(
                        json!({"errors":[{"message":"Synthetic finite page failure"}]}),
                    );
            }
            let end = (start + 25).min(236);
            ResponseTemplate::new(200).insert_header("x-ratelimit-remaining", "5000")
            .set_body_json(json!({"data":{
                "viewer":{"login":"synthetic-viewer"},
                "authored":{"issueCount":236,"nodes":(start+1..=end).map(node).collect::<Vec<_>>(),
                "pageInfo":{"hasNextPage":end<236,"endCursor":format!("cursor-{end}")}}
            }}))
        })
        .mount(&server)
        .await;
    let client = GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("synthetic-token")
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .build()
            .unwrap(),
    );
    client.fetch_viewer().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scoped-failure.db");
    let conn = crate::store::open_db(&path).unwrap();
    let polls = SourcePolls::default();
    let mut now = 1000;
    for _ in 0..5 {
        step(&client, &server, &conn, &polls, now).await;
        now += 120;
    }
    assert_eq!(saved(&conn).len(), 236);
    assert_eq!(ready_count(&saved(&conn)), 130);
    assert!(saved(&conn)
        .iter()
        .all(|r| r.observation.as_ref().unwrap().state == ObservationState::Observed));
    assert!(
        queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer"
        )
        .unwrap()
        .state
        .done
    );

    // Repeat after recovery/convergence: a second cycle must not lose evidence.
    for _ in 0..2 {
        *mode.lock().unwrap() = "healthy";
        step(&client, &server, &conn, &polls, now).await;
        now += 120;
        let previous = saved(&conn);
        let before = queue_scan::load(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "synthetic-viewer",
        )
        .unwrap()
        .state;
        assert_eq!(before.after.as_deref(), Some("cursor-50"));
        *mode.lock().unwrap() = if optional_head {
            "head-failed"
        } else {
            "failed"
        };
        let result = step(&client, &server, &conn, &polls, now).await;
        let after = result.scan.as_ref().unwrap();
        assert!(after.state.step_failure.is_some());
        assert!(polls
            .get(&Source::default(), CachedList::Reviewing)
            .error
            .is_some());
        assert_eq!(after.state.candidates, before.candidates);
        if optional_head {
            assert_eq!(after.state.after.as_deref(), Some("cursor-100"));
            assert_eq!(after.state.seen.len(), 100);
        } else {
            assert_eq!(after.state.after, before.after);
            assert_eq!(after.state.seen, before.seen);
        }
        let untouched = |number: u64| !optional_head || !(51..=100).contains(&number);
        for old in previous.iter().filter(|row| untouched(row.number)) {
            let actual = result
                .prs
                .iter()
                .find(|row| row.identity() == old.identity())
                .unwrap();
            assert_eq!(
                actual, old,
                "an unrelated row must keep all values and its original observation time"
            );
        }
        assert_eq!(ready_count(&result.prs), 130);
        // Reopen SQLite before recovery: retained provenance is durable.
        let reopened = crate::store::open_db(&path).unwrap();
        assert_eq!(saved(&reopened), result.prs);
        *mode.lock().unwrap() = "healthy";
        now += 120;
        let recovered = step(&client, &server, &conn, &polls, now).await;
        let original_tail = previous.iter().find(|row| row.number == 236).unwrap();
        assert_eq!(
            recovered.prs.iter().find(|row| row.number == 236).unwrap(),
            original_tail
        );
        assert!(polls
            .get(&Source::default(), CachedList::Reviewing)
            .error
            .is_none());
        // Finish this pass before inducing the next cycle.
        for _ in 0..5 {
            if queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "synthetic-viewer",
            )
            .unwrap()
            .state
            .done
            {
                break;
            }
            now += 120;
            step(&client, &server, &conn, &polls, now).await;
        }
        assert!(
            queue_scan::load(
                &conn,
                &Source::default(),
                CachedList::Reviewing,
                "synthetic-viewer"
            )
            .unwrap()
            .state
            .done
        );
        now += 120;
    }
}

#[tokio::test]
async fn failed_finite_step_preserves_unrelated_observation_receipts() {
    failure_cycle(false).await;
}
#[tokio::test]
async fn failed_optional_head_refresh_keeps_successful_tail_receipts() {
    failure_cycle(true).await;
}
