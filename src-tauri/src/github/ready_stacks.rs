//! Advisory stack metadata for Ready rows; never fetch full PR details.
use super::{client::GitHubClient, model::PrStack, stats::Budget};
use crate::identity::{PrIdentity, Provider};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::Semaphore;

#[derive(Debug, Serialize)]
pub struct RowStack {
    #[serde(flatten)]
    pub identity: PrIdentity,
    pub stack: PrStack,
}

// Shared with paired phones: parallel callers cannot multiply API fan-out.
static LOOKUPS: Semaphore = Semaphore::const_new(4);

pub async fn ready_stacks(
    client: &GitHubClient,
    rows: Vec<PrIdentity>,
) -> Result<Vec<RowStack>, String> {
    let scoped = client.with_read_context(super::admission::ReadContext::new(
        super::admission::ReadClass::Advisory,
        Duration::from_secs(10),
    ));
    let client = &scoped;
    if rows.len() > 8 {
        return Err("Stack metadata accepts at most 8 pull requests per batch".into());
    }
    let budget = client.request_budget();
    let mut seen = HashSet::new();
    let rows = rows.into_iter().filter(|row| seen.insert(row.clone()));
    Ok(stream::iter(rows.map(|identity| {
        let budget = &budget;
        async move {
            let stack = lookup(client, &identity, budget).await;
            RowStack { identity, stack }
        }
    }))
    .buffer_unordered(4)
    .collect()
    .await)
}

async fn lookup(client: &GitHubClient, row: &PrIdentity, budget: &Budget) -> PrStack {
    if row.source.provider != Provider::Github || row.source.host != "github.com" {
        return PrStack::Unknown;
    }
    let Some((owner, repo)) = row.repo.split_once('/') else {
        return PrStack::Unknown;
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') || row.number == 0 {
        return PrStack::Unknown;
    }
    // Decline advisory work under load. Eight rows in two waves, each with
    // a <=1s wait and the stack walk's 10s ceiling, stays below IPC's 30s.
    let Ok(Ok(_permit)) = tokio::time::timeout(Duration::from_secs(1), LOOKUPS.acquire()).await
    else {
        return PrStack::Unknown;
    };
    client
        .fetch_pr_stack_advisory(owner, repo, row.number, budget)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Source;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn row(number: u64) -> PrIdentity {
        PrIdentity {
            source: Source::default(),
            repo: "demo/widgets".into(),
            number,
        }
    }

    async fn client(server: &MockServer) -> GitHubClient {
        GitHubClient::new(
            octocrab::Octocrab::builder()
                .base_uri(server.uri())
                .unwrap()
                .personal_token("test-token")
                .build()
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn reads_off_list_membership_once_for_duplicate_rows_without_detail_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "repository": {"defaultBranchRef": {"name": "main"}, "pullRequest": {
                    "number": 5, "headRefName": "feature", "baseRefName": "main", "stackEntry": {
                        "position": 5, "stack": {"number": 9, "size": 8}
                    }
                }}
            }})))
            .expect(1)
            .mount(&server)
            .await;
        let got = ready_stacks(&client(&server).await, vec![row(5), row(5)])
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(
            got[0].stack,
            PrStack::Stacked {
                position: 5,
                size: 8,
                position_exact: true,
                size_exact: true,
                ..
            }
        ));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body["query"].as_str().unwrap().contains("query PrStack("));
        assert!(!body["query"].as_str().unwrap().contains("reviewThreads"));
    }

    #[tokio::test]
    async fn oversized_batches_are_rejected_without_network_work() {
        let server = MockServer::start().await;
        assert!(
            ready_stacks(&client(&server).await, (1..=9).map(row).collect())
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn foreign_sources_and_invalid_repositories_are_not_sent_to_github() {
        let server = MockServer::start().await;
        let mut gitlab = row(1);
        gitlab.source.provider = Provider::Gitlab;
        let mut enterprise = row(2);
        enterprise.source.host = "github.example".into();
        let mut invalid = row(3);
        invalid.repo = "bad/namespace/repo".into();
        let got = ready_stacks(&client(&server).await, vec![gitlab, enterprise, invalid])
            .await
            .unwrap();
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|r| r.stack == PrStack::Unknown));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    // Explicit current-thread runtimes keep every response inside the quota
    // scope. Real async HTTP reads are exercised without process-wide writes
    // or a mutex held across an await.
    #[test]
    fn a_stack_response_updates_quota_before_the_next_advisory_batch() {
        super::super::stats::budget::scoped::with(520, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let server = MockServer::start().await;
                    Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "rateLimit": {"cost": 21, "remaining": 499},
                "repository": {"defaultBranchRef": {"name": "main"}, "pullRequest": {
                    "number": 5, "headRefName": "feature", "baseRefName": "main", "stackEntry": {
                        "position": 5, "stack": {"number": 9, "size": 8}
                    }
                }}
            }})))
            .expect(1)
            .mount(&server)
            .await;
                    let client = client(&server).await;
                    let first = ready_stacks(&client, vec![row(5)]).await.unwrap();
                    assert!(matches!(first[0].stack, PrStack::Stacked { .. }));
                    let second = ready_stacks(&client, vec![row(6)]).await.unwrap();
                    assert_eq!(second[0].stack, PrStack::Unknown);
                    assert_eq!(server.received_requests().await.unwrap().len(), 1);
                    assert_eq!(super::super::stats::budget::observed_remaining(), Some(499));
                });
        });
    }

    #[test]
    fn advisory_walk_stops_at_the_reserve_and_keeps_partial_membership() {
        super::super::stats::budget::scoped::with(520, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
        use wiremock::matchers::body_string_contains;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(body_string_contains("query PrStack("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "rateLimit": {"cost": 5, "remaining": 515},
                "repository": {"defaultBranchRef": {"name": "main"}, "pullRequest": {
                    "number": 5, "headRefName": "feature", "baseRefName": "base", "stackEntry": null,
                    "baseRef": {"associatedPullRequests": {"nodes": [{"number": 4, "baseRefName": "main"}]}}
                }}
            }}))).expect(1).mount(&server).await;
        Mock::given(method("POST"))
            .and(body_string_contains("query PrStackUp("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "rateLimit": {"cost": 16, "remaining": 499},
                "repository": {"pullRequests": {"nodes": [{"number": 6, "headRefName": "next"}]}}
            }})))
            .expect(1)
            .mount(&server)
            .await;
        let got = ready_stacks(&client(&server).await, vec![row(5)])
            .await
            .unwrap();
        assert!(matches!(
            got[0].stack,
            PrStack::Stacked {
                position: 2,
                size: 3,
                position_exact: true,
                size_exact: false,
                ..
            }
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_eq!(super::super::stats::budget::observed_remaining(), Some(499));
                });
        });
    }

    #[test]
    fn explicit_detail_stack_reads_are_metered_without_the_advisory_reserve_gate() {
        super::super::stats::budget::scoped::with(499, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let server = MockServer::start().await;
                    Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "rateLimit": {"cost": 1, "remaining": 498},
                "repository": {"defaultBranchRef": {"name": "main"}, "pullRequest": {
                    "number": 5, "headRefName": "feature", "baseRefName": "main", "stackEntry": {
                        "position": 5, "stack": {"number": 9, "size": 8}
                    }
                }}
            }})))
            .expect(1)
            .mount(&server)
            .await;
                    let stack = client(&server)
                        .await
                        .fetch_pr_stack("demo", "widgets", 5)
                        .await;
                    assert!(matches!(stack, PrStack::Stacked { .. }));
                    assert_eq!(super::super::stats::budget::observed_remaining(), Some(498));
                });
        });
    }
}
