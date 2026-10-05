//! Where a pull request sits in a stack (#1452).
//!
//! Asked of GitHub per pull request, on the DETAIL path, because the list
//! cannot answer it: `deriveStacked` (`src/lib/derive.ts`) only sees the
//! rows the current view holds, and in "To review" the parent of a stacked
//! pull request is usually not one of them. That is how a stacked PR came
//! to offer an "Add to merge queue" GitHub then refused.
//!
//! Two lookups, both bounded:
//!
//! - `PR_STACK_QUERY`: GitHub's native stack entry, plus the base chain
//!   DOWN toward the trunk, nested in one request.
//! - `PR_STACK_UP_QUERY`: the chain UP, one serial request per hop, since
//!   nothing in the schema lists the pull requests targeting a branch.
//!
//! Either walk can stop early -- out of hops, out of time, an ambiguous
//! branch, a failed request -- and what it already found is kept and
//! QUALIFIED ("at least"), never discarded and never printed as exact.

use super::advisory::{Measurement, StackKey, FAILURE_TTL, SUCCESS_TTL};
use super::client::GitHubClient;
use super::model::{PrStack, StackMember};
use super::query::{PR_STACK_QUERY, PR_STACK_UP_QUERY};
use super::stats::Budget;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

/// Upward hops before the total is reported as a floor.
///
/// Each is a serial POST, so this is a latency budget. Three covers the
/// stacks this app has been shown (two to four pull requests) with the
/// pull request itself anywhere in them; a taller stack still renders,
/// qualified.
pub const UP_HOPS: usize = 3;

/// Wall-clock ceiling on the whole lookup.
///
/// Well inside `poll::FETCH_TIMEOUT` (30s), which bounds the detail
/// command this runs beside: a stack walk that ran the command out of time
/// would cost the whole pull request, not just the badge. Enforced PER
/// REQUEST against what remains, not by wrapping the walk in one
/// `tokio::time::timeout` -- that would drop the future and the hops it
/// had already found, which is #1044's defect.
pub const STACK_BUDGET: Duration = Duration::from_secs(10);

/// What the downward query established.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Down {
    /// `(position, size, stack number)` from `stackEntry`, when native.
    pub native: Option<(u64, u64, u64)>,
    /// The native stack's entries, bottom first (#1468).
    pub members: Vec<StackMember>,
    /// Whether `members` is the whole list.
    pub members_complete: bool,
    pub head: String,
    pub cross_repository: bool,
    /// The open pull requests beneath this one, nearest first.
    pub parents: Vec<u64>,
    /// True when the walk reached the bottom of the chain.
    pub complete: bool,
    /// The walk stopped at a branch that two open pull requests share, so
    /// there IS something beneath that point -- just not one we can name.
    pub ambiguous: bool,
}

/// Read `PR_STACK_QUERY`'s response. `None` when the pull request itself
/// is missing, which is "could not tell", not "not stacked".
pub fn parse_down(v: &Value) -> Option<Down> {
    let repo = &v["repository"];
    let pr = &repo["pullRequest"];
    let number = pr["number"].as_u64()?;
    let trunk = repo["defaultBranchRef"]["name"].as_str();

    let native = match (
        pr["stackEntry"]["position"].as_u64(),
        pr["stackEntry"]["stack"]["size"].as_u64(),
    ) {
        (Some(p), Some(s)) => Some((
            p,
            s,
            pr["stackEntry"]["stack"]["number"].as_u64().unwrap_or(0),
        )),
        _ => None,
    };

    let (members, members_complete) = parse_members(&pr["stackEntry"]["stack"]["entries"]);
    let mut down = Down {
        native,
        members,
        members_complete,
        head: pr["headRefName"].as_str().unwrap_or_default().to_string(),
        cross_repository: pr["isCrossRepository"].as_bool().unwrap_or(false),
        ..Down::default()
    };

    let mut node = pr;
    loop {
        // A base that IS the trunk has nothing beneath it -- the one ending
        // that needs no further lookup to be certain of.
        if trunk.is_some() && node["baseRefName"].as_str() == trunk {
            down.complete = true;
            break;
        }
        // The deepest level of the nested query selects no `baseRef`, so an
        // ABSENT key means the walk ran out of hops, not that the chain
        // ended. `null` is different: the base branch is gone, and no open
        // pull request can have a deleted branch as its head.
        let Some(base_ref) = node.get("baseRef") else {
            break;
        };
        let Some(prs) = base_ref["associatedPullRequests"]["nodes"].as_array() else {
            down.complete = base_ref.is_null();
            break;
        };
        match prs.as_slice() {
            [] => {
                down.complete = true;
                break;
            }
            [only] => {
                let Some(n) = only["number"].as_u64() else {
                    break;
                };
                // A cycle cannot be a real stack. Stop, and leave the walk
                // marked incomplete rather than looping or claiming a count.
                if n == number || down.parents.contains(&n) {
                    break;
                }
                down.parents.push(n);
                node = only;
            }
            _ => {
                down.ambiguous = true;
                break;
            }
        }
    }
    Some(down)
}

/// A native stack's `entries`, bottom first, and whether that is all of
/// them. An entry GitHub could not describe (a pull request the viewer
/// cannot see) makes the list incomplete rather than silently shorter.
fn parse_members(entries: &Value) -> (Vec<StackMember>, bool) {
    let Some(nodes) = entries["nodes"].as_array() else {
        return (Vec::new(), false);
    };
    let mut members: Vec<StackMember> = nodes
        .iter()
        .filter_map(|n| {
            let pr = &n["pullRequest"];
            Some(StackMember {
                position: n["position"].as_u64()?,
                number: pr["number"].as_u64()?,
                title: pr["title"].as_str().unwrap_or_default().to_string(),
                state: pr["state"].as_str()?.to_lowercase(),
                is_draft: pr["isDraft"].as_bool(),
                review: pr["reviewDecision"].as_str().map(str::to_lowercase),
                checks: pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"]
                    .as_str()
                    .map(str::to_lowercase),
            })
        })
        .collect();
    members.sort_by_key(|m| m.position);
    let complete = members.len() == nodes.len()
        && entries["totalCount"].as_u64() == Some(members.len() as u64);
    (members, complete)
}

/// One step of the upward walk.
#[derive(Debug, Clone, PartialEq)]
pub enum UpStep {
    /// Nothing is based on this branch: the top of the stack.
    Top,
    /// Exactly one open pull request is. `head` is `None` when that pull
    /// request comes from a fork, whose branch nothing in THIS repository
    /// can be based on -- so it is also the top.
    Child { number: u64, head: Option<String> },
    /// Two or more are: a tree, not a chain, so "N of M" has no single
    /// answer past this point.
    Branches,
    /// The response did not say.
    Unreadable,
}

pub fn parse_up(v: &Value) -> UpStep {
    let Some(nodes) = v["repository"]["pullRequests"]["nodes"].as_array() else {
        return UpStep::Unreadable;
    };
    match nodes.as_slice() {
        [] => UpStep::Top,
        [only] => match only["number"].as_u64() {
            Some(number) => UpStep::Child {
                number,
                head: if only["isCrossRepository"].as_bool().unwrap_or(false) {
                    None
                } else {
                    only["headRefName"].as_str().map(str::to_string)
                },
            },
            None => UpStep::Unreadable,
        },
        _ => UpStep::Branches,
    }
}

/// Combine both walks into the one answer the view renders.
pub fn assemble(down: &Down, children: &[u64], up_complete: bool) -> PrStack {
    if let Some((position, size, number)) = down.native {
        return PrStack::Stacked {
            native: true,
            stack_number: (number > 0).then_some(number),
            position,
            size,
            position_exact: true,
            size_exact: true,
            below: down.parents.first().copied(),
            members: down.members.clone(),
            members_complete: down.members_complete,
        };
    }
    // An ambiguous hop still proves there is SOMETHING beneath it, so the
    // floor counts it.
    let position = down.parents.len() as u64 + 1 + u64::from(down.ambiguous);
    if position == 1 && children.is_empty() {
        // Nothing found either way. Only a walk that FINISHED both ways may
        // say so; otherwise it is unknown, not standalone.
        return if down.complete && up_complete {
            PrStack::None
        } else {
            PrStack::Unknown
        };
    }
    PrStack::Stacked {
        native: false,
        stack_number: None,
        position,
        size: position + children.len() as u64,
        position_exact: down.complete,
        size_exact: down.complete && up_complete,
        below: down.parents.first().copied(),
        members: Vec::new(),
        members_complete: false,
    }
}

#[derive(Debug, Clone)]
struct Continuation {
    down: Down,
    children: Vec<u64>,
    head: String,
    hops: usize,
    observed_at: Instant,
    valid_until: Instant,
}
type ContinuationSlot = Arc<Mutex<Option<Continuation>>>;
#[derive(Debug, Default)]
pub(super) struct Continuations {
    slots: Mutex<HashMap<StackKey, (Instant, ContinuationSlot)>>,
}
impl Continuations {
    fn slot(&self, key: StackKey) -> Option<ContinuationSlot> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, slot)) = slots.get_mut(&key) {
            *at = Instant::now();
            return Some(slot.clone());
        }
        if slots.len() >= 512 {
            let victim = slots
                .iter()
                .filter(|(_, (_, slot))| Arc::strong_count(slot) == 1)
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())?;
            slots.remove(&victim);
        }
        let slot = Arc::new(Mutex::new(None));
        slots.insert(key, (Instant::now(), slot.clone()));
        Some(slot)
    }
    pub fn pending(&self, key: &StackKey) -> bool {
        let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.get(key).is_some_and(|(_, slot)| {
            slot.lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|state| state.valid_until > Instant::now())
        })
    }
}
fn unknown_measurement() -> Measurement<PrStack> {
    let observed_at = Instant::now();
    Measurement {
        value: PrStack::Unknown,
        observed_at,
        valid_until: observed_at + FAILURE_TTL,
        successful: false,
    }
}

impl GitHubClient {
    /// Where `number` sits in a stack. Never an error: every failure is
    /// `PrStack::Unknown`, or a partial answer qualified as one.
    pub async fn fetch_pr_stack(&self, owner: &str, name: &str, number: u64) -> PrStack {
        self.fetch_stack_with_budget(owner, name, number, &self.request_budget(), false, None)
            .await
            .value
    }

    /// Advisory list enrichment leaves the reserve for explicit detail reads.
    /// A batch shares its local minimum so concurrent responses cannot raise
    /// the quota another member already found to be low.
    pub(super) async fn fetch_pr_stack_advisory(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        budget: &Budget,
        expected: Option<(&str, &str)>,
    ) -> Measurement<PrStack> {
        self.fetch_stack_with_budget(owner, name, number, budget, true, expected)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_stack_with_budget(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        budget: &Budget,
        advisory: bool,
        expected: Option<(&str, &str)>,
    ) -> Measurement<PrStack> {
        if advisory && !budget.permits(8) {
            return unknown_measurement();
        }
        let started = Instant::now();
        let remaining = || STACK_BUDGET.saturating_sub(started.elapsed());
        let slot = if advisory {
            expected.and_then(|(head, base)| {
                self.advisory.stack_continuations.slot((
                    format!("{owner}/{name}"),
                    number,
                    head.into(),
                    base.into(),
                ))
            })
        } else {
            None
        };
        let resumed = slot.as_ref().and_then(|slot| {
            let mut saved = slot.lock().unwrap_or_else(|e| e.into_inner());
            if saved
                .as_ref()
                .is_some_and(|state| state.valid_until <= Instant::now())
            {
                *saved = None;
            }
            saved.clone()
        });
        let save = |state: Option<Continuation>| {
            if let Some(slot) = &slot {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = state;
            }
        };
        let mut state =
            if let Some(state) = resumed {
                state
            } else {
                let first=tokio::time::timeout(remaining(), self.graphql_partial_ok(&json!({
                "query":PR_STACK_QUERY,"variables":{"owner":owner,"repo":name,"number":number}
            }))).await;
                let v = match first {
                    Ok(Ok(v)) => v,
                    _ => return unknown_measurement(),
                };
                budget.record(&v);
                if super::client::refused_fields_of(&v) > 0 {
                    return unknown_measurement();
                }
                if let Some((head, base)) = expected {
                    let row = &v["repository"]["pullRequest"];
                    if row["number"].as_u64() != Some(number)
                        || row["headRefOid"].as_str() != Some(head)
                        || row["baseRefName"].as_str() != Some(base)
                    {
                        return unknown_measurement();
                    }
                }
                let Some(down) = parse_down(&v) else {
                    return unknown_measurement();
                };
                let observed_at = Instant::now();
                Continuation {
                    head: down.head.clone(),
                    down,
                    children: Vec::new(),
                    hops: 0,
                    observed_at,
                    valid_until: observed_at + SUCCESS_TTL,
                }
            };
        let mut complete = state.down.native.is_some()
            || state.down.cross_repository
            || state.down.head.is_empty();
        let mut resumable = false;
        while !complete && state.hops < UP_HOPS {
            // Save only parsed, identity-checked progress BEFORE the next await,
            // so cancellation cannot discard a successfully received prerequisite.
            save(Some(state.clone()));
            if advisory && !budget.permits(8) {
                resumable = true;
                break;
            }
            let step=tokio::time::timeout(remaining(), self.graphql_partial_ok(&json!({
                "query":PR_STACK_UP_QUERY,"variables":{"owner":owner,"repo":name,"base":state.head}
            }))).await;
            let page = match step {
                Ok(Ok(page)) => page,
                Ok(Err(super::client::ClientError::NotDispatched(_))) | Err(_) => {
                    resumable = true;
                    break;
                }
                Ok(Err(_)) => break,
            };
            budget.record(&page);
            state.hops += 1;
            match parse_up(&page) {
                UpStep::Top => complete = true,
                UpStep::Child {
                    number: child,
                    head: next,
                } => {
                    if child == number
                        || state.children.contains(&child)
                        || state.down.parents.contains(&child)
                    {
                        break;
                    }
                    state.children.push(child);
                    match next {
                        Some(head) => state.head = head,
                        None => complete = true,
                    }
                }
                UpStep::Branches | UpStep::Unreadable => break,
            }
        }
        if !resumable {
            save(None);
        }
        let value = assemble(&state.down, &state.children, complete);
        let successful = value != PrStack::Unknown;
        Measurement {
            value,
            observed_at: state.observed_at,
            valid_until: if successful {
                state.valid_until
            } else {
                Instant::now() + FAILURE_TTL
            },
            successful,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_capacity_does_not_evict_pinned_leaders() {
        let continuations = Continuations::default();
        let key = |number| {
            (
                "synthetic/repo".into(),
                number,
                "head".into(),
                "main".into(),
            )
        };
        let mut pinned: Vec<_> = (1..=512)
            .map(|number| continuations.slot(key(number)).unwrap())
            .collect();
        assert!(continuations.slot(key(513)).is_none());
        assert!(Arc::ptr_eq(
            &pinned[0],
            &continuations.slot(key(1)).unwrap()
        ));
        pinned.pop();
        assert!(continuations.slot(key(513)).is_some());
        assert_eq!(continuations.slots.lock().unwrap().len(), 512);
        assert!(!continuations.slots.lock().unwrap().contains_key(&key(512)));
    }

    /// A `PR_STACK_QUERY` response for pull request 30 based on `feat-b`,
    /// with `chain` as the nested `baseRef` answer.
    fn down_response(base: &str, chain: Value) -> Value {
        json!({
            "repository": {
                "defaultBranchRef": { "name": "main" },
                "pullRequest": {
                    "number": 30,
                    "headRefName": "feat-c",
                    "baseRefName": base,
                    "isCrossRepository": false,
                    "stackEntry": null,
                    "baseRef": chain
                }
            }
        })
    }

    fn one(number: u64, base: &str, deeper: Option<Value>) -> Value {
        let mut node = json!({ "number": number, "baseRefName": base });
        if let Some(d) = deeper {
            node["baseRef"] = d;
        }
        json!({ "associatedPullRequests": { "nodes": [node] } })
    }

    fn none() -> Value {
        json!({ "associatedPullRequests": { "nodes": [] } })
    }

    /// The reported case: the parent is NOT in any list the app holds, and
    /// the pull request is still found to be stacked -- because the answer
    /// comes from GitHub's graph, not from the rows on screen.
    #[test]
    fn native_members_keep_readiness_facts_and_do_not_default_absent_to_ready() {
        let (members, complete) = parse_members(&json!({ "totalCount": 2, "nodes": [
            {"position": 1, "pullRequest": {"number": 10, "title": "Base", "state": "OPEN", "isDraft": true,
              "reviewDecision": "REVIEW_REQUIRED", "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}}},
            {"position": 2, "pullRequest": {"number": 20, "title": "Next", "state": "OPEN"}}
        ]}));
        assert!(complete);
        let facts = serde_json::to_value(&members).unwrap();
        assert_eq!(facts[0]["is_draft"], true);
        assert_eq!(facts[0]["review"], "review_required");
        assert_eq!(facts[0]["checks"], "failure");
        assert!(facts[1]["is_draft"].is_null());
        assert!(facts[1]["review"].is_null());
        assert!(facts[1]["checks"].is_null());
    }

    #[test]
    fn a_stacked_pull_request_is_found_without_its_parent_in_the_list() {
        // #30 on feat-b (#20), on feat-a (#10), on main.
        let v = down_response(
            "feat-b",
            one(20, "feat-a", Some(one(10, "main", Some(none())))),
        );
        let down = parse_down(&v).unwrap();
        assert_eq!(down.parents, vec![20, 10]);
        assert!(down.complete);
        assert_eq!(
            assemble(&down, &[40], true),
            PrStack::Stacked {
                native: false,
                stack_number: None,
                position: 3,
                size: 4,
                position_exact: true,
                size_exact: true,
                below: Some(20),
                members: vec![],
                members_complete: false,
            }
        );
    }

    /// GitHub's native entry wins, and its numbers are taken as given.
    #[test]
    fn a_native_stack_entry_is_exact() {
        let mut v = down_response("feat-a", one(10, "main", Some(none())));
        v["repository"]["pullRequest"]["stackEntry"] =
            json!({ "position": 2, "stack": { "number": 7, "size": 4 } });
        let down = parse_down(&v).unwrap();
        assert_eq!(
            assemble(&down, &[], true),
            PrStack::Stacked {
                native: true,
                stack_number: Some(7),
                position: 2,
                size: 4,
                position_exact: true,
                size_exact: true,
                below: Some(10),
                members: vec![],
                members_complete: false,
            }
        );
    }

    /// A pull request into the trunk with nothing on top is standalone --
    /// and says so only because BOTH walks finished.
    #[test]
    fn a_pull_request_into_the_trunk_with_nothing_above_is_not_stacked() {
        let v = down_response("main", none());
        let down = parse_down(&v).unwrap();
        assert!(down.complete && down.parents.is_empty());
        assert_eq!(assemble(&down, &[], true), PrStack::None);
        // The upward walk failed: that is not evidence of standalone.
        assert_eq!(assemble(&down, &[], false), PrStack::Unknown);
    }

    /// The bottom of a stack is in one, at position 1 -- and has nothing
    /// beneath it to merge first.
    #[test]
    fn the_bottom_of_a_stack_is_position_one() {
        let down = parse_down(&down_response("main", none())).unwrap();
        match assemble(&down, &[31, 32], true) {
            PrStack::Stacked {
                position,
                size,
                below,
                ..
            } => {
                assert_eq!((position, size, below), (1, 3, None));
            }
            other => panic!("expected stacked, got {other:?}"),
        }
    }

    /// Four hops without reaching the trunk: the position is a FLOOR.
    #[test]
    fn a_walk_that_runs_out_of_hops_is_qualified() {
        // The deepest level has no `baseRef` key, as the query selects none.
        let deepest = one(3, "feat-x", None);
        let chain = one(
            6,
            "f5",
            Some(one(5, "f4", Some(one(4, "f3", Some(deepest))))),
        );
        let down = parse_down(&down_response("f6", chain)).unwrap();
        assert_eq!(down.parents, vec![6, 5, 4, 3]);
        assert!(
            !down.complete,
            "ran out of hops, so the bottom was not seen"
        );
        match assemble(&down, &[], true) {
            PrStack::Stacked {
                position,
                position_exact,
                size_exact,
                ..
            } => {
                assert_eq!(position, 5);
                assert!(!position_exact && !size_exact);
            }
            other => panic!("expected stacked, got {other:?}"),
        }
    }

    /// ...but reaching the trunk AT the deepest level is certain.
    #[test]
    fn the_deepest_hop_landing_on_the_trunk_is_complete() {
        let chain = one(
            6,
            "f5",
            Some(one(5, "f4", Some(one(4, "f3", Some(one(3, "main", None)))))),
        );
        let down = parse_down(&down_response("f6", chain)).unwrap();
        assert!(down.complete);
    }

    /// Two open pull requests from the base branch: stacked on SOMETHING,
    /// position a floor, and no single parent named.
    #[test]
    fn an_ambiguous_base_is_stacked_with_no_named_parent() {
        let chain = json!({ "associatedPullRequests": { "nodes": [
            { "number": 11, "baseRefName": "main" },
            { "number": 12, "baseRefName": "main" }
        ] } });
        let down = parse_down(&down_response("shared", chain)).unwrap();
        assert!(down.ambiguous && !down.complete);
        match assemble(&down, &[], true) {
            PrStack::Stacked {
                position,
                position_exact,
                below,
                ..
            } => {
                assert_eq!((position, position_exact, below), (2, false, None));
            }
            other => panic!("expected stacked, got {other:?}"),
        }
    }

    /// An upward walk cut short keeps the hops it found and qualifies the
    /// total, rather than discarding them (#1044).
    #[test]
    fn an_incomplete_upward_walk_keeps_what_it_found() {
        let down = parse_down(&down_response("feat-b", one(20, "main", Some(none())))).unwrap();
        match assemble(&down, &[40, 50], false) {
            PrStack::Stacked {
                position,
                size,
                position_exact,
                size_exact,
                ..
            } => {
                assert_eq!((position, size), (2, 4));
                assert!(position_exact, "the downward walk finished");
                assert!(!size_exact, "the upward one did not");
            }
            other => panic!("expected stacked, got {other:?}"),
        }
    }

    /// A missing pull request is "could not tell", not "not stacked".
    #[test]
    fn a_response_without_the_pull_request_is_unknown() {
        assert_eq!(
            parse_down(&json!({ "repository": { "pullRequest": null } })),
            None
        );
    }

    #[test]
    fn the_upward_step_reads_top_child_fork_and_branches() {
        let page = |nodes: Value| json!({ "repository": { "pullRequests": { "nodes": nodes } } });
        assert_eq!(parse_up(&page(json!([]))), UpStep::Top);
        assert_eq!(
            parse_up(&page(
                json!([{ "number": 40, "headRefName": "feat-d", "isCrossRepository": false }])
            )),
            UpStep::Child {
                number: 40,
                head: Some("feat-d".into())
            }
        );
        assert_eq!(
            parse_up(&page(
                json!([{ "number": 41, "headRefName": "patch-1", "isCrossRepository": true }])
            )),
            UpStep::Child {
                number: 41,
                head: None
            }
        );
        assert_eq!(
            parse_up(&page(json!([{ "number": 40 }, { "number": 41 }]))),
            UpStep::Branches
        );
        assert_eq!(parse_up(&json!({})), UpStep::Unreadable);
    }

    mod live {
        use super::*;
        use wiremock::matchers::{body_string_contains, method, path};
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

        async fn answer(server: &MockServer, needle: &str, data: Value) {
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_string_contains(needle))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data })))
                .mount(server)
                .await;
        }

        fn up(nodes: Value) -> Value {
            json!({ "repository": { "pullRequests": { "nodes": nodes } } })
        }

        /// Both walks, driven through the real requests: #30 is on #20
        /// (on the trunk), and #40 then #50 are on #30.
        #[tokio::test]
        async fn the_walk_finds_position_and_size_from_both_directions() {
            let server = MockServer::start().await;
            answer(
                &server,
                "PrStack(",
                down_response("feat-b", one(20, "main", Some(none()))),
            )
            .await;
            answer(
                &server,
                r#""base":"feat-c""#,
                up(json!([{ "number": 40, "headRefName": "feat-d", "isCrossRepository": false }])),
            )
            .await;
            answer(
                &server,
                r#""base":"feat-d""#,
                up(json!([{ "number": 50, "headRefName": "feat-e", "isCrossRepository": false }])),
            )
            .await;
            answer(&server, r#""base":"feat-e""#, up(json!([]))).await;

            let got = client_for(&server)
                .await
                .fetch_pr_stack("acme", "widgets", 30)
                .await;
            assert_eq!(
                got,
                PrStack::Stacked {
                    native: false,
                    stack_number: None,
                    position: 2,
                    size: 4,
                    position_exact: true,
                    size_exact: true,
                    below: Some(20),
                    members: vec![],
                    members_complete: false,
                }
            );
        }

        /// An upward hop that fails keeps the hops before it: the total is
        /// qualified, not thrown away (#1044).
        #[tokio::test]
        async fn a_failed_upward_hop_qualifies_rather_than_discards() {
            let server = MockServer::start().await;
            answer(
                &server,
                "PrStack(",
                down_response("feat-b", one(20, "main", Some(none()))),
            )
            .await;
            answer(
                &server,
                r#""base":"feat-c""#,
                up(json!([{ "number": 40, "headRefName": "feat-d", "isCrossRepository": false }])),
            )
            .await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_string_contains(r#""base":"feat-d""#))
                .respond_with(ResponseTemplate::new(401))
                .mount(&server)
                .await;

            let got = client_for(&server)
                .await
                .fetch_pr_stack("acme", "widgets", 30)
                .await;
            match got {
                PrStack::Stacked {
                    position,
                    size,
                    position_exact,
                    size_exact,
                    ..
                } => {
                    assert_eq!((position, size), (2, 3));
                    assert!(position_exact);
                    assert!(!size_exact, "the walk up did not finish");
                }
                other => panic!("expected a qualified stack, got {other:?}"),
            }
        }

        /// A failed stack lookup is Unknown -- never "not stacked".
        #[tokio::test]
        async fn a_failed_lookup_is_unknown() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(ResponseTemplate::new(401))
                .mount(&server)
                .await;
            let got = client_for(&server)
                .await
                .fetch_pr_stack("acme", "widgets", 30)
                .await;
            assert_eq!(got, PrStack::Unknown);
        }
    }

    /// A native stack's membership is read bottom first, and only a list
    /// matching GitHub's `totalCount` is complete -- the stack-merge
    /// confirmation (#1468) is built from it and must not understate.
    #[test]
    fn a_native_stacks_members_are_read_and_checked_for_completeness() {
        let mut v = down_response("feat-b", one(20, "main", Some(none())));
        v["repository"]["pullRequest"]["stackEntry"] = json!({
            "position": 2,
            "stack": { "number": 7, "size": 3, "entries": { "totalCount": 3, "nodes": [
                { "position": 2, "pullRequest": { "number": 30, "title": "Second", "state": "OPEN" } },
                { "position": 1, "pullRequest": { "number": 20, "title": "First", "state": "MERGED" } },
                { "position": 3, "pullRequest": { "number": 40, "title": "Third", "state": "OPEN" } }
            ] } }
        });
        let down = parse_down(&v).unwrap();
        assert!(down.members_complete);
        assert_eq!(
            down.members
                .iter()
                .map(|m| (m.number, m.state.as_str()))
                .collect::<Vec<_>>(),
            vec![(20, "merged"), (30, "open"), (40, "open")]
        );

        // One entry GitHub could not describe: not complete.
        v["repository"]["pullRequest"]["stackEntry"]["stack"]["entries"]["nodes"][2]
            ["pullRequest"] = Value::Null;
        assert!(!parse_down(&v).unwrap().members_complete);
        // A page shorter than the total: not complete.
        v["repository"]["pullRequest"]["stackEntry"]["stack"]["entries"] =
            json!({ "totalCount": 60, "nodes": [] });
        assert!(!parse_down(&v).unwrap().members_complete);
    }

    /// A payload cached before #1452 carries no `stack`, and must read as
    /// Unknown -- never as "not stacked", which would re-offer the queue.
    #[test]
    fn an_old_payload_without_a_stack_is_unknown() {
        let mut v = serde_json::to_value(super::super::model::PrDetail::default()).unwrap();
        v.as_object_mut().unwrap().remove("stack");
        let d: super::super::model::PrDetail = serde_json::from_value(v).unwrap();
        assert_eq!(d.stack, PrStack::Unknown);
    }
}
