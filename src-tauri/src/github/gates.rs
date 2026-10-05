//! Base-branch review gates: can the viewer's approval count, and must
//! conversations be resolved before merge (#1451, #1454).
//!
//! Some repositories' rules require the most recent push to be approved by
//! someone other than its pusher (`require_last_push_approval`), or every
//! review thread to be resolved before merge
//! (`required_review_thread_resolution`). Headstate fetched neither, so it
//! offered Approve to the one person whose approval could not count, and
//! reported "a required review or check is missing" for a merge that was
//! waiting on conversations.
//!
//! # What the API actually answers (MEASURED 2026-09-25, live `gh api`)
//!
//! - `GET /repos/{o}/{r}/rules/branches/{branch}` is readable WITHOUT admin:
//!   a public repository the measuring account has only `pull` on returned
//!   its `pull_request` rule with `parameters.require_last_push_approval:
//!   true`. `X-RateLimit-Resource: core`.
//! - It returns RULESET rules only. Classic branch protection is invisible
//!   to it, and reading that (`/branches/{b}/protection`) answered 404 to
//!   the same non-admin account. So an EMPTY answer is "no ruleset asks",
//!   never "no rule" -- the UI draws no conclusion from `false`.
//! - One branch can carry SEVERAL `pull_request` rules (repository and
//!   organization rulesets stack; one measured branch had four). GitHub
//!   enforces all of them, so a requirement is on if ANY rule sets it.
//! - A slash in the branch name works unencoded (`rules/branches/release/x`).
//! - `GET /repos/{o}/{r}/activity?ref=refs/heads/{head}` names the pusher as
//!   `actor`. **`activity_type=push` is NOT enough**, contrary to the
//!   issue's sketch: a branch whose only push created it is recorded as
//!   `branch_creation`, and the filtered query returned an EMPTY list for
//!   such a head. So this reads the unfiltered list and takes the newest
//!   entry that moved the ref.
//! - A fork's head lives in the fork, and the activity API answered there.
//!
//! # What this refuses to claim
//!
//! - **Rules unreadable** (403/404, a network failure, a budget refusal):
//!   nothing new is rendered. "We could not ask" is not "no rule".
//! - **Pusher unknown**: the newest ref-moving activity must name the SAME
//!   commit the detail view shows. If it does not -- the activity log lags,
//!   the branch moved since the detail was fetched, the actor was deleted --
//!   the pusher is unknown. The head commit's author or committer is never
//!   used as a stand-in: it only approximates the pusher (a rebase, a
//!   cherry-pick, or pushing someone else's commits all break it), and the
//!   claim this feeds -- "your approval won't count" -- disables a button.

use super::advisory::{FAILURE_TTL, SUCCESS_TTL};
use futures_util::{stream, StreamExt};
use std::time::Duration;

use super::client::GitHubClient;
use super::stats::Budget;

/// What the base branch's rulesets say, or why we do not know.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BaseRules {
    /// The rules were read. `false` means no RULESET requires it; classic
    /// branch protection cannot be seen here, so it is NOT "not required".
    Read {
        require_last_push_approval: bool,
        required_review_thread_resolution: bool,
    },
    /// We did not ask: the REST budget was too low to spend on an
    /// advisory read, or the request could not be formed.
    Declined { reason: String },
    /// We asked and GitHub did not answer usably.
    Unreadable { reason: String },
}

/// Who pushed the pull request's head commit, or why we do not know.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LastPusher {
    /// The activity log names who moved the head ref to the head commit.
    Known { login: String },
    /// Not looked up because no readable rule makes it matter. Distinct
    /// from `Unknown`: nothing was asked, so nothing failed.
    NotNeeded,
    /// We did not ask (budget, or no head repository to ask).
    Declined { reason: String },
    /// We asked and could not tell.
    Unknown { reason: String },
}

/// Both answers the detail view needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewGates {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_valid_for_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pusher_valid_for_ms: Option<u64>,
    pub rules: BaseRules,
    pub last_pusher: LastPusher,
}

/// Rules change rarely -- an admin edits a ruleset -- so one read per
/// (repository, base branch) serves every pull request into that base for
/// this long. Ten minutes bounds how stale a just-edited rule can read
/// while keeping a triage session that opens twenty PRs into `main` at one
/// request rather than twenty.
const RULES_TTL: Duration = Duration::from_secs(600);

/// `(require_last_push_approval, required_review_thread_resolution)` from a
/// `rules/branches` response, or `None` if it is not the documented list.
///
/// ORed across every `pull_request` rule: rulesets stack, and GitHub
/// enforces the strictest. A missing parameter reads as `false` for that
/// one rule -- it cannot turn another rule's `true` off.
pub fn map_rules(v: &serde_json::Value) -> Option<(bool, bool)> {
    let rules = v.as_array()?;
    let mut last_push = false;
    let mut resolution = false;
    for rule in rules {
        if rule["type"].as_str() != Some("pull_request") {
            continue;
        }
        let p = &rule["parameters"];
        last_push |= p["require_last_push_approval"].as_bool() == Some(true);
        resolution |= p["required_review_thread_resolution"].as_bool() == Some(true);
    }
    Some((last_push, resolution))
}

/// The activity types that MOVE a ref to a new commit. Anything else
/// (`branch_deletion`, `pr_merge`, `merge_queue_merge`) says nothing about
/// who pushed the head.
const REF_MOVES: &[&str] = &["push", "force_push", "branch_creation"];

/// Who pushed `head_oid`, from an `activity` response (newest first).
///
/// Takes the NEWEST ref-moving entry and believes it only if its `after`
/// is the head commit the view shows. An older entry naming the same
/// commit is not accepted: if the newest move went elsewhere, the view is
/// stale and the answer belongs to a different head.
pub fn map_last_pusher(v: &serde_json::Value, head_oid: &str) -> LastPusher {
    let Some(entries) = v.as_array() else {
        return LastPusher::Unknown {
            reason: "the activity response was not a list".into(),
        };
    };
    let newest = entries.iter().find(|e| {
        e["activity_type"]
            .as_str()
            .is_some_and(|t| REF_MOVES.contains(&t))
    });
    let Some(entry) = newest else {
        return LastPusher::Unknown {
            reason: "no push to this branch is on record".into(),
        };
    };
    if head_oid.is_empty() || entry["after"].as_str() != Some(head_oid) {
        return LastPusher::Unknown {
            reason: "the latest recorded push is not the head commit shown".into(),
        };
    }
    match entry["actor"]["login"].as_str() {
        Some(login) if !login.is_empty() => LastPusher::Known {
            login: login.to_string(),
        },
        _ => LastPusher::Unknown {
            reason: "the push has no recorded actor".into(),
        },
    }
}

/// `owner/name` with nothing that could escape the path it is spliced into.
fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((o, n)) if ok(o) && ok(n))
}

/// Percent-encode everything outside RFC 3986's unreserved set, keeping
/// `/` -- branch names contain slashes and both endpoints accept them raw.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The base branch's rules, from the cache or one REST read.
pub async fn base_rules(
    client: &GitHubClient,
    budget: &Budget,
    repo: &str,
    base: &str,
) -> BaseRules {
    base_rules_receipt(client, budget, repo, base).await.0
}

async fn base_rules_receipt(
    client: &GitHubClient,
    budget: &Budget,
    repo: &str,
    base: &str,
) -> (BaseRules, Duration) {
    if !valid_repo(repo) || base.is_empty() {
        return (
            BaseRules::Declined {
                reason: "no base branch to ask about".into(),
            },
            Duration::ZERO,
        );
    }
    client
        .advisory
        .rules
        .load_receipt(
            (repo.into(), base.into()),
            client.read_context().deadline,
            |value| {
                if matches!(value, BaseRules::Read { .. }) {
                    RULES_TTL
                } else {
                    FAILURE_TTL
                }
            },
            base_rules_uncached(client, budget, repo, base),
        )
        .await
        .unwrap_or_else(|| {
            (
                BaseRules::Declined {
                    reason: "not checked before the request deadline".into(),
                },
                Duration::ZERO,
            )
        })
}

async fn base_rules_uncached(
    client: &GitHubClient,
    budget: &Budget,
    repo: &str,
    base: &str,
) -> BaseRules {
    if !client.rest_reserve_allows() {
        return BaseRules::Declined {
            reason: "the REST rate-limit budget is nearly spent".into(),
        };
    }
    let path = format!("/repos/{repo}/rules/branches/{}", encode(base));
    match client.rest_get(&path, budget).await {
        Ok(v) => match map_rules(&v) {
            Some((a, b)) => BaseRules::Read {
                require_last_push_approval: a,
                required_review_thread_resolution: b,
            },
            None => BaseRules::Unreadable {
                reason: "GitHub's rules answer was not a list".into(),
            },
        },
        Err(
            super::client::ClientError::NotDispatched(reason)
            | super::client::ClientError::RateLimited(reason),
        ) => BaseRules::Declined { reason },
        Err(e) => BaseRules::Unreadable {
            reason: e.to_string(),
        },
    }
}

/// Who pushed `head_oid` to `head_ref` in `head_repo`, by one REST read.
pub async fn last_pusher(
    client: &GitHubClient,
    budget: &Budget,
    head_repo: Option<&str>,
    head_ref: &str,
    head_oid: &str,
) -> LastPusher {
    last_pusher_receipt(client, budget, head_repo, head_ref, head_oid)
        .await
        .0
}

async fn last_pusher_receipt(
    client: &GitHubClient,
    budget: &Budget,
    head_repo: Option<&str>,
    head_ref: &str,
    head_oid: &str,
) -> (LastPusher, Duration) {
    // No head repository means the fork is gone or the detail has not
    // arrived. Asking the BASE repository instead would be a guess -- a
    // same-named branch there is a different branch.
    let Some(head_repo) = head_repo.filter(|r| valid_repo(r)) else {
        return (
            LastPusher::Declined {
                reason: "the head repository is not known".into(),
            },
            Duration::ZERO,
        );
    };
    if head_ref.is_empty() || head_oid.is_empty() {
        return (
            LastPusher::Declined {
                reason: "the head branch is not known".into(),
            },
            Duration::ZERO,
        );
    }
    client
        .advisory
        .pushers
        .load_receipt(
            (head_repo.into(), head_ref.into(), head_oid.into()),
            client.read_context().deadline,
            |value| {
                if matches!(value, LastPusher::Known { .. }) {
                    SUCCESS_TTL
                } else {
                    FAILURE_TTL
                }
            },
            last_pusher_uncached(client, budget, head_repo, head_ref, head_oid),
        )
        .await
        .unwrap_or_else(|| {
            (
                LastPusher::Declined {
                    reason: "not checked before the request deadline".into(),
                },
                Duration::ZERO,
            )
        })
}

async fn last_pusher_uncached(
    client: &GitHubClient,
    budget: &Budget,
    head_repo: &str,
    head_ref: &str,
    head_oid: &str,
) -> LastPusher {
    if !client.rest_reserve_allows() {
        return LastPusher::Declined {
            reason: "the REST rate-limit budget is nearly spent".into(),
        };
    }
    // Ten, not one: the newest entry can be a non-moving one, and
    // `activity_type=push` would miss a head that was only ever created
    // (see the module docs). Same single request either way.
    let path = format!(
        "/repos/{head_repo}/activity?ref={}&per_page=10",
        encode(&format!("refs/heads/{head_ref}"))
    );
    match client.rest_get(&path, budget).await {
        Ok(v) => map_last_pusher(&v, head_oid),
        Err(
            super::client::ClientError::NotDispatched(reason)
            | super::client::ClientError::RateLimited(reason),
        ) => LastPusher::Declined { reason },
        Err(e) => LastPusher::Unknown {
            reason: e.to_string(),
        },
    }
}

/// Both gates for one pull request. Never fails: every failure is folded
/// into a state that renders nothing new.
///
/// The pusher is asked only when a readable rule makes it matter, so a
/// repository without the rule costs one cached read and nothing more.
///
/// Each stage has its OWN `per_request` ceiling rather than one timeout
/// around both: a pusher lookup that hangs must not take a rules answer
/// that already arrived down with it (partial is not nothing, #1044). The
/// rules stage also caches before it returns, so even its own timeout
/// loses nothing a later open could reuse.
#[allow(clippy::too_many_arguments)]
pub async fn review_gates(
    client: &GitHubClient,
    budget: &Budget,
    repo: &str,
    base: &str,
    head_repo: Option<&str>,
    head_ref: &str,
    head_oid: &str,
    per_request: Duration,
) -> ReviewGates {
    let (rules, rules_lifetime) =
        tokio::time::timeout(per_request, base_rules_receipt(client, budget, repo, base))
            .await
            .unwrap_or_else(|_| {
                (
                    BaseRules::Unreadable {
                        reason: format!("timed out after {}s", per_request.as_secs()),
                    },
                    Duration::ZERO,
                )
            });
    let rules_until = tokio::time::Instant::now() + rules_lifetime;
    let (last_pusher, pusher_lifetime) = match rules {
        BaseRules::Read {
            require_last_push_approval: true,
            ..
        } => tokio::time::timeout(
            per_request,
            last_pusher_receipt(client, budget, head_repo, head_ref, head_oid),
        )
        .await
        .unwrap_or_else(|_| {
            (
                LastPusher::Unknown {
                    reason: format!("timed out after {}s", per_request.as_secs()),
                },
                Duration::ZERO,
            )
        }),
        _ => (LastPusher::NotNeeded, Duration::ZERO),
    };
    // The independent pusher stage consumes the rules receipt's remaining life.
    ReviewGates {
        rules,
        last_pusher,
        rules_valid_for_ms: Some(
            rules_until
                .saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as u64,
        ),
        pusher_valid_for_ms: Some(pusher_lifetime.as_millis() as u64),
    }
}

// ---------------------------------------------------------------------
// The Ready for review strip (#1576)
// ---------------------------------------------------------------------

/// One strip row's question: who pushed its head, and what its base's
/// rules say.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PusherAsk {
    pub repo: String,
    pub number: u64,
    pub base: String,
    /// Where the head lives; `None` for a deleted fork or an old snapshot.
    pub head_repo: Option<String>,
    pub head_ref: String,
    pub head_oid: String,
}

/// One strip row's answer. `head_oid` is echoed so the frontend can drop
/// an answer about a head the row has since moved off.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RowPusher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pusher_valid_for_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_valid_for_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_pusher: Option<super::advisory::LastKnown<LastPusher>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_rules: Option<super::advisory::LastKnown<BaseRules>>,
    pub repo: String,
    pub number: u64,
    pub head_oid: String,
    pub base: String,
    pub head_ref: String,
    pub head_repo: Option<String>,
    pub rules: BaseRules,
    /// Never `NotNeeded` here: the strip's tag wants the pusher whatever
    /// the rules say. `Declined` is "not checked" -- the budget, the
    /// per-refresh cap, or no head repository to ask -- and the strip
    /// renders it as not checked, never as a verdict.
    pub last_pusher: LastPusher,
}

pub async fn strip_pushers(
    client: &GitHubClient,
    budget: &Budget,
    asks: &[PusherAsk],
    per_request: Duration,
) -> Result<Vec<RowPusher>, String> {
    let client = client.with_read_context(super::admission::ReadContext::new(
        super::admission::ReadClass::Advisory,
        Duration::from_secs(10),
    ));
    let keys: Vec<_> = asks
        .iter()
        .map(|a| serde_json::to_string(a).expect("string-only advisory identity"))
        .collect();
    // Membership is logical row identity: reordering and head movement do not
    // reset continuation. Actual value-cache keys still include all head facts.
    let members: Vec<_> = asks
        .iter()
        .map(|a| serde_json::to_string(&(a.repo.to_lowercase(), a.number)).expect("row identity"))
        .collect();
    let batch = client.advisory.batch(&members)?;
    let order = client.advisory.batch_order(&keys, &members, &batch);
    let mut out: Vec<RowPusher> = asks
        .iter()
        .map(|a| RowPusher {
            pusher_valid_for_ms: None,
            rules_valid_for_ms: None,
            last_known_pusher: None,
            last_known_rules: None,
            repo: a.repo.clone(),
            number: a.number,
            head_oid: a.head_oid.clone(),
            base: a.base.clone(),
            head_ref: a.head_ref.clone(),
            head_repo: a.head_repo.clone(),
            rules: BaseRules::Declined {
                reason: "not checked on this refresh".into(),
            },
            last_pusher: LastPusher::Declined {
                reason: "not checked on this refresh".into(),
            },
        })
        .collect();
    let mut work = stream::iter(
        order
            .into_iter()
            .map(|i| {
                let client = client.clone();
                let budget = budget.clone();
                let a = asks[i].clone();
                let key = keys[i].clone();
                let member = members[i].clone();
                let batch = batch.clone();
                async move {
                    let mut context = client.read_context();
                    let advisory = client.advisory.clone();
                    context.first_attempt = Some(super::admission::FirstAttempt::new(move || {
                        advisory.batch_offered(key, member, &batch);
                    }));
                    let client = client.with_read_context(context);
                    let rules = tokio::time::timeout(
                        per_request,
                        base_rules(&client, &budget, &a.repo, &a.base),
                    )
                    .await
                    .unwrap_or_else(|_| BaseRules::Unreadable {
                        reason: "rules lookup timed out".into(),
                    });
                    let pusher = tokio::time::timeout(
                        per_request,
                        last_pusher(
                            &client,
                            &budget,
                            a.head_repo.as_deref(),
                            &a.head_ref,
                            &a.head_oid,
                        ),
                    )
                    .await
                    .unwrap_or_else(|_| LastPusher::Unknown {
                        reason: "pusher lookup timed out".into(),
                    });
                    (i, rules, pusher)
                }
            })
            .collect::<Vec<_>>(),
    )
    .buffer_unordered(2);
    // Keep completed rows when the shared command deadline expires. Dropping
    // this stream cancels its futures; there is no detached batch worker.
    while let Ok(Some((i, rules, pusher))) =
        tokio::time::timeout_at(client.read_context().deadline, work.next()).await
    {
        out[i].rules = rules;
        out[i].last_pusher = pusher;
    }
    for (row, ask) in out.iter_mut().zip(asks) {
        let rules_key = (ask.repo.clone(), ask.base.clone());
        if let Some((rules, lifetime)) = client.advisory.rules.peek_receipt(&rules_key) {
            row.rules = rules;
            row.rules_valid_for_ms = Some(lifetime.as_millis() as u64);
        }
        row.last_known_rules = client.advisory.rules.last_success(&rules_key);
        if let Some(repo) = &ask.head_repo {
            let key = (repo.clone(), ask.head_ref.clone(), ask.head_oid.clone());
            if let Some((pusher, lifetime)) = client.advisory.pushers.peek_receipt(&key) {
                row.last_pusher = pusher;
                row.pusher_valid_for_ms = Some(lifetime.as_millis() as u64);
            }
            row.last_known_pusher = client.advisory.pushers.last_success(&key);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn client_for(server: &MockServer) -> GitHubClient {
        let oc = octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        GitHubClient::new(oc)
    }

    #[tokio::test]
    async fn strip_receipt_preserves_native_remaining_pusher_and_policy_lifetime() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(|request: &wiremock::Request| {
                if request.url.path().contains("/activity") {
                    ResponseTemplate::new(200).set_body_json(
                        json!([{"activity_type":"push","after":HEAD,"actor":{"login":"octocat"}}]),
                    )
                } else {
                    ResponseTemplate::new(200).set_body_json(json!([]))
                }
            })
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let asks = [ask("octocat/hello-world", 1, HEAD)];
        let first = strip_pushers(&client, &client.request_budget(), &asks, T)
            .await
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(55)).await;
        let second = strip_pushers(&client, &client.request_budget(), &asks, T)
            .await
            .unwrap();
        tokio::time::resume();
        let first = serde_json::to_value(&first[0]).unwrap();
        let second = serde_json::to_value(&second[0]).unwrap();
        assert!(first["pusher_valid_for_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 59_000 && ms <= 60_000));
        assert!(second["pusher_valid_for_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 4_000 && ms <= 5_000));
        assert!(second["rules_valid_for_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 544_000 && ms <= 545_000));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rules_receipts_are_owned_by_authenticated_client_and_coalesce() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(30))
                    .set_body_json(json!([])),
            )
            .expect(2)
            .mount(&server)
            .await;
        let a = client_for(&server).await;
        let b = client_for(&server).await;
        let budget = a.request_budget();
        let (one, two) = tokio::join!(
            base_rules(&a, &budget, "octocat/hello-world", "task4-owner"),
            base_rules(&a, &budget, "octocat/hello-world", "task4-owner")
        );
        assert_eq!(one, two);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "same-client requests must coalesce"
        );
        base_rules(&a, &budget, "octocat/hello-world", "task4-owner").await;
        base_rules(
            &b,
            &b.request_budget(),
            "octocat/hello-world",
            "task4-owner",
        )
        .await;
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn two_windows_120_rows_44_bases_share_attempts_and_reach_later_rows() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(|request: &wiremock::Request| {
            if request.url.path().contains("/activity") {
                ResponseTemplate::new(200).set_body_json(json!([{"activity_type":"push", "after":HEAD, "actor":{"login":"octocat"}}]))
            } else if request.url.path().ends_with("base-0") {
                ResponseTemplate::new(403).set_body_json(json!({"message":"synthetic permission refusal"}))
            } else { ResponseTemplate::new(200).set_body_json(json!([])) }
        }).mount(&server).await;
        let client = client_for(&server).await;
        let asks: Vec<_> = (1..=120)
            .map(|i| {
                let mut a = ask("octocat/hello-world", i, HEAD);
                a.base = format!("base-{}", i % 44);
                a
            })
            .collect();
        let mut known = std::collections::HashSet::new();
        let mut prior = 0;
        for _ in 0..40 {
            let budget = client.request_budget();
            let (a, b) = tokio::join!(
                strip_pushers(&client, &budget, &asks, T),
                strip_pushers(&client, &budget, &asks, T)
            );
            for row in a.unwrap().iter().chain(&b.unwrap()) {
                if matches!(row.last_pusher, LastPusher::Known { .. }) {
                    known.insert(row.number);
                }
            }
            let count = server.received_requests().await.unwrap().len();
            assert!(count - prior <= 8, "actual HTTP attempts per shared cycle");
            prior = count;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::resume();
        }
        assert_eq!(
            known.len(),
            120,
            "failure at the prefix must not starve later rows"
        );
    }

    #[tokio::test]
    async fn stack_and_pusher_commands_share_the_same_eight_http_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let number = body["variables"]["number"].as_u64().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{"number":number,"headRefName":"feature","baseRefName":"main","stackEntry":{"position":1,"stack":{"number":1,"size":1}}}}}}))
        }).mount(&server).await;
        let client = client_for(&server).await;
        let asks: Vec<_> = (1..=120)
            .map(|i| ask("octocat/hello-world", i, HEAD))
            .collect();
        let stacks = (1..=8)
            .map(|number| super::super::ready_stacks::StackAsk {
                identity: crate::identity::PrIdentity {
                    source: crate::identity::Source::default(),
                    repo: "octocat/hello-world".into(),
                    number,
                },
                head_oid: None,
                base_ref: None,
            })
            .collect();
        let budget = client.request_budget();
        let (pushers, stacks) = tokio::join!(
            strip_pushers(&client, &budget, &asks, T),
            super::super::ready_stacks::ready_stacks(&client, stacks)
        );
        assert_eq!(pushers.unwrap().len(), 120);
        assert_eq!(stacks.unwrap().len(), 8);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 8);
        assert!(requests.iter().any(|r| r.method == "POST"));
        assert!(requests.iter().any(|r| r.method == "GET"));
    }

    #[tokio::test]
    async fn sustained_mixed_large_owner_and_selected_detail_share_actual_attempts() {
        use super::super::model::PrStack;
        use super::super::ready_stacks::{ready_stacks, StackAsk};
        use std::collections::HashSet;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(|request: &wiremock::Request| {
                if request.url.path().contains("/activity") {
                    ResponseTemplate::new(200).set_body_json(
                        json!([{"activity_type":"push","after":HEAD,"actor":{"login":"octocat"}}]),
                    )
                } else {
                    ResponseTemplate::new(200).set_body_json(json!([]))
                }
            })
            .mount(&server)
            .await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let number = body["variables"]["number"].as_u64();
            if let Some(number) = number {
                // Non-native membership needs a second real HTTP attempt.
                ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},"pullRequest":{"number":number,"headRefOid":HEAD,"headRefName":"feature","baseRefName":"main","stackEntry":null}}}}))
            } else { ResponseTemplate::new(200).set_body_json(json!({"data":{"repository":{"pullRequests":{"nodes":[]}}}})) }
        }).mount(&server).await;
        let client = client_for(&server).await;
        let population: Vec<_> = (1..=275)
            .map(|number| ask(&format!("synthetic/repo-{}", number % 44), number, HEAD))
            .collect();
        let stack_ask = |a: &PusherAsk| StackAsk {
            identity: crate::identity::PrIdentity {
                source: crate::identity::Source::default(),
                repo: a.repo.clone(),
                number: a.number,
            },
            head_oid: Some(a.head_oid.clone()),
            base_ref: Some(a.base.clone()),
        };
        let detail = ask("synthetic/selected", 999, HEAD);
        // A late selected row cannot bypass an already exhausted native
        // period. It receives an opportunity first in the following period.
        let competing = client.with_read_context(super::super::admission::ReadContext::new(
            super::super::admission::ReadClass::Advisory,
            Duration::from_secs(10),
        ));
        for _ in 0..8 {
            competing
                .stats_graphql(&json!({"query":"synthetic competing advisory"}))
                .await
                .unwrap();
        }
        assert_eq!(
            ready_stacks(&client, vec![stack_ask(&detail)])
                .await
                .unwrap()[0]
                .stack,
            PrStack::Unknown
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 8);
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::resume();
        let mut pushers_seen = HashSet::new();
        let mut stacks_seen = HashSet::new();
        let mut attempts = 8;
        let mut detail_opportunities = 0;
        let mut longest_detail_gap = 0;
        let mut detail_gap = 0;
        for cycle in 0..315 {
            let window: Vec<_> = (0..8)
                .map(|offset| population[(cycle * 8 + offset) % population.len()].clone())
                .collect();
            let budget = client.request_budget();
            // Replay the production shared queue: selected ancestry first,
            // then alternate single-row pusher/strip-stack demand. Each command
            // still executes real native fanout under the unchanged meter.
            let selected = ready_stacks(&client, vec![stack_ask(&detail)])
                .await
                .unwrap();
            if selected[0].stack != PrStack::Unknown {
                detail_opportunities += 1;
                detail_gap = 0;
            } else {
                detail_gap += 1;
                longest_detail_gap = longest_detail_gap.max(detail_gap);
            }
            for row in &window {
                for pusher_turn in [cycle % 2 == 0, cycle % 2 != 0] {
                    if pusher_turn {
                        let pushers = strip_pushers(&client, &budget, std::slice::from_ref(row), T)
                            .await
                            .unwrap();
                        pushers_seen.extend(
                            pushers
                                .iter()
                                .filter(|r| matches!(r.last_pusher, LastPusher::Known { .. }))
                                .map(|r| r.number),
                        );
                    } else {
                        let stacks = ready_stacks(&client, vec![stack_ask(row)]).await.unwrap();
                        stacks_seen.extend(
                            stacks
                                .iter()
                                .filter(|r| r.stack != PrStack::Unknown)
                                .map(|r| r.identity.number),
                        );
                    }
                }
            }
            let actual = server.received_requests().await.unwrap().len();
            assert!(
                actual - attempts <= 8,
                "cycle {cycle}: actual native HTTP attempts, including fanout"
            );
            attempts = actual;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::resume();
        }
        assert_eq!(
            pushers_seen.len(),
            275,
            "every pusher gets an opportunity under sustained synthetic demand"
        );
        assert_eq!(
            stacks_seen.len(),
            275,
            "every stack gets an opportunity under sustained synthetic demand"
        );
        assert!(
            longest_detail_gap <= 1,
            "selected detail waited {} cycles",
            longest_detail_gap
        );
        let detail_reads = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).unwrap_or_default();
                body["variables"]["number"].as_u64() == Some(999)
            })
            .count();
        assert!(
            detail_reads <= 158,
            "fresh selected native receipts must be reused"
        );
        println!("mixed: cycles=315 attempts={attempts} selected_downward_reads={detail_reads} pushers={} stacks={} selected_successes={detail_opportunities} longest_detail_gap={longest_detail_gap}", pushers_seen.len(), stacks_seen.len());
    }

    // A completion-only fairness ledger loses both opportunities when the
    // deadline drops the slow prefix. Surviving identities must keep progress
    // even when an unrelated head changes between concurrent refreshes.
    #[tokio::test]
    async fn canceled_slow_prefix_eventually_offers_healthy_survivor() {
        slow_prefix_progress(false, false, false).await;
    }

    #[tokio::test]
    async fn concurrent_canceled_slow_prefix_preserves_survivor_progress_after_churn() {
        slow_prefix_progress(true, true, false).await;
    }

    #[tokio::test]
    async fn concurrent_canceled_slow_prefix_eventually_offers_healthy_survivor() {
        slow_prefix_progress(true, false, false).await;
    }

    #[tokio::test]
    async fn staggered_concurrent_slow_prefix_eventually_offers_healthy_survivor() {
        slow_prefix_progress(true, false, true).await;
    }

    // Unlike a delayed wiremock response, these slow sockets remain blocked
    // regardless of wall-clock scheduling. Dropping the fixture aborts the
    // listener and its JoinSet, closing every held connection.
    struct HeldPusherProvider {
        uri: String,
        paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl HeldPusherProvider {
        async fn start() -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let uri = format!("http://{}", listener.local_addr().unwrap());
            let paths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let received = paths.clone();
            let task = tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut socket, _) = accepted.unwrap();
                            let received = received.clone();
                            connections.spawn(async move {
                                let mut header = Vec::new();
                                let mut buf = [0; 1024];
                                while !header.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                                    let count = socket.read(&mut buf).await.unwrap();
                                    if count == 0 { return; }
                                    header.extend_from_slice(&buf[..count]);
                                }
                                let request = String::from_utf8(header).unwrap();
                                let path = request.split_whitespace().nth(1).unwrap()
                                    .split('?').next().unwrap().to_owned();
                                received.lock().unwrap().push(path.clone());
                                if path.contains("/slow-") {
                                    std::future::pending::<()>().await;
                                }
                                let body = if path.ends_with("/activity") {
                                    json!([push("push", HEAD, "octocat")]).to_string()
                                } else { "[]".to_owned() };
                                let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                                // Cancellation is expected to close some client sockets.
                                let _ = socket.write_all(reply.as_bytes()).await;
                            });
                        }
                        Some(result) = connections.join_next(), if !connections.is_empty() => {
                            result.unwrap();
                        }
                    }
                }
            });
            Self { uri, paths, task }
        }
        fn client(&self) -> GitHubClient {
            GitHubClient::new(
                octocrab::Octocrab::builder()
                    .base_uri(&self.uri)
                    .unwrap()
                    .personal_token("test-token".to_owned())
                    .build()
                    .unwrap(),
            )
        }
        fn paths(&self) -> Vec<String> {
            self.paths.lock().unwrap().clone()
        }
        fn slow_count(&self) -> usize {
            self.paths
                .lock()
                .unwrap()
                .iter()
                .filter(|path| path.contains("/slow-"))
                .count()
        }
    }
    impl Drop for HeldPusherProvider {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    // A runnable cooperative driver prevents paused Tokio time from auto-
    // advancing while real loopback IO is pending. Wall time is used only as
    // a generous hung-fixture watchdog, never to establish admission behavior.
    async fn drive_advisory_until(mut ready: impl FnMut() -> bool) {
        let watchdog = std::time::Instant::now();
        while !ready() {
            assert!(
                watchdog.elapsed() < Duration::from_secs(30),
                "advisory fixture made no progress"
            );
            tokio::task::yield_now().await;
        }
    }
    fn start_strip(
        client: GitHubClient,
        budget: Budget,
        asks: Vec<PusherAsk>,
    ) -> tokio::task::JoinHandle<Vec<RowPusher>> {
        tokio::spawn(async move { strip_pushers(&client, &budget, &asks, T).await.unwrap() })
    }
    async fn finish_strip(task: tokio::task::JoinHandle<Vec<RowPusher>>) -> Vec<RowPusher> {
        drive_advisory_until(|| task.is_finished()).await;
        task.await.unwrap()
    }

    #[tokio::test]
    async fn oversized_515_canceled_prefix_has_finite_opportunity() {
        oversized_progress(false).await;
    }
    #[tokio::test]
    async fn concurrent_oversized_515_canceled_prefix_has_finite_opportunity() {
        oversized_progress(true).await;
    }
    async fn oversized_progress(concurrent: bool) {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        let asks: Vec<_> = (0..515)
            .map(|i| ask(&format!("synthetic/slow-{i:04}"), i + 1, HEAD))
            .collect();
        tokio::time::pause();
        let budget = client.request_budget();
        for _ in 0..300 {
            let bounded = client.with_read_context(super::super::admission::ReadContext::new(
                super::super::admission::ReadClass::Advisory,
                Duration::from_millis(150),
            ));
            let before = provider.paths().len();
            let task = start_strip(bounded.clone(), budget.clone(), asks.clone());
            let peer = concurrent.then(|| start_strip(bounded, budget.clone(), asks.clone()));
            drive_advisory_until(|| provider.paths().len() >= before + 2).await;
            tokio::time::advance(Duration::from_millis(151)).await;
            let out = finish_strip(task).await;
            if let Some(peer) = peer {
                assert_eq!(finish_strip(peer).await.len(), 515);
            }
            assert_eq!(out.len(), 515, "no silent tail omission");
            assert_eq!(
                provider.paths().len(),
                before + 2,
                "two held actual attempts per canceled command"
            );
            tokio::time::advance(Duration::from_secs(31)).await;
        }
        let seen: std::collections::HashSet<_> = provider.paths().into_iter().collect();
        assert_eq!(
            seen.len(),
            515,
            "every accepted identity must reach the provider"
        );
        assert_eq!(
            provider.paths().len(),
            600,
            "count dispatched requests, including canceled replies"
        );
        tokio::time::resume();
    }

    #[tokio::test]
    async fn alternating_oversized_batches_keep_independent_continuation_after_reorder_and_head_churn(
    ) {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        let mut batches: Vec<Vec<_>> = ["a", "b"]
            .iter()
            .map(|prefix| {
                (0..515)
                    .map(|i| ask(&format!("synthetic/slow-{prefix}-{i:04}"), i + 1, HEAD))
                    .collect()
            })
            .collect();
        tokio::time::pause();
        let budget = client.request_budget();
        for cycle in 0..300 {
            for asks in &mut batches {
                asks.reverse();
                if cycle == 100 {
                    asks[0].head_oid = OTHER.into();
                }
                let bounded = client.with_read_context(super::super::admission::ReadContext::new(
                    super::super::admission::ReadClass::Advisory,
                    Duration::from_millis(150),
                ));
                let before = provider.paths().len();
                let task = start_strip(bounded, budget.clone(), asks.clone());
                drive_advisory_until(|| provider.paths().len() >= before + 2).await;
                tokio::time::advance(Duration::from_millis(151)).await;
                assert_eq!(finish_strip(task).await.len(), 515);
                assert_eq!(provider.paths().len(), before + 2);
                tokio::time::advance(Duration::from_secs(31)).await;
            }
        }
        let seen: std::collections::HashSet<_> = provider.paths().into_iter().collect();
        assert_eq!(
            seen.len(),
            1030,
            "both stable sets receive finite provider opportunity"
        );
        assert_eq!(
            provider.paths().len(),
            1200,
            "count dispatched requests, including canceled replies"
        );
        tokio::time::resume();
    }

    #[tokio::test]
    async fn batch_context_overload_is_explicit_without_http_and_idle_contexts_can_reenter() {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        tokio::time::pause();
        for i in 0..512 {
            client
                .advisory
                .batch(&[format!("context-{i}"), "other".into()])
                .unwrap();
        }
        let asks = [
            ask("synthetic/slow-a", 1, HEAD),
            ask("synthetic/slow-b", 2, HEAD),
        ];
        let result = strip_pushers(&client, &client.request_budget(), &asks, T).await;
        assert!(result.unwrap_err().contains("Retry this batch"));
        assert!(provider.paths().is_empty());
        // A live lease survives the idle TTL and shares its cursor with another caller.
        let lease = client
            .advisory
            .batch(&["context-0".into(), "other".into()])
            .unwrap()
            .unwrap();
        tokio::time::advance(Duration::from_secs(301)).await;
        let same = client
            .advisory
            .batch(&["other".into(), "context-0".into()])
            .unwrap()
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(&lease, &same));
        assert!(client
            .advisory
            .batch(&["new".into(), "other".into()])
            .is_ok());
        tokio::time::resume();
    }

    async fn slow_prefix_progress(concurrent: bool, churn: bool, staggered: bool) {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        tokio::time::pause();
        let mut asks = vec![
            ask("synthetic/slow-a", 1, HEAD),
            ask("synthetic/slow-b", 2, HEAD),
            ask("synthetic/healthy", 3, HEAD),
        ];
        if concurrent && !churn {
            asks.push(ask("synthetic/slow-new", 10, HEAD));
        }
        let mut healthy = false;
        for cycle in 0..4 {
            let bounded = client.with_read_context(super::super::admission::ReadContext::new(
                super::super::admission::ReadClass::Advisory,
                Duration::from_millis(150),
            ));
            let before = provider.paths().len();
            let slow_before = provider.slow_count();
            let budget = client.request_budget();
            let first = start_strip(bounded.clone(), budget.clone(), asks.clone());
            let second = if concurrent {
                let later = if staggered {
                    drive_advisory_until(|| provider.slow_count() >= slow_before + 2).await;
                    tokio::time::advance(Duration::from_millis(20)).await;
                    client.with_read_context(super::super::admission::ReadContext::new(
                        super::super::admission::ReadClass::Advisory,
                        Duration::from_millis(150),
                    ))
                } else {
                    bounded
                };
                Some(start_strip(later, budget, asks.clone()))
            } else {
                None
            };
            drive_advisory_until(|| provider.slow_count() >= slow_before + 2).await;
            // Both provider slots are demonstrably held before time expires.
            // No wall-clock interval must accommodate a healthy HTTP exchange.
            tokio::time::advance(Duration::from_millis(if staggered { 129 } else { 149 })).await;
            assert!(
                !first.is_finished(),
                "held leaders cannot complete before the deadline"
            );
            tokio::time::advance(Duration::from_millis(2)).await;
            let a = finish_strip(first).await;
            let b = if let Some(second) = second {
                if staggered {
                    // Singleflight and short declined receipts can leave just
                    // one new slow request. Wait for either two held requests,
                    // or one held request plus both settled healthy stages.
                    drive_advisory_until(|| {
                        let held = provider.slow_count() - slow_before;
                        let healthy_settled = client
                            .advisory
                            .rules
                            .peek(&("synthetic/healthy".into(), "main".into()))
                            .is_some()
                            && client
                                .advisory
                                .pushers
                                .peek(&("synthetic/healthy".into(), "feat/3".into(), HEAD.into()))
                                .is_some();
                        second.is_finished() || held >= 4 || (held >= 3 && healthy_settled)
                    })
                    .await;
                    tokio::time::advance(Duration::from_millis(18)).await;
                    assert!(
                        !second.is_finished(),
                        "later caller remains held before its own deadline"
                    );
                    tokio::time::advance(Duration::from_millis(2)).await;
                }
                finish_strip(second).await
            } else {
                Vec::new()
            };
            let requests = provider.paths();
            assert!(requests.len() - before <= 8, "shared actual-attempt cap");
            healthy |= a.iter().chain(&b).any(|row| {
                row.number == 3
                    && matches!(row.last_pusher, LastPusher::Known { .. })
                    && matches!(row.rules, BaseRules::Read { .. })
            });
            if cycle == 0 && !staggered {
                assert!(!healthy, "both workers are occupied by slow leaders");
                assert_eq!(requests.len(), 2, "same-key concurrent callers coalesce");
                assert!(
                    a.iter().chain(&b).all(
                        |row| row.last_known_pusher.is_none() && row.last_known_rules.is_none()
                    ),
                    "scheduling cannot fabricate observations"
                );
            }
            if churn && cycle == 0 {
                asks[0].head_oid = "changed".into();
                asks.push(ask("synthetic/slow-new", 10, HEAD));
            }
            tokio::time::advance(Duration::from_secs(30)).await;
        }
        tokio::time::resume();
        assert!(
            healthy,
            "healthy survivor never received a provider opportunity"
        );
        assert!(provider
            .paths()
            .iter()
            .any(|path| path == "/repos/synthetic/healthy/activity"));
        println!("slow-prefix: concurrent={concurrent} churn={churn} staggered={staggered} cycles=4 actual_attempts={} healthy_observed={healthy}", provider.paths().len());
    }

    #[tokio::test]
    async fn cached_pushers_bypass_busy_workers_and_cancellation_releases_them() {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        tokio::time::pause();
        let cached_asks = vec![ask("synthetic/cached", 1, HEAD)];
        let before = finish_strip(start_strip(
            client.clone(),
            client.request_budget(),
            cached_asks.clone(),
        ))
        .await;
        assert!(matches!(before[0].last_pusher, LastPusher::Known { .. }));
        let slow = start_strip(
            client.clone(),
            client.request_budget(),
            vec![
                ask("synthetic/slow-a", 2, HEAD),
                ask("synthetic/slow-b", 3, HEAD),
            ],
        );
        drive_advisory_until(|| provider.slow_count() == 2).await;
        let cached = finish_strip(start_strip(
            client.clone(),
            client.request_budget(),
            cached_asks,
        ))
        .await;
        assert!(matches!(cached[0].last_pusher, LastPusher::Known { .. }));
        assert!(cached[0].pusher_valid_for_ms <= before[0].pusher_valid_for_ms);
        assert!(cached[0].rules_valid_for_ms <= before[0].rules_valid_for_ms);
        assert_eq!(provider.paths().len(), 4, "cached call issues no HTTP");
        assert!(
            !slow.is_finished(),
            "cached caller completed while leaders remain held"
        );
        let other = provider.client();
        let independent = finish_strip(start_strip(
            other.clone(),
            other.request_budget(),
            vec![ask("synthetic/other-account", 4, HEAD)],
        ))
        .await;
        assert!(provider
            .paths()
            .iter()
            .any(|path| path == "/repos/synthetic/other-account/activity"));
        assert!(matches!(
            independent[0].last_pusher,
            LastPusher::Known { .. }
        ));
        assert!(
            !slow.is_finished(),
            "other account completed while leaders remain held"
        );
        slow.abort();
        assert!(slow.await.unwrap_err().is_cancelled());
        let recovered = finish_strip(start_strip(
            client.clone(),
            client.request_budget(),
            vec![ask("synthetic/recovered", 5, HEAD)],
        ))
        .await;
        assert!(provider
            .paths()
            .iter()
            .any(|path| path == "/repos/synthetic/recovered/activity"));
        assert!(matches!(recovered[0].last_pusher, LastPusher::Known { .. }));
        tokio::time::resume();
    }

    #[tokio::test]
    async fn command_deadline_retains_rules_completed_before_slow_activity() {
        let server = MockServer::start().await;
        Mock::given(path("/repos/octocat/hello-world/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(path("/repos/octocat/hello-world/activity"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(2))
                    .set_body_json(json!([])),
            )
            .mount(&server)
            .await;
        let client =
            client_for(&server)
                .await
                .with_read_context(super::super::admission::ReadContext::new(
                    super::super::admission::ReadClass::Advisory,
                    Duration::from_millis(80),
                ));
        let start = tokio::time::Instant::now();
        let rows = strip_pushers(
            &client,
            &client.request_budget(),
            &[ask("octocat/hello-world", 1, HEAD)],
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(matches!(rows[0].rules, BaseRules::Read { .. }));
        assert!(!matches!(rows[0].last_pusher, LastPusher::Known { .. }));
    }

    fn pr_rule(last_push: bool, resolution: bool) -> serde_json::Value {
        json!({
            "type": "pull_request",
            "parameters": {
                "required_approving_review_count": 1,
                "require_last_push_approval": last_push,
                "required_review_thread_resolution": resolution
            }
        })
    }

    fn push(kind: &str, after: &str, login: &str) -> serde_json::Value {
        json!({ "activity_type": kind, "after": after, "actor": { "login": login } })
    }

    const HEAD: &str = "1111111111111111111111111111111111111111";
    const OTHER: &str = "2222222222222222222222222222222222222222";
    const T: Duration = Duration::from_secs(10);

    /// Rulesets stack, and GitHub enforces the strictest, so ONE rule
    /// setting a requirement turns it on whatever the others say.
    #[test]
    fn a_requirement_is_on_if_any_pull_request_rule_sets_it() {
        let v = json!([
            { "type": "deletion" },
            pr_rule(false, false),
            pr_rule(true, false),
            pr_rule(false, true),
        ]);
        assert_eq!(map_rules(&v), Some((true, true)));
    }

    /// An empty list is a READ with nothing required -- which the UI must
    /// still not render as "not required", since classic protection is
    /// invisible here. The mapper's job is only to report what it read.
    #[test]
    fn no_pull_request_rule_reads_as_nothing_required() {
        assert_eq!(map_rules(&json!([])), Some((false, false)));
        assert_eq!(
            map_rules(&json!([{ "type": "deletion" }])),
            Some((false, false))
        );
    }

    /// Anything but the documented list is not an answer.
    #[test]
    fn a_non_list_rules_answer_is_not_read() {
        assert_eq!(map_rules(&json!({ "message": "Not Found" })), None);
    }

    /// The newest ref move names the head commit, so its actor pushed it.
    /// `branch_creation` counts: MEASURED, a head only ever created has no
    /// `push` entry at all.
    #[test]
    fn the_newest_move_to_the_head_commit_names_the_pusher() {
        let v = json!([push("branch_creation", HEAD, "someone")]);
        assert_eq!(
            map_last_pusher(&v, HEAD),
            LastPusher::Known {
                login: "someone".into()
            }
        );
        let v = json!([push("force_push", HEAD, "a"), push("push", OTHER, "b")]);
        assert_eq!(
            map_last_pusher(&v, HEAD),
            LastPusher::Known { login: "a".into() }
        );
    }

    /// If the newest move went to a DIFFERENT commit, the view is stale or
    /// the log lags -- an older entry naming our commit is not accepted.
    #[test]
    fn a_newest_move_to_another_commit_leaves_the_pusher_unknown() {
        let v = json!([push("push", OTHER, "b"), push("push", HEAD, "a")]);
        assert!(matches!(
            map_last_pusher(&v, HEAD),
            LastPusher::Unknown { .. }
        ));
    }

    /// Non-moving entries are skipped rather than trusted or fatal.
    #[test]
    fn non_moving_activity_is_skipped() {
        let v = json!([
            { "activity_type": "pr_merge", "after": OTHER, "actor": { "login": "m" } },
            push("push", HEAD, "a"),
        ]);
        assert_eq!(
            map_last_pusher(&v, HEAD),
            LastPusher::Known { login: "a".into() }
        );
    }

    /// Nothing on record, a missing actor, or an empty head is unknown --
    /// never a guess.
    #[test]
    fn missing_evidence_is_unknown() {
        assert!(matches!(
            map_last_pusher(&json!([]), HEAD),
            LastPusher::Unknown { .. }
        ));
        let v = json!([{ "activity_type": "push", "after": HEAD, "actor": null }]);
        assert!(matches!(
            map_last_pusher(&v, HEAD),
            LastPusher::Unknown { .. }
        ));
        let v = json!([push("push", HEAD, "a")]);
        assert!(matches!(
            map_last_pusher(&v, ""),
            LastPusher::Unknown { .. }
        ));
    }

    /// Only a well-formed `owner/name` is spliced into a path.
    #[test]
    fn only_owner_slash_name_is_a_valid_repo() {
        assert!(valid_repo("some-org/some.repo_1"));
        assert!(!valid_repo("some-org"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("../x"));
        assert!(!valid_repo("a/b?x=1"));
    }

    /// A branch's slashes stay; anything that could end the path or start
    /// a new query parameter does not.
    #[test]
    fn encoding_keeps_slashes_and_escapes_query_syntax() {
        assert_eq!(encode("refs/heads/feat/x"), "refs/heads/feat/x");
        assert_eq!(encode("a&b#c+d"), "a%26b%23c%2Bd");
    }

    /// Rule present, viewer's commit on top: the rules and the pusher both
    /// arrive, through the budget's REST accounting.
    #[tokio::test]
    async fn rule_present_reads_rules_then_the_pusher() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-one/rules/branches/main"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([pr_rule(true, true)]))
                    .insert_header("x-ratelimit-remaining", "4900"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-one/activity"))
            .and(query_param("ref", "refs/heads/feat/x"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([push("push", HEAD, "viewer")])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let budget = Budget::new();
        let g = review_gates(
            &client,
            &budget,
            "gate-org/gate-one",
            "main",
            Some("gate-org/gate-one"),
            "feat/x",
            HEAD,
            T,
        )
        .await;
        assert_eq!(
            g.rules,
            BaseRules::Read {
                require_last_push_approval: true,
                required_review_thread_resolution: true
            }
        );
        assert_eq!(
            g.last_pusher,
            LastPusher::Known {
                login: "viewer".into()
            }
        );
        assert_eq!(budget.rest_requests(), 2);
    }

    #[tokio::test]
    async fn detail_receipts_keep_original_rules_and_pusher_lifetimes_across_shared_clients() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(|request: &wiremock::Request| {
                if request.url.path().contains("/activity") {
                    ResponseTemplate::new(200).set_body_json(json!([push("push", HEAD, "viewer")]))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!([pr_rule(true, true)]))
                }
            })
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let first = review_gates(
            &client,
            &client.request_budget(),
            "receipt/repo",
            "main",
            Some("receipt/repo"),
            "topic",
            HEAD,
            T,
        )
        .await;
        assert!(first
            .rules_valid_for_ms
            .is_some_and(|ms| ms > 599_000 && ms <= 600_000));
        assert!(first
            .pusher_valid_for_ms
            .is_some_and(|ms| ms > 59_000 && ms <= 60_000));
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(55)).await;
        let peer = client.clone();
        let second = review_gates(
            &peer,
            &peer.request_budget(),
            "receipt/repo",
            "main",
            Some("receipt/repo"),
            "topic",
            HEAD,
            T,
        )
        .await;
        assert!(second
            .rules_valid_for_ms
            .is_some_and(|ms| ms > 544_000 && ms <= 545_000));
        assert!(second
            .pusher_valid_for_ms
            .is_some_and(|ms| ms > 4_000 && ms <= 5_000));
        tokio::time::resume();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        let legacy: ReviewGates = serde_json::from_value(json!({"rules":{"state":"unreadable","reason":"old peer"},"last_pusher":{"state":"not_needed"}})).unwrap();
        assert_eq!(legacy.rules_valid_for_ms, None);
    }

    #[tokio::test]
    async fn independent_pusher_timeout_preserves_rules_and_consumes_original_lifetime() {
        let provider = HeldPusherProvider::start().await;
        let client = provider.client();
        // A previous selected read supplied the shared native policy receipt.
        client
            .advisory
            .rules
            .load(
                ("receipt/repo".into(), "main".into()),
                tokio::time::Instant::now() + T,
                |_| RULES_TTL,
                async {
                    BaseRules::Read {
                        require_last_push_approval: true,
                        required_review_thread_resolution: true,
                    }
                },
            )
            .await
            .unwrap();
        tokio::time::pause();
        let worker = client.clone();
        let task = tokio::spawn(async move {
            review_gates(
                &worker,
                &worker.request_budget(),
                "receipt/repo",
                "main",
                Some("synthetic/slow-pusher"),
                "topic",
                HEAD,
                T,
            )
            .await
        });
        drive_advisory_until(|| provider.paths().len() == 1).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        drive_advisory_until(|| task.is_finished()).await;
        let value = task.await.unwrap();
        assert!(matches!(value.rules, BaseRules::Read { .. }));
        assert!(matches!(value.last_pusher, LastPusher::Unknown { .. }));
        assert!(value
            .rules_valid_for_ms
            .is_some_and(|ms| ms > 588_000 && ms <= 589_000));
        assert_eq!(value.pusher_valid_for_ms, Some(0));
        assert_eq!(
            provider.paths().len(),
            1,
            "good rules are reused while only the pusher is asked"
        );
        tokio::time::resume();
    }

    /// Rule absent: the pusher is never asked for, so a repository without
    /// the rule costs one read. `expect(0)` fails the test if it is.
    #[tokio::test]
    async fn rule_absent_does_not_ask_for_the_pusher() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-two/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([pr_rule(false, true)])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-two/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let g = review_gates(
            &client,
            &Budget::new(),
            "gate-org/gate-two",
            "main",
            Some("gate-org/gate-two"),
            "feat/x",
            HEAD,
            T,
        )
        .await;
        assert_eq!(g.last_pusher, LastPusher::NotNeeded);
        assert!(matches!(
            g.rules,
            BaseRules::Read {
                required_review_thread_resolution: true,
                ..
            }
        ));
    }

    /// A refused lookup (404: the viewer cannot read the rules) is
    /// Unreadable, NOT an empty rule set; retry after short failure eligibility.
    #[tokio::test]
    async fn refused_rules_are_unreadable_and_retry_after_short_eligibility() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-three/rules/branches/main"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": "https://docs.github.com/rest"
            })))
            .expect(2)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        for _ in 0..2 {
            let g = review_gates(
                &client,
                &Budget::new(),
                "gate-org/gate-three",
                "main",
                Some("gate-org/gate-three"),
                "feat/x",
                HEAD,
                T,
            )
            .await;
            assert!(matches!(g.rules, BaseRules::Unreadable { .. }), "{g:?}");
            assert_eq!(g.last_pusher, LastPusher::NotNeeded);
            tokio::time::pause();
            tokio::time::advance(FAILURE_TTL).await;
            tokio::time::resume();
        }
    }

    /// A successful read is cached per (repo, base): the second open of a
    /// pull request into the same base asks GitHub nothing.
    #[tokio::test]
    async fn rules_are_cached_per_repo_and_base() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-four/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let budget = Budget::new();
        for _ in 0..2 {
            let r = base_rules(&client, &budget, "gate-org/gate-four", "main").await;
            assert!(matches!(r, BaseRules::Read { .. }));
        }
        assert_eq!(budget.rest_requests(), 1);
    }

    /// A low REST budget DECLINES: nothing is sent, and the state says we
    /// did not ask rather than that GitHub did not answer. Seeded locally,
    /// touching no process-wide figure (src-tauri/CLAUDE.md).
    #[tokio::test]
    async fn a_low_rest_budget_declines_without_asking() {
        let server = MockServer::start().await;
        let client = client_for(&server).await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-remaining", "10")
                    .set_body_json(json!([])),
            )
            .mount(&server)
            .await;
        client
            .rest_get("/quota", &client.request_budget())
            .await
            .unwrap();
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let budget = client.request_budget();
        let r = base_rules(&client, &budget, "gate-org/gate-five", "main").await;
        assert!(matches!(r, BaseRules::Declined { .. }), "{r:?}");
        let p = last_pusher(&client, &budget, Some("gate-org/gate-five"), "x", HEAD).await;
        assert!(matches!(p, LastPusher::Declined { .. }), "{p:?}");
    }

    /// A failed pusher lookup is Unknown with the rule still read, so the
    /// view can qualify rather than assert.
    #[tokio::test]
    async fn a_failed_pusher_lookup_is_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-six/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([pr_rule(true, false)])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/gate-org/gate-six/activity"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "boom" })))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let g = review_gates(
            &client,
            &Budget::new(),
            "gate-org/gate-six",
            "main",
            Some("gate-org/gate-six"),
            "feat/x",
            HEAD,
            T,
        )
        .await;
        assert!(matches!(
            g.rules,
            BaseRules::Read {
                require_last_push_approval: true,
                ..
            }
        ));
        assert!(matches!(g.last_pusher, LastPusher::Unknown { .. }), "{g:?}");
    }

    /// No head repository (a deleted fork, or the seeded placeholder) is
    /// declined -- the base repository is never asked in its place.
    #[tokio::test]
    async fn no_head_repository_declines_the_pusher() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let p = last_pusher(&client, &Budget::new(), None, "feat/x", HEAD).await;
        assert!(matches!(p, LastPusher::Declined { .. }), "{p:?}");
    }

    fn ask(repo: &str, number: u64, head_oid: &str) -> PusherAsk {
        PusherAsk {
            repo: repo.into(),
            number,
            base: "main".into(),
            head_repo: Some(repo.into()),
            head_ref: format!("feat/{number}"),
            head_oid: head_oid.into(),
        }
    }

    /// #1576: a known pusher is cached by HEAD COMMIT. The same head asks
    /// GitHub once however many refreshes; a new head asks again, because
    /// only a push moves it and that push may be someone else's.
    #[tokio::test]
    async fn strip_pushers_are_cached_by_head_commit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-one/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([pr_rule(true, false)])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-one/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                push("push", HEAD, "viewer"),
                push("push", OTHER, "someone")
            ])))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let budget = Budget::seeded_rest_for_test(5000);
        for _ in 0..2 {
            let out = strip_pushers(&client, &budget, &[ask("strip-org/strip-one", 1, HEAD)], T)
                .await
                .unwrap();
            assert_eq!(
                out[0].last_pusher,
                LastPusher::Known {
                    login: "viewer".into()
                }
            );
            assert!(matches!(
                out[0].rules,
                BaseRules::Read {
                    require_last_push_approval: true,
                    ..
                }
            ));
        }
        assert_eq!(
            budget.rest_requests(),
            2,
            "one rules read, one activity read"
        );

        // A different head is a different question: asked again. The log
        // names OTHER's pusher only as the second entry, so it is Unknown
        // -- and never borrowed from HEAD's cached answer.
        let out = strip_pushers(&client, &budget, &[ask("strip-org/strip-one", 1, OTHER)], T)
            .await
            .unwrap();
        assert!(
            matches!(out[0].last_pusher, LastPusher::Unknown { .. }),
            "{out:?}"
        );
        assert_eq!(budget.rest_requests(), 3);
    }

    /// Activity can lag a fresh push: Unknown has short retry eligibility.
    #[tokio::test]
    async fn an_unknown_strip_pusher_retries_after_short_eligibility() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-two/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-two/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(2)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let budget = Budget::seeded_rest_for_test(5000);
        for _ in 0..2 {
            let out = strip_pushers(&client, &budget, &[ask("strip-org/strip-two", 2, HEAD)], T)
                .await
                .unwrap();
            assert!(
                matches!(out[0].last_pusher, LastPusher::Unknown { .. }),
                "{out:?}"
            );
            tokio::time::pause();
            tokio::time::advance(FAILURE_TTL).await;
            tokio::time::resume();
        }
    }

    /// Past the per-refresh cap a row is NOT CHECKED -- `Declined`, never
    /// `Unknown` -- and nothing is sent for it. The rows inside the cap
    /// are the first ones, in the order the strip gave.
    #[tokio::test]
    async fn rows_past_the_strip_cap_are_not_checked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-three/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-three/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(7)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let asks: Vec<PusherAsk> = (0..32)
            .map(|n| ask("strip-org/strip-three", n, &format!("{n:040}")))
            .collect();
        let out = strip_pushers(&client, &Budget::seeded_rest_for_test(5000), &asks, T)
            .await
            .unwrap();
        assert_eq!(out.len(), asks.len(), "every row answered, none dropped");
        for (i, row) in out.iter().enumerate() {
            assert_eq!(row.number, i as u64, "answers stay in the rows' order");
            if i < 7 {
                assert!(
                    matches!(row.last_pusher, LastPusher::Unknown { .. }),
                    "{row:?}"
                );
            } else {
                assert!(
                    matches!(row.last_pusher, LastPusher::Declined { .. }),
                    "{row:?}"
                );
            }
        }
    }

    /// A spent REST budget sends nothing and says so: rules and pusher are
    /// both `Declined` (we did not ask), which the strip counts as not
    /// checked and never hides on.
    #[tokio::test]
    async fn a_spent_budget_leaves_strip_rows_not_checked() {
        let server = MockServer::start().await;
        let client = client_for(&server).await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-remaining", "10")
                    .set_body_json(json!([])),
            )
            .mount(&server)
            .await;
        client
            .rest_get("/quota", &client.request_budget())
            .await
            .unwrap();
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let out = strip_pushers(
            &client,
            &client.request_budget(),
            &[ask("strip-org/strip-four", 4, HEAD)],
            T,
        )
        .await
        .unwrap();
        assert!(
            matches!(out[0].rules, BaseRules::Declined { .. }),
            "{out:?}"
        );
        assert!(
            matches!(out[0].last_pusher, LastPusher::Declined { .. }),
            "{out:?}"
        );
    }

    /// No head repository: declined without a request, and it does not
    /// use up a slot under the cap.
    #[tokio::test]
    async fn a_strip_row_without_a_head_repository_is_not_asked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-five/rules/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/strip-org/strip-five/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let mut a = ask("strip-org/strip-five", 5, HEAD);
        a.head_repo = None;
        let out = strip_pushers(&client, &Budget::seeded_rest_for_test(5000), &[a], T)
            .await
            .unwrap();
        assert!(
            matches!(out[0].last_pusher, LastPusher::Declined { .. }),
            "{out:?}"
        );
    }
}
