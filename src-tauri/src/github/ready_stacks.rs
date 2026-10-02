//! Advisory stack metadata for Ready rows; never fetch full PR details.
use super::{
    advisory::{FAILURE_TTL, SUCCESS_TTL},
    client::GitHubClient,
    model::PrStack,
};
use crate::identity::{PrIdentity, Provider};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct StackAsk {
    #[serde(flatten)]
    pub identity: PrIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct RowStack {
    #[serde(flatten)]
    pub identity: PrIdentity,
    pub stack: PrStack,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}

impl GitHubClient {
    /// Task6: local-only lookup; never waits for an in-flight loader or fetches.
    pub fn peek_advisory_stack(
        &self,
        repo: &str,
        number: u64,
        head: &str,
        base: &str,
    ) -> Option<PrStack> {
        self.advisory
            .stacks
            .peek(&(repo.into(), number, head.into(), base.into()))
    }
}

pub async fn ready_stacks(
    client: &GitHubClient,
    rows: Vec<StackAsk>,
) -> Result<Vec<RowStack>, String> {
    if rows.len() > 8 {
        return Err("Stack metadata accepts at most 8 pull requests per batch".into());
    }
    let client = client.with_read_context(super::admission::ReadContext::new(
        super::admission::ReadClass::Advisory,
        Duration::from_secs(10),
    ));
    let mut seen = HashSet::new();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|row| seen.insert(row.clone()))
        .collect();
    let mut out: Vec<_> = rows
        .iter()
        .map(|row| RowStack {
            identity: row.identity.clone(),
            stack: PrStack::Unknown,
            head_oid: None,
            base_ref: None,
        })
        .collect();
    let mut canonical: Vec<_> = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("string-only stack identity"))
        .collect();
    canonical.sort();
    let group = serde_json::to_string(&canonical).expect("string-only stack group");
    // Rotate WITHIN a repeated batch, independently of other owners/groups.
    // Otherwise competing advisory work can always leave its final rows out.
    let mut order: Vec<_> = (0..rows.len()).collect();
    order.sort_by_key(|i| serde_json::to_string(&rows[*i]).expect("string-only stack identity"));
    let shift = client.advisory.rotation(group, order.len());
    order.rotate_left(shift);
    let mut work = stream::iter(
        order
            .into_iter()
            .map(|i| {
                let client = client.clone();
                let row = rows[i].clone();
                async move { (i, lookup(&client, &row).await) }
            })
            .collect::<Vec<_>>(),
    )
    .buffer_unordered(2);
    while let Ok(Some((i, stack))) =
        tokio::time::timeout_at(client.read_context().deadline, work.next()).await
    {
        // Unknown is not verified matching evidence. Legacy asks cannot acquire
        // a head/base receipt just because the current provider read succeeded.
        if stack != PrStack::Unknown
            && rows[i]
                .head_oid
                .as_ref()
                .is_some_and(|head| !head.is_empty())
            && rows[i]
                .base_ref
                .as_ref()
                .is_some_and(|base| !base.is_empty())
        {
            out[i].head_oid = rows[i].head_oid.clone();
            out[i].base_ref = rows[i].base_ref.clone();
        }
        out[i].stack = stack;
    }
    Ok(out)
}

async fn lookup(client: &GitHubClient, ask: &StackAsk) -> PrStack {
    let row = &ask.identity;
    if row.source.provider != Provider::Github || row.source.host != "github.com" {
        return PrStack::Unknown;
    }
    let Some((owner, repo)) = row.repo.split_once('/') else {
        return PrStack::Unknown;
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') || row.number == 0 {
        return PrStack::Unknown;
    }
    let budget = client.request_budget();
    let expected = ask
        .head_oid
        .as_deref()
        .zip(ask.base_ref.as_deref())
        .filter(|(h, b)| !h.is_empty() && !b.is_empty());
    let work = client.fetch_pr_stack_advisory(owner, repo, row.number, &budget, expected);
    if let Some((head, base)) = expected {
        client
            .advisory
            .stacks
            .load(
                (row.repo.clone(), row.number, head.into(), base.into()),
                client.read_context().deadline,
                |s| {
                    if *s == PrStack::Unknown {
                        FAILURE_TTL
                    } else {
                        SUCCESS_TTL
                    }
                },
                work,
            )
            .await
            .unwrap_or(PrStack::Unknown)
    } else {
        work.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Source;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn row(number: u64) -> StackAsk {
        StackAsk {
            identity: PrIdentity {
                source: Source::default(),
                repo: "demo/widgets".into(),
                number,
            },
            head_oid: None,
            base_ref: None,
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
                    "number": 5, "headRefOid": "task4-head", "headRefName": "feature", "baseRefName": "main", "stackEntry": {
                        "position": 5, "stack": {"number": 9, "size": 8}
                    }
                }}
            }})))
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server).await;
        let mut ask = row(5);
        ask.head_oid = Some("task4-head".into());
        ask.base_ref = Some("main".into());
        let got = ready_stacks(&client, vec![ask.clone(), ask.clone()])
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
        ready_stacks(&client, vec![ask]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body["query"].as_str().unwrap().contains("query PrStack("));
        assert!(!body["query"].as_str().unwrap().contains("reviewThreads"));
    }

    #[tokio::test]
    async fn repeated_starved_batches_rotate_later_stack_rows_into_service() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let Some(number) = body["variables"]["number"].as_u64() else { return ResponseTemplate::new(200).set_body_json(json!({"data":{"synthetic":true}})); };
            ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{"number":number,"headRefName":"feature","baseRefName":"main","stackEntry":{"position":1,"stack":{"number":1,"size":1}}}}}}))
        }).mount(&server).await;
        let client = client(&server).await;
        let mut served = HashSet::new();
        for _ in 0..8 {
            let advisory = client.with_read_context(super::super::admission::ReadContext::new(
                super::super::admission::ReadClass::Advisory,
                Duration::from_secs(10),
            ));
            for _ in 0..6 {
                advisory
                    .stats_graphql(&json!({"query":"synthetic competing advisory"}))
                    .await
                    .unwrap();
            }
            for answer in ready_stacks(&client, (1..=8).map(row).collect())
                .await
                .unwrap()
            {
                if answer.stack != PrStack::Unknown {
                    served.insert(answer.identity.number);
                }
            }
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::resume();
        }
        assert_eq!(
            served.len(),
            8,
            "even with six competing attempts every cycle, each stack row gets a turn"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 64);
    }

    #[tokio::test]
    async fn only_matching_semantic_receipts_are_reused_or_peeked() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{"number":5,"headRefOid":"head-a","headRefName":"feature","baseRefName":"main","stackEntry":{"position":1,"stack":{"number":9,"size":1}}}}}}))).mount(&server).await;
        let client = client(&server).await;
        assert_eq!(
            client.peek_advisory_stack("demo/widgets", 5, "head-a", "main"),
            None
        );
        ready_stacks(&client, vec![row(5)]).await.unwrap();
        assert_eq!(
            client.peek_advisory_stack("demo/widgets", 5, "head-a", "main"),
            None,
            "legacy identity-only read cannot certify a keyed receipt"
        );
        let mut ask = row(5);
        ask.head_oid = Some("head-a".into());
        ask.base_ref = Some("main".into());
        let first = ready_stacks(&client, vec![ask.clone()]).await.unwrap();
        assert!(matches!(
            first[0].stack,
            PrStack::Stacked {
                native: true,
                size: 1,
                ..
            }
        ));
        assert_eq!(first[0].head_oid.as_deref(), Some("head-a"));
        assert_eq!(
            client.peek_advisory_stack("demo/widgets", 5, "head-a", "main"),
            Some(first[0].stack.clone())
        );
        ready_stacks(&client, vec![ask.clone()]).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        ask.base_ref = Some("changed-base".into());
        let changed = ready_stacks(&client, vec![ask]).await.unwrap();
        assert_eq!(changed[0].stack, PrStack::Unknown);
        assert_eq!(changed[0].head_oid, None);
        tokio::time::pause();
        tokio::time::advance(SUCCESS_TTL).await;
        assert_eq!(
            client.peek_advisory_stack("demo/widgets", 5, "head-a", "main"),
            None
        );
        tokio::time::resume();
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
        gitlab.identity.source.provider = Provider::Gitlab;
        let mut enterprise = row(2);
        enterprise.identity.source.host = "github.example".into();
        let mut invalid = row(3);
        invalid.identity.repo = "bad/namespace/repo".into();
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
                    "number": 5, "headRefOid": "task4-head", "headRefName": "feature", "baseRefName": "main", "stackEntry": {
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
                    "number": 5, "headRefOid": "task4-head", "headRefName": "feature", "baseRefName": "main", "stackEntry": {
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
