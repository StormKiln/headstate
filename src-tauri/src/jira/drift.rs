//! The drift rules that are raised as FACT, for desktop notifications
//! (#2043).
//!
//! The rule table itself lives in the frontend (`src/lib/jira/drift.ts`),
//! which shows every level: facts, qualified statements, the grace wait and
//! the some-merged count. A notification is only ever for a fact, and it
//! must be decided while the window is closed, so the two fact rows it can
//! announce -- R1 "open, ticket still To Do" and R2 "merged, ticket not
//! moved" -- are decided here too. `fixtures/drift_facts.json` is read by
//! both test suites, so the two sides cannot disagree about what is a fact
//! without one of them failing.

use super::keys::FoundIn;
use super::lookup::{KeyLookup, Search};
use super::model::{
    ExpectedCategory, JiraIssue, JiraLink, JiraStatus, LinkTarget, PrRole, PrState, StatusCategory,
    StatusExpectation, StatusExpectations,
};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap};

/// Expectations by project key, as `UiPrefs::jira_status_expectations`.
pub type ExpectationMap = BTreeMap<String, StatusExpectations>;

const DEFAULT_ON_OPEN: StatusExpectation = StatusExpectation::Category {
    category: ExpectedCategory::InProgress,
};
const DEFAULT_ON_MERGED: StatusExpectation = StatusExpectation::Category {
    category: ExpectedCategory::Done,
};

/// What became of one pull request, as far as it is known. `Unknown` is
/// one of the user's own that left its list with no search to say what
/// became of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    Open,
    Merged,
    Closed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPr {
    pub url: String,
    pub fate: Fate,
    pub draft: bool,
    pub merged_at: Option<String>,
    /// The user's own (authored) pull request, which is what a
    /// notification is about.
    pub authored: bool,
}

fn linked(f: FoundIn) -> bool {
    // Every `FoundIn` is a title or a branch today; matched exhaustively so
    // a new source (a body mention, #2035) must decide here whether it
    // drives a rule.
    match f {
        FoundIn::Title | FoundIn::Branch => true,
    }
}

/// The pull requests a lookup knows for its key, one per URL. Mirrors
/// `knownPrs` in `drift.ts`.
pub fn known_prs(links: &[JiraLink]) -> Vec<KnownPr> {
    let mut searched: HashMap<&str, (Option<PrState>, bool, Option<&String>)> = HashMap::new();
    for l in links {
        if let LinkTarget::Pr {
            role: PrRole::Searched,
            url,
            state,
            is_draft,
            merged_at,
            ..
        } = &l.target
        {
            searched.insert(url.as_str(), (*state, *is_draft, merged_at.as_ref()));
        }
    }
    let mut out: Vec<KnownPr> = Vec::new();
    for l in links {
        let LinkTarget::Pr {
            role,
            url,
            is_draft,
            ..
        } = &l.target
        else {
            continue;
        };
        if !linked(l.found_in) {
            continue;
        }
        let listed = *role != PrRole::Searched && l.current;
        let s = searched.get(url.as_str());
        let fate = if listed {
            Fate::Open
        } else {
            match s.and_then(|s| s.0) {
                Some(PrState::Open) => Fate::Open,
                Some(PrState::Merged) => Fate::Merged,
                Some(PrState::Closed) => Fate::Closed,
                None => Fate::Unknown,
            }
        };
        let authored = *role == PrRole::Authored;
        let pr = KnownPr {
            url: url.clone(),
            fate,
            draft: if listed {
                *is_draft
            } else {
                s.map_or(*is_draft, |s| s.1)
            },
            merged_at: (fate == Fate::Merged)
                .then(|| s.and_then(|s| s.2.cloned()))
                .flatten(),
            authored,
        };
        match out.iter_mut().find(|p| p.url == *url) {
            // Authored and reviewing can both hold one pull request: open
            // wins, and authorship is kept whichever row came first.
            Some(prior) => {
                prior.authored |= authored;
                if prior.fate != Fate::Open && listed {
                    let keep = prior.authored;
                    *prior = pr;
                    prior.authored = keep;
                }
            }
            None => out.push(pr),
        }
    }
    out
}

/// Whether a lookup asked everywhere it could. Mirrors `lookupComplete`.
pub fn lookup_complete(l: &KeyLookup) -> bool {
    let whole = |s: &Search| matches!(s, Search::Asked { truncated: false, unsearched, .. } if unsearched.is_empty());
    whole(&l.search) && l.gitlab.as_ref().is_none_or(whole)
}

/// Whether `status` meets `e`; `None` when it cannot be told. Mirrors
/// `meets` in `drift.ts`.
fn meets(status: &JiraStatus, e: &StatusExpectation) -> Option<bool> {
    match e {
        StatusExpectation::Category { category } => {
            let want = match category {
                ExpectedCategory::New => StatusCategory::New,
                ExpectedCategory::InProgress => StatusCategory::InProgress,
                ExpectedCategory::Done => StatusCategory::Done,
            };
            (status.category != StatusCategory::Unknown).then(|| status.category == want)
        }
        StatusExpectation::Status { name } => {
            (!status.name.is_empty()).then(|| status.name == *name)
        }
    }
}

fn expectations<'a>(issue: &JiraIssue, map: &'a ExpectationMap) -> Option<&'a StatusExpectations> {
    map.get(&issue.project_key)
}

/// R1 as a fact: an open, non-draft pull request whose ticket is still in
/// the To Do category and not what the project expects while open.
pub fn not_started(issue: &JiraIssue, draft: bool, map: &ExpectationMap) -> bool {
    let on_open = expectations(issue, map)
        .and_then(|e| e.on_open.as_ref())
        .unwrap_or(&DEFAULT_ON_OPEN);
    !draft
        && issue.status.category == StatusCategory::New
        && meets(&issue.status, on_open) == Some(false)
}

/// Whether the ticket's status, as read, is not where a merge should leave
/// it -- by default, not done. `false` when that cannot be told.
pub fn left_behind(issue: &JiraIssue, map: &ExpectationMap) -> bool {
    let on_merged = expectations(issue, map)
        .and_then(|e| e.on_merged.as_ref())
        .unwrap_or(&DEFAULT_ON_MERGED);
    issue.status.category != StatusCategory::Unknown
        && meets(&issue.status, on_merged) == Some(false)
}

/// R2 as a fact: the lookup asked everywhere, every pull request's fate is
/// known, none is open, one merged, the ticket is not where a merge should
/// leave it, and the grace period after the latest merge is over -- timed
/// from merge times that were all reported.
pub fn merged_not_moved(
    issue: &JiraIssue,
    prs: &[KnownPr],
    complete: bool,
    map: &ExpectationMap,
    now: DateTime<Utc>,
    grace: Duration,
) -> bool {
    if !complete || issue.status.category == StatusCategory::Unknown {
        return false;
    }
    let count = |f: Fate| prs.iter().filter(|p| p.fate == f).count();
    if count(Fate::Open) > 0 || count(Fate::Unknown) > 0 || count(Fate::Merged) == 0 {
        return false;
    }
    if !left_behind(issue, map) {
        return false;
    }
    let times: Option<Vec<DateTime<Utc>>> = prs
        .iter()
        .filter(|p| p.fate == Fate::Merged)
        .map(|p| {
            DateTime::parse_from_rfc3339(p.merged_at.as_deref()?)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        })
        .collect();
    match times.and_then(|t| t.into_iter().max()) {
        Some(latest) => now - latest >= grace,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Source;
    use crate::jira::model::JiraIssueType;
    use serde::Deserialize;

    fn issue(category: StatusCategory, name: &str) -> JiraIssue {
        JiraIssue {
            key: "ABC-1".into(),
            id: None,
            summary: "s".into(),
            issue_type: JiraIssueType {
                name: "Story".into(),
                hierarchy_level: Some(0),
                icon_url: None,
            },
            status: JiraStatus {
                name: name.into(),
                category,
            },
            assignee: None,
            parent_key: None,
            parent_checked: true,
            project_key: "ABC".into(),
            url: None,
            updated: None,
            read_at: None,
            stale: false,
        }
    }

    #[derive(Deserialize)]
    struct Fixtures {
        r1: Vec<R1Case>,
        r2: Vec<R2Case>,
    }
    #[derive(Deserialize)]
    struct R1Case {
        name: String,
        category: StatusCategory,
        status: String,
        draft: bool,
        #[serde(default)]
        expectations: ExpectationMap,
        fact: bool,
    }
    #[derive(Deserialize)]
    struct R2Case {
        name: String,
        category: StatusCategory,
        status: String,
        prs: Vec<FixturePr>,
        complete: bool,
        now: String,
        grace_mins: i64,
        #[serde(default)]
        expectations: ExpectationMap,
        fact: bool,
    }
    #[derive(Deserialize)]
    struct FixturePr {
        fate: String,
        #[serde(default)]
        merged_at: Option<String>,
    }

    fn fixtures() -> Fixtures {
        serde_json::from_str(include_str!("fixtures/drift_facts.json")).unwrap()
    }

    /// The shared table: `drift.test.ts` reads the same file.
    #[test]
    fn r1_facts_match_the_shared_table() {
        for c in fixtures().r1 {
            assert_eq!(
                not_started(&issue(c.category, &c.status), c.draft, &c.expectations),
                c.fact,
                "{}",
                c.name
            );
        }
    }

    #[test]
    fn r2_facts_match_the_shared_table() {
        for c in fixtures().r2 {
            let prs: Vec<KnownPr> = c
                .prs
                .iter()
                .enumerate()
                .map(|(i, p)| KnownPr {
                    url: format!("u{i}"),
                    fate: match p.fate.as_str() {
                        "open" => Fate::Open,
                        "merged" => Fate::Merged,
                        "closed" => Fate::Closed,
                        _ => Fate::Unknown,
                    },
                    draft: false,
                    merged_at: p.merged_at.clone(),
                    authored: true,
                })
                .collect();
            let now = DateTime::parse_from_rfc3339(&c.now)
                .unwrap()
                .with_timezone(&Utc);
            assert_eq!(
                merged_not_moved(
                    &issue(c.category, &c.status),
                    &prs,
                    c.complete,
                    &c.expectations,
                    now,
                    Duration::minutes(c.grace_mins),
                ),
                c.fact,
                "{}",
                c.name
            );
        }
    }

    fn pr_link(role: PrRole, current: bool, state: Option<PrState>) -> JiraLink {
        JiraLink {
            key: "ABC-1".into(),
            found_in: FoundIn::Title,
            current,
            target: LinkTarget::Pr {
                source: Source::default(),
                role,
                repo: "octocat/hello-world".into(),
                number: 1,
                title: "ABC-1".into(),
                url: "https://github.com/octocat/hello-world/pull/1".into(),
                head_ref: "x".into(),
                is_draft: false,
                state,
                author: None,
                merged_at: (state == Some(PrState::Merged)).then(|| "2026-01-02T03:04:05Z".into()),
            },
        }
    }

    #[test]
    fn known_prs_takes_a_fate_from_the_search_and_keeps_authorship() {
        let own = pr_link(PrRole::Authored, false, None);
        assert_eq!(known_prs(std::slice::from_ref(&own))[0].fate, Fate::Unknown);
        let found = pr_link(PrRole::Searched, false, Some(PrState::Merged));
        let got = known_prs(&[own, found]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].fate, Fate::Merged);
        assert!(got[0].authored);
        assert_eq!(got[0].merged_at.as_deref(), Some("2026-01-02T03:04:05Z"));
        // Listed is open, whatever an older search said.
        let listed = known_prs(&[
            pr_link(PrRole::Searched, false, Some(PrState::Merged)),
            pr_link(PrRole::Authored, true, None),
        ]);
        assert_eq!(listed[0].fate, Fate::Open);
        assert!(listed[0].authored);
    }

    #[test]
    fn a_lookup_is_complete_only_when_every_half_asked_everywhere() {
        let asked = |unsearched: Vec<String>, truncated| Search::Asked {
            at: "2026-01-02T03:04:05Z".into(),
            total: 1,
            read: 1,
            truncated,
            scope: vec!["octocat".into()],
            unsearched,
        };
        let l = |search, gitlab| KeyLookup {
            key: "ABC-1".into(),
            links: vec![],
            search,
            gitlab,
        };
        assert!(lookup_complete(&l(asked(vec![], false), None)));
        assert!(lookup_complete(&l(
            asked(vec![], false),
            Some(asked(vec![], false))
        )));
        assert!(!lookup_complete(&l(asked(vec![], true), None)));
        assert!(!lookup_complete(&l(
            asked(vec!["acme".into()], false),
            None
        )));
        assert!(!lookup_complete(&l(
            Search::NotAsked {
                reason: "already known from your pull requests".into()
            },
            None
        )));
        assert!(!lookup_complete(&l(
            asked(vec![], false),
            Some(Search::Failed { error: "x".into() })
        )));
    }
}
