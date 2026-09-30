//! Advisory stack metadata for Ready rows; never fetch full PR details.
use super::{client::GitHubClient, model::PrStack};
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
    if rows.len() > 8 {
        return Err("Stack metadata accepts at most 8 pull requests per batch".into());
    }
    let mut seen = HashSet::new();
    let rows = rows.into_iter().filter(|row| seen.insert(row.clone()));
    Ok(stream::iter(rows.map(|identity| async move {
        let stack = lookup(client, &identity).await;
        RowStack { identity, stack }
    }))
    .buffer_unordered(4)
    .collect()
    .await)
}

async fn lookup(client: &GitHubClient, row: &PrIdentity) -> PrStack {
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
    if !super::stats::Budget::new().permits(8) {
        return PrStack::Unknown;
    }
    client.fetch_pr_stack(owner, repo, row.number).await
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
}
