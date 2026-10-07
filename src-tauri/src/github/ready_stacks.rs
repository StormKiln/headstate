//! Advisory stack metadata for Ready rows; never fetch full PR details.
use super::{client::GitHubClient, model::PrStack};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisory_progress: Option<super::advisory::Progress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known_stack: Option<super::advisory::LastKnown<PrStack>>,
    #[serde(flatten)]
    pub identity: PrIdentity,
    pub stack: PrStack,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_for_ms: Option<u64>,
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
    ready_stacks_with_demand(client, rows, false).await
}

/// Only the selected action prerequisite may use foreground admission. Keep
/// one exact subject and eight actual attempts (including retries), with the
/// same ten-second deadline, cache slots, continuations and account semaphore.
pub async fn ready_stacks_with_demand(
    client: &GitHubClient,
    rows: Vec<StackAsk>,
    selected: bool,
) -> Result<Vec<RowStack>, String> {
    if selected
        && (rows.len() != 1
            || rows.iter().any(|row| {
                row.head_oid.as_ref().is_none_or(|head| head.is_empty())
                    || row.base_ref.as_ref().is_none_or(|base| base.is_empty())
            }))
    {
        return Err("Selected stack metadata requires one exact head and base".into());
    }
    if rows.len() > 8 {
        return Err("Stack metadata accepts at most 8 pull requests per batch".into());
    }
    let client = client.with_read_context(super::admission::ReadContext::new(
        if selected {
            super::admission::ReadClass::Foreground
        } else {
            super::admission::ReadClass::Advisory
        },
        Duration::from_secs(10),
    ));
    let client = if selected {
        client.with_attempt_limit(8)
    } else {
        client
    };
    check_principal(&client)?;
    let mut seen = HashSet::new();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|row| seen.insert(row.clone()))
        .collect();
    let mut out: Vec<_> = rows
        .iter()
        .map(|row| RowStack {
            advisory_progress: None,
            last_known_stack: None,
            identity: row.identity.clone(),
            stack: PrStack::Unknown,
            head_oid: None,
            base_ref: None,
            valid_for_ms: None,
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
    let admitted: Vec<_> = rows
        .iter()
        .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .collect();
    let mut work = stream::iter(
        order
            .into_iter()
            .map(|i| {
                let flag = admitted[i].clone();
                let client = client.with_first_attempt(move || {
                    flag.store(true, std::sync::atomic::Ordering::Release)
                });
                let row = rows[i].clone();
                async move { (i, lookup(&client, &row).await) }
            })
            .collect::<Vec<_>>(),
    )
    .buffer_unordered(2);
    while let Ok(Some((i, (stack, lifetime)))) =
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
            out[i].valid_for_ms = lifetime;
            out[i].head_oid = rows[i].head_oid.clone();
            out[i].base_ref = rows[i].base_ref.clone();
        }
        out[i].stack = stack;
    }
    // Release canceled loaders before nonblocking retained-history lookups.
    drop(work);
    for ((answer, row), admitted) in out.iter_mut().zip(&rows).zip(&admitted) {
        if let Some((head, base)) = row.head_oid.as_ref().zip(row.base_ref.as_ref()) {
            answer.last_known_stack = client.advisory.stacks.last_success(&(
                row.identity.repo.clone(),
                row.identity.number,
                head.clone(),
                base.clone(),
            ));
            // Matching identity qualifies the display receipt only. Unknown
            // and no valid_for_ms can never become fresh action evidence.
            if answer.last_known_stack.is_some() {
                answer.head_oid = Some(head.clone());
                answer.base_ref = Some(base.clone());
            }
        }
        use super::advisory::{Progress, ProgressOutcome};
        let admitted = admitted.load(std::sync::atomic::Ordering::Acquire);
        let pending = row
            .head_oid
            .as_ref()
            .zip(row.base_ref.as_ref())
            .is_some_and(|(head, base)| {
                client.advisory.stack_continuations.pending(&(
                    row.identity.repo.clone(),
                    row.identity.number,
                    head.clone(),
                    base.clone(),
                ))
            });
        let eligible = row.identity.source.provider == Provider::Github
            && row.identity.source.host == "github.com"
            && row.identity.number > 0
            && row
                .identity
                .repo
                .split_once('/')
                .is_some_and(|(owner, repo)| {
                    !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
                })
            && !row.head_oid.as_ref().is_some_and(|s| s.is_empty())
            && !row.base_ref.as_ref().is_some_and(|s| s.is_empty());
        answer.advisory_progress = Some(Progress {
            admitted,
            outcome: if !eligible {
                ProgressOutcome::Ineligible
            } else if pending {
                ProgressOutcome::Partial
            } else if admitted || answer.stack != PrStack::Unknown {
                ProgressOutcome::Offered
            } else {
                ProgressOutcome::Deferred
            },
        });
    }
    check_principal(&client)?;
    Ok(out)
}

fn check_principal(client: &GitHubClient) -> Result<(), String> {
    if let Some(caller) = client.read_context().advisory_context.as_ref() {
        drop(caller.guard()?);
    }
    Ok(())
}

async fn lookup(client: &GitHubClient, ask: &StackAsk) -> (PrStack, Option<u64>) {
    let row = &ask.identity;
    if row.source.provider != Provider::Github || row.source.host != "github.com" {
        return (PrStack::Unknown, None);
    }
    let Some((owner, repo)) = row.repo.split_once('/') else {
        return (PrStack::Unknown, None);
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') || row.number == 0 {
        return (PrStack::Unknown, None);
    }
    if ask.head_oid.as_ref().is_some_and(|head| head.is_empty())
        || ask.base_ref.as_ref().is_some_and(|base| base.is_empty())
    {
        return (PrStack::Unknown, None);
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
            .load_measurement(
                (row.repo.clone(), row.number, head.into(), base.into()),
                client.read_context().deadline,
                |value| {
                    // A locally deferred strip receipt must not suppress the
                    // selected opportunity. Known evidence still singleflights.
                    (client.read_context().class != super::admission::ReadClass::Foreground
                        || *value != PrStack::Unknown)
                        && !client.advisory.stack_continuations.pending(&(
                            row.repo.clone(),
                            row.number,
                            head.into(),
                            base.into(),
                        ))
                },
                work,
            )
            .await
            .map(|(stack, ttl)| (stack, Some(ttl.as_millis() as u64)))
            .unwrap_or((PrStack::Unknown, None))
    } else {
        (work.await.value, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::advisory::SUCCESS_TTL;
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
                .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
                .build()
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn singleton_stack_resumes_verified_downward_work_after_shared_allowance_refusal() {
        assert_stack_resume(31, 1, 9, 27_000, 29_000, false).await;
    }

    #[tokio::test]
    async fn resumed_stack_with_less_than_failure_ttl_retains_success_and_original_age() {
        assert_stack_resume(57, 1, 9, 1_000, 3_000, false).await;
    }

    #[tokio::test]
    async fn expired_stack_prerequisite_is_retrieved_again_before_authority_returns() {
        assert_stack_resume(61, 2, 10, 58_000, 60_000, false).await;
    }

    #[tokio::test]
    async fn selected_continuation_recovers_in_the_exhausted_advisory_cycle() {
        assert_stack_resume(0, 1, 9, 59_000, 60_000, true).await;
    }

    #[tokio::test]
    async fn selected_expired_continuation_reacquires_before_authority_returns() {
        assert_stack_resume(61, 2, 10, 58_000, 60_000, true).await;
    }

    async fn assert_stack_resume(
        advance_secs: u64,
        downward_count: usize,
        total_count: usize,
        min_ttl: u64,
        max_ttl: u64,
        selected: bool,
    ) {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            let data = if query.contains("query PrStackUp") {
                json!({"repository":{"pullRequests":{"nodes":[]}}})
            } else if query.contains("query PrStack(") {
                json!({"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{
                    "number":5,"headRefOid":"head-5","headRefName":"feature-5","baseRefName":"main","stackEntry":null
                }}})
            } else { json!({"synthetic":true}) };
            ResponseTemplate::new(200).set_body_json(json!({"data":data}))
        }).mount(&server).await;
        let client = client(&server).await;
        let advisory = client.with_read_context(super::super::admission::ReadContext::new(
            super::super::admission::ReadClass::Advisory,
            Duration::from_secs(10),
        ));
        for _ in 0..7 {
            advisory
                .stats_graphql(&json!({"query":"synthetic competing advisory"}))
                .await
                .unwrap();
        }
        let mut ask = row(5);
        ask.head_oid = Some("head-5".into());
        ask.base_ref = Some("main".into());
        let partial = ready_stacks(&client, vec![ask.clone()]).await.unwrap();
        assert_eq!(partial[0].stack, PrStack::Unknown);
        assert_eq!(
            serde_json::to_value(&partial[0]).unwrap()["advisory_progress"],
            json!({"outcome":"partial", "admitted":true})
        );
        assert_eq!(partial[0].valid_for_ms, None);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            8,
            "real eighth attempt only retrieves the downward prerequisite"
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(advance_secs)).await;
        tokio::time::resume();
        let completed = ready_stacks_with_demand(&client, vec![ask.clone()], selected)
            .await
            .unwrap();
        assert_eq!(completed[0].stack, PrStack::None);
        assert_eq!(
            serde_json::to_value(&completed[0]).unwrap()["advisory_progress"],
            json!({"outcome":"offered", "admitted":true})
        );
        let requests = server.received_requests().await.unwrap();
        let downward = requests
            .iter()
            .filter(|r| {
                serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["query"]
                    .as_str()
                    .unwrap()
                    .contains("query PrStack(")
            })
            .count();
        assert_eq!(
            downward, downward_count,
            "verified unexpired prerequisite must survive later-stage admission refusal"
        );
        assert_eq!(
            requests.len(),
            total_count,
            "resumption retrieves only missing or expired documents"
        );
        assert!(
            completed[0]
                .valid_for_ms
                .is_some_and(|ttl| ttl > min_ttl && ttl <= max_ttl),
            "aggregate authority retains the original downward deadline"
        );
        let known = completed[0]
            .last_known_stack
            .as_ref()
            .expect("short remaining success must remain display history");
        let expected_age = if downward_count == 1 {
            advance_secs * 1000
        } else {
            0
        };
        assert!(known.age_ms >= expected_age && known.age_ms < expected_age + 1000);
        let cached = ready_stacks(&client, vec![ask]).await.unwrap();
        assert_eq!(
            serde_json::to_value(&cached[0]).unwrap()["advisory_progress"],
            json!({"outcome":"offered", "admitted":false})
        );
    }

    #[tokio::test]
    async fn resumed_multihop_walk_keeps_original_authority_and_total_hop_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            let data = if query.contains("query PrStackUp") {
                let base = body["variables"]["base"].as_str().unwrap();
                let child = base.strip_prefix("feature-").unwrap().parse::<u64>().unwrap() + 1;
                json!({"repository":{"pullRequests":{"nodes":[{"number":child,"headRefName":format!("feature-{child}"),"isCrossRepository":false}]}}})
            } else if query.contains("query PrStack(") {
                json!({"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{"number":5,"headRefOid":"head-5","headRefName":"feature-5","baseRefName":"main","stackEntry":null}}})
            } else { json!({}) };
            ResponseTemplate::new(200).set_body_json(json!({"data":data}))
        }).mount(&server).await;
        let client = client(&server).await;
        let advisory = client.with_read_context(super::super::admission::ReadContext::new(
            super::super::admission::ReadClass::Advisory,
            Duration::from_secs(10),
        ));
        for _ in 0..7 {
            advisory
                .stats_graphql(&json!({"query":"synthetic competing advisory"}))
                .await
                .unwrap();
        }
        let mut ask = row(5);
        ask.head_oid = Some("head-5".into());
        ask.base_ref = Some("main".into());
        assert_eq!(
            ready_stacks(&client, vec![ask.clone()]).await.unwrap()[0].stack,
            PrStack::Unknown
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::time::resume();
        let got = ready_stacks(&client, vec![ask]).await.unwrap();
        assert!(matches!(
            got[0].stack,
            PrStack::Stacked {
                size: 4,
                size_exact: false,
                ..
            }
        ));
        assert!(got[0]
            .valid_for_ms
            .is_some_and(|ttl| ttl > 28_000 && ttl <= 29_000));
        assert_eq!(
            got[0].advisory_progress.unwrap().outcome,
            super::super::advisory::ProgressOutcome::Offered
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            11,
            "seven competitors + one retained downward + three upward hops"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| String::from_utf8_lossy(&r.body).contains("query PrStack("))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn command_deadline_releases_partial_loader_before_reading_retained_history() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["query"].as_str().unwrap().contains("query PrStackUp") {
                ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"pullRequests":{"nodes":[]}}}})).set_delay(Duration::from_secs(60))
            } else if body["query"].as_str().unwrap().contains("query PrStack(") {
                ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{
                    "number":5,"headRefOid":"head-5","headRefName":"feature-5","baseRefName":"base-4","stackEntry":null,
                    "baseRef":{"associatedPullRequests":{"nodes":[{"number":4,"baseRefName":"main"}]}}
                }}}}))
            } else { ResponseTemplate::new(200).set_body_json(json!({"data":{}})) }
        }).mount(&server).await;
        let client = client(&server).await;
        let advisory = client.with_read_context(super::super::admission::ReadContext::new(
            super::super::admission::ReadClass::Advisory,
            Duration::from_secs(10),
        ));
        for _ in 0..7 {
            advisory
                .stats_graphql(&json!({"query":"synthetic competing advisory"}))
                .await
                .unwrap();
        }
        let mut ask = row(5);
        ask.head_oid = Some("head-5".into());
        ask.base_ref = Some("base-4".into());
        let partial = ready_stacks(&client, vec![ask.clone()]).await.unwrap();
        assert!(matches!(
            partial[0].stack,
            PrStack::Stacked {
                size_exact: false,
                ..
            }
        ));
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::time::resume();
        let short = client.with_read_context(super::super::admission::ReadContext::new(
            super::super::admission::ReadClass::Advisory,
            Duration::from_millis(30),
        ));
        let timed_out = ready_stacks(&short, vec![ask]).await.unwrap();
        // The inner request deadline may settle its qualified floor before
        // the outer stream timeout wins. Both preserve only original authority.
        assert!(
            timed_out[0].stack == PrStack::Unknown
                || matches!(
                    timed_out[0].stack,
                    PrStack::Stacked {
                        size_exact: false,
                        ..
                    }
                )
        );
        assert!(timed_out[0].valid_for_ms.is_none_or(|ttl| ttl <= 29_000));
        assert!(
            timed_out[0].last_known_stack.is_some(),
            "outer deadline must release its cache-loader guard before looking up retained history"
        );
        let progress = timed_out[0].advisory_progress.unwrap();
        assert!(progress.admitted);
        // An outer cancellation retains a continuation; a completed inner
        // transport-timeout error settles the qualified floor as offered.
        assert!(matches!(
            progress.outcome,
            super::super::advisory::ProgressOutcome::Partial
                | super::super::advisory::ProgressOutcome::Offered
        ));
    }

    #[tokio::test]
    async fn canceled_stack_loader_retains_only_completed_identity_checked_stage() {
        assert_canceled_stack_loader(false).await;
    }

    #[tokio::test]
    async fn canceled_selected_loader_retains_only_completed_identity_checked_stage() {
        assert_canceled_stack_loader(true).await;
    }

    async fn assert_canceled_stack_loader(selected: bool) {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let server = MockServer::start().await;
        let upward = Arc::new(AtomicUsize::new(0));
        let requests = upward.clone();
        Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["query"].as_str().unwrap().contains("query PrStackUp") {
                let response = ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"pullRequests":{"nodes":[]}}}}));
                if requests.fetch_add(1, Ordering::SeqCst) == 0 { response.set_delay(Duration::from_secs(60)) } else { response }
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{
                    "number":5,"headRefOid":"head-5","headRefName":"feature-5","baseRefName":"main","stackEntry":null
                }}}}))
            }
        }).mount(&server).await;
        let client = client(&server).await;
        let mut ask = row(5);
        ask.head_oid = Some("head-5".into());
        ask.base_ref = Some("main".into());
        let worker_client = client.clone();
        let worker_ask = ask.clone();
        let worker = tokio::spawn(async move {
            ready_stacks_with_demand(&worker_client, vec![worker_ask], selected).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while upward.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        assert!(client.advisory.stack_continuations.pending(&(
            "demo/widgets".into(),
            5,
            "head-5".into(),
            "main".into()
        )));
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::time::resume();
        let completed = ready_stacks_with_demand(&client, vec![ask], selected)
            .await
            .unwrap();
        assert_eq!(completed[0].stack, PrStack::None);
        assert!(completed[0]
            .valid_for_ms
            .is_some_and(|ttl| ttl > 27_000 && ttl <= 29_000));
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            3,
            "one downward, canceled upward, resumed upward"
        );
    }

    #[tokio::test]
    async fn selected_recovers_exact_evidence_after_150_rows_exhaust_advisory() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let number = body["variables"]["number"].as_u64().unwrap_or(5);
                ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{
                "defaultBranchRef":{"name":"main"},"pullRequest":{"number":number,
                "headRefOid":"selected-head","headRefName":"feature","baseRefName":"main",
                "stackEntry":{"position":1,"stack":{"number":9,"size":1}}}}}}))
            })
            .mount(&server)
            .await;
        let client = client(&server).await;
        for number in 1..=150 {
            ready_stacks(&client, vec![row(number)]).await.unwrap();
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 8);
        let mut ask = row(151);
        ask.head_oid = Some("selected-head".into());
        ask.base_ref = Some("main".into());
        assert_eq!(
            ready_stacks(&client, vec![ask.clone()]).await.unwrap()[0].stack,
            PrStack::Unknown
        );
        let recovered = ready_stacks_with_demand(&client, vec![ask.clone()], true)
            .await
            .unwrap();
        assert_ne!(
            recovered[0].stack,
            PrStack::Unknown,
            "selected action prerequisite recovers in the same exhausted cycle"
        );
        assert_eq!(recovered[0].head_oid, ask.head_oid);
        assert_eq!(recovered[0].base_ref, ask.base_ref);
        assert!(recovered[0].valid_for_ms.is_some_and(|ms| ms > 59_000));
        assert_eq!(server.received_requests().await.unwrap().len(), 9);
        ready_stacks_with_demand(&client, vec![ask.clone()], true)
            .await
            .unwrap();
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            9,
            "fresh selected evidence reuses the shared slot"
        );
        let other = self::client(&server).await;
        ready_stacks_with_demand(&other, vec![ask.clone()], true)
            .await
            .unwrap();
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            10,
            "another account cannot reuse this evidence"
        );
        for changed in [
            StackAsk {
                head_oid: Some("changed".into()),
                ..ask.clone()
            },
            StackAsk {
                base_ref: Some("changed".into()),
                ..ask.clone()
            },
        ] {
            let answer = ready_stacks_with_demand(&client, vec![changed], true)
                .await
                .unwrap();
            assert_eq!(answer[0].stack, PrStack::Unknown);
            assert_eq!(answer[0].valid_for_ms, None);
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        assert!(ready_stacks_with_demand(&client, vec![ask], true)
            .await
            .unwrap()[0]
            .valid_for_ms
            .is_some());
        assert_eq!(server.received_requests().await.unwrap().len(), 13);
    }

    #[tokio::test]
    async fn selected_requires_one_exact_subject_and_retired_principal_cannot_read_cache() {
        let server = MockServer::start().await;
        let client = client(&server).await;
        for asks in [vec![], vec![row(1)], vec![row(1), row(2)]] {
            assert!(ready_stacks_with_demand(&client, asks, true).await.is_err());
        }
        let caller = crate::remote::context::DispatchContext::paired_for_test();
        caller.retire_for_test();
        let client = client.with_advisory_context(caller);
        let ask = StackAsk {
            head_oid: Some("head".into()),
            base_ref: Some("main".into()),
            ..row(1)
        };
        assert!(ready_stacks_with_demand(&client, vec![ask], true)
            .await
            .is_err());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn selected_and_strip_share_one_exact_receipt_without_extending_its_lifetime() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)
            .set_delay(Duration::from_millis(10))
            .set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{
                "number":5,"headRefOid":"head-5","headRefName":"feature","baseRefName":"main",
                "stackEntry":{"position":1,"stack":{"number":9,"size":1}}
            }}}}))).expect(1).mount(&server).await;
        let client = client(&server).await;
        let ask = StackAsk {
            head_oid: Some("head-5".into()),
            base_ref: Some("main".into()),
            ..row(5)
        };
        let (strip, selected) = tokio::join!(
            ready_stacks(&client, vec![ask.clone()]),
            ready_stacks_with_demand(&client, vec![ask.clone()], true)
        );
        let strip = strip.unwrap();
        let selected = selected.unwrap();
        assert_eq!(strip[0].stack, selected[0].stack);
        assert_ne!(selected[0].stack, PrStack::Unknown);
        assert!(selected[0].valid_for_ms <= strip[0].valid_for_ms);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn selected_respects_primary_and_secondary_provider_cooldowns() {
        for secondary in [false, true] {
            let server = MockServer::start().await;
            let response = if secondary {
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "120")
                    .set_body_json(json!({"message":"secondary rate limit"}))
            } else {
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .set_body_json(json!({"message":"primary quota"}))
            };
            Mock::given(method("POST"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let client = client(&server).await;
            let ask = StackAsk {
                head_oid: Some("head".into()),
                base_ref: Some("main".into()),
                ..row(1)
            };
            for _ in 0..2 {
                let answer = ready_stacks_with_demand(&client, vec![ask.clone()], true)
                    .await
                    .unwrap();
                assert_eq!(answer[0].stack, PrStack::Unknown);
                assert_eq!(answer[0].valid_for_ms, None);
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
            let snapshot = client.admission_snapshot();
            assert!(if secondary {
                snapshot.secondary_cooldown_ms > 119_000
            } else {
                snapshot.graphql.primary_cooldown_ms > 59_000
            });
        }
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
        assert!(got[0]
            .valid_for_ms
            .is_some_and(|ms| ms > 59_000 && ms <= 60_000));
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(55)).await;
        let aged = ready_stacks(&client, vec![ask]).await.unwrap();
        assert_eq!(aged[0].stack, got[0].stack);
        assert!(aged[0]
            .valid_for_ms
            .is_some_and(|ms| ms > 4_000 && ms <= 5_000));
        tokio::time::resume();
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
    async fn explicitly_empty_identity_facts_are_ineligible_without_provider_work() {
        let server = MockServer::start().await;
        let mut head = row(1);
        head.head_oid = Some(String::new());
        let mut base = row(2);
        base.base_ref = Some(String::new());
        let got = ready_stacks(&client(&server).await, vec![head, base])
            .await
            .unwrap();
        assert!(got.iter().all(|r| r.advisory_progress
            == Some(super::super::advisory::Progress {
                outcome: super::super::advisory::ProgressOutcome::Ineligible,
                admitted: false,
            })));
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
