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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    pub rules: BaseRules,
    pub last_pusher: LastPusher,
}

/// Rules change rarely -- an admin edits a ruleset -- so one read per
/// (repository, base branch) serves every pull request into that base for
/// this long. Ten minutes bounds how stale a just-edited rule can read
/// while keeping a triage session that opens twenty PRs into `main` at one
/// request rather than twenty.
const RULES_TTL: Duration = Duration::from_secs(600);

/// Successful reads only. A failure is not cached, so a transient 502
/// does not suppress the gate for ten minutes; the next open asks again.
static RULES_CACHE: Mutex<Option<RulesCache>> = Mutex::new(None);

/// (repository, base branch) -> when read, and
/// `(require_last_push_approval, required_review_thread_resolution)`.
type RulesCache = HashMap<(String, String), (Instant, (bool, bool))>;

fn cached_rules(repo: &str, base: &str) -> Option<(bool, bool)> {
    let guard = RULES_CACHE.lock().ok()?;
    let (at, rules) = guard.as_ref()?.get(&(repo.to_string(), base.to_string()))?;
    (at.elapsed() < RULES_TTL).then_some(*rules)
}

fn store_rules(repo: &str, base: &str, rules: (bool, bool)) {
    if let Ok(mut guard) = RULES_CACHE.lock() {
        guard.get_or_insert_with(HashMap::new).insert(
            (repo.to_string(), base.to_string()),
            (Instant::now(), rules),
        );
    }
}

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
    if !valid_repo(repo) || base.is_empty() {
        return BaseRules::Declined {
            reason: "no base branch to ask about".into(),
        };
    }
    if let Some((a, b)) = cached_rules(repo, base) {
        return BaseRules::Read {
            require_last_push_approval: a,
            required_review_thread_resolution: b,
        };
    }
    if !budget.permits_rest(1) {
        return BaseRules::Declined {
            reason: "the REST rate-limit budget is nearly spent".into(),
        };
    }
    let path = format!("/repos/{repo}/rules/branches/{}", encode(base));
    match client.rest_get(&path, budget).await {
        Ok(v) => match map_rules(&v) {
            Some((a, b)) => {
                store_rules(repo, base, (a, b));
                BaseRules::Read {
                    require_last_push_approval: a,
                    required_review_thread_resolution: b,
                }
            }
            None => BaseRules::Unreadable {
                reason: "GitHub's rules answer was not a list".into(),
            },
        },
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
    // No head repository means the fork is gone or the detail has not
    // arrived. Asking the BASE repository instead would be a guess -- a
    // same-named branch there is a different branch.
    let Some(head_repo) = head_repo.filter(|r| valid_repo(r)) else {
        return LastPusher::Declined {
            reason: "the head repository is not known".into(),
        };
    };
    if head_ref.is_empty() || head_oid.is_empty() {
        return LastPusher::Declined {
            reason: "the head branch is not known".into(),
        };
    }
    if !budget.permits_rest(1) {
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
    let rules = tokio::time::timeout(per_request, base_rules(client, budget, repo, base))
        .await
        .unwrap_or_else(|_| BaseRules::Unreadable {
            reason: format!("timed out after {}s", per_request.as_secs()),
        });
    let last_pusher = match rules {
        BaseRules::Read {
            require_last_push_approval: true,
            ..
        } => tokio::time::timeout(
            per_request,
            last_pusher(client, budget, head_repo, head_ref, head_oid),
        )
        .await
        .unwrap_or_else(|_| LastPusher::Unknown {
            reason: format!("timed out after {}s", per_request.as_secs()),
        }),
        _ => LastPusher::NotNeeded,
    };
    ReviewGates { rules, last_pusher }
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
    /// Unreadable, NOT an empty rule set -- and is not cached, so the next
    /// open asks again.
    #[tokio::test]
    async fn refused_rules_are_unreadable_and_not_cached() {
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
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let budget = Budget::seeded_rest_for_test(10);
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
}
