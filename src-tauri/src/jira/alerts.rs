//! Desktop notifications for Jira drift (#2043).
//!
//! # Why here, and not in the frontend
//!
//! The page decides every drift level from what it has loaded (`drift.ts`),
//! but a notification is for when the window is closed, and then nothing
//! there runs. The background refresh ([`super::refresh`]) already reads
//! the issues the index names on its own cadence, window or not, so the
//! notifier runs at the end of each of its passes, over what that pass
//! just read. The fact rules it needs are in [`super::drift`], held to the
//! page's by a shared fixture table.
//!
//! # First observation
//!
//! A notice fires when a (rule, issue, pull request) is first seen true:
//! once per pair, not on every pass, and never for what was already true
//! when the app started or when the notification was switched on. That is
//! [`Observer`]: the first successful pass it sees is the baseline, and a
//! pass that failed is neither a baseline nor an observation -- its
//! statuses may be old.
//!
//! # What it asks
//!
//! R1 needs only the index and the issues the pass read. R2 needs every
//! pull request for the issue accounted for, which only a search says, so
//! for up to [`MAX_LOOKUPS`] issues an own pull request recently left its
//! list for, it runs the same lookup the page does ([`super::lookup::find`]):
//! budget-gated, and answered from the database within its TTL. Only while
//! the notification is on.

use super::cache::Cached;
use super::drift::{self, ExpectationMap, Fate};
use super::lookup::KeyLookup;
use super::model::{JiraLink, LinkTarget, PrRole};
use crate::identity::Source;
use crate::poll::{Breakage, NoticeKind};
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, HashSet};

/// How long after an own pull request left its list R2 is still looked
/// for. A ticket left behind for a week is old news, not an event.
pub fn recent() -> Duration {
    Duration::days(7)
}

/// Lookups one pass may run for R2.
pub const MAX_LOOKUPS: usize = 5;

/// How old an issue's status may be and still decide a notice: read by
/// this pass or the one before, never one served from an old cache.
fn fresh() -> Duration {
    Duration::from_std(super::refresh::CADENCE).unwrap_or(Duration::minutes(10))
        + Duration::minutes(5)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    NotStarted,
    MergedNotMoved,
}

/// One rule true as a fact for one issue and one of the user's own pull
/// requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    pub rule: Rule,
    pub key: String,
    pub summary: String,
    pub source: Source,
    pub repo: String,
    pub number: u64,
    pub url: String,
}

impl Firing {
    fn id(&self) -> (Rule, String, String) {
        (self.rule, self.key.clone(), self.url.clone())
    }

    pub fn breakage(&self) -> Breakage {
        Breakage {
            source: self.source.clone(),
            title: if self.summary.is_empty() {
                self.key.clone()
            } else {
                format!("{} {}", self.key, self.summary)
            },
            repo: self.repo.clone(),
            number: self.number,
            url: self.url.clone(),
            kind: match self.rule {
                Rule::NotStarted => NoticeKind::JiraNotStarted,
                Rule::MergedNotMoved => NoticeKind::JiraNotMoved,
            },
            jira_key: Some(self.key.clone()),
        }
    }
}

/// What has been seen true, so each pair is announced once.
#[derive(Debug, Default)]
pub struct Observer {
    baseline: bool,
    seen: HashSet<(Rule, String, String)>,
}

impl Observer {
    /// The firings not seen before. The first call is the baseline and
    /// returns nothing: those were true before anyone was watching.
    pub fn first_observations(&mut self, now: Vec<Firing>) -> Vec<Firing> {
        if !self.baseline {
            self.baseline = true;
            self.seen.extend(now.iter().map(Firing::id));
            return Vec::new();
        }
        now.into_iter()
            .filter(|f| self.seen.insert(f.id()))
            .collect()
    }
}

fn usable<'a>(
    issues: &'a HashMap<String, Cached>,
    key: &str,
    now: DateTime<Utc>,
) -> Option<&'a Cached> {
    issues.get(key).filter(|c| now - c.read_at <= fresh())
}

fn pr_of(l: &JiraLink) -> Option<(&Source, &String, u64, &String)> {
    match &l.target {
        LinkTarget::Pr {
            source,
            repo,
            number,
            url,
            ..
        } => Some((source, repo, *number, url)),
        _ => None,
    }
}

/// R1 as fact, for the user's own open pull requests.
pub fn open_facts(
    links: &[JiraLink],
    issues: &HashMap<String, Cached>,
    map: &ExpectationMap,
    now: DateTime<Utc>,
) -> Vec<Firing> {
    links
        .iter()
        .filter(|l| l.current)
        .filter_map(|l| {
            let LinkTarget::Pr {
                role: PrRole::Authored,
                is_draft,
                ..
            } = &l.target
            else {
                return None;
            };
            let c = usable(issues, &l.key, now)?;
            if !drift::not_started(&c.issue, *is_draft, map) {
                return None;
            }
            let (source, repo, number, url) = pr_of(l)?;
            Some(Firing {
                rule: Rule::NotStarted,
                key: l.key.clone(),
                summary: c.issue.summary.clone(),
                source: source.clone(),
                repo: repo.clone(),
                number,
                url: url.clone(),
            })
        })
        .collect()
}

/// R2 as fact, from lookups. One notice per issue, for the user's own
/// pull request that merged last.
pub fn merged_facts(
    lookups: &[KeyLookup],
    issues: &HashMap<String, Cached>,
    map: &ExpectationMap,
    now: DateTime<Utc>,
    grace: Duration,
) -> Vec<Firing> {
    lookups
        .iter()
        .filter_map(|l| {
            let c = usable(issues, &l.key, now)?;
            let prs = drift::known_prs(&l.links);
            let complete = drift::lookup_complete(l);
            if !drift::merged_not_moved(&c.issue, &prs, complete, map, now, grace) {
                return None;
            }
            let mine = prs
                .iter()
                .filter(|p| p.authored && p.fate == Fate::Merged)
                .max_by(|a, b| a.merged_at.cmp(&b.merged_at))?;
            let link = l.links.iter().find(|x| {
                matches!(&x.target, LinkTarget::Pr { url, role, .. }
                    if *url == mine.url && *role == PrRole::Authored)
            })?;
            let (source, repo, number, url) = pr_of(link)?;
            Some(Firing {
                rule: Rule::MergedNotMoved,
                key: l.key.clone(),
                summary: c.issue.summary.clone(),
                source: source.clone(),
                repo: repo.clone(),
                number,
                url: url.clone(),
            })
        })
        .collect()
}

/// Keys worth a lookup for R2: own pull request left its list recently,
/// none still listed, and the issue's status, as just read, not already
/// where a merge should leave it. Most recently left first.
pub fn r2_candidates(
    left: &[String],
    links: &[JiraLink],
    issues: &HashMap<String, Cached>,
    map: &ExpectationMap,
    now: DateTime<Utc>,
) -> Vec<String> {
    let still_open: HashSet<&str> = links
        .iter()
        .filter(|l| {
            l.current
                && matches!(&l.target, LinkTarget::Pr { role, .. } if *role != PrRole::Searched)
        })
        .map(|l| l.key.as_str())
        .collect();
    left.iter()
        .filter(|k| !still_open.contains(k.as_str()))
        .filter(|k| {
            // A status that already meets the expectation cannot raise R2;
            // asking would spend a search to learn nothing.
            usable(issues, k, now).is_some_and(|c| drift::left_behind(&c.issue, map))
        })
        .take(MAX_LOOKUPS)
        .cloned()
        .collect()
}

/// One sweep, after a pass that read Jira: what is true as fact now, less
/// what the observer has already seen. Reads the index and the issue cache
/// the pass just wrote, and runs at most [`MAX_LOOKUPS`] lookups for R2.
pub async fn sweep(
    client: Option<std::sync::Arc<crate::github::client::GitHubClient>>,
    glab: Option<std::path::PathBuf>,
    db: &std::path::Path,
    ui: &crate::poll::UiPrefs,
    observer: &mut Observer,
) -> Result<Vec<Firing>, String> {
    let now = Utc::now();
    let excluded: HashSet<String> = ui.jira_excluded_projects.iter().cloned().collect();
    let path = db.to_path_buf();
    let since = (now - recent()).to_rfc3339();
    let (links, left, issues) = tokio::task::spawn_blocking(move || {
        let conn = crate::store::open_db(&path).map_err(|e| e.to_string())?;
        let projects: HashSet<String> = crate::store::settings::get::<Vec<String>>(
            &conn,
            crate::store::settings::keys::JIRA_PROJECTS,
        )
        .map_err(|e| e.to_string())?
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !excluded.contains(p))
        .collect();
        let links = super::index::load(&conn, &projects).map_err(|e| e.to_string())?;
        let left =
            super::index::recently_left(&conn, &projects, &since).map_err(|e| e.to_string())?;
        let mut keys: Vec<String> = links.iter().map(|l| l.key.clone()).collect();
        keys.extend(left.iter().cloned());
        keys.sort();
        keys.dedup();
        let issues = super::cache::load(&conn, &keys).map_err(|e| e.to_string())?;
        Ok::<_, String>((links, left, issues))
    })
    .await
    .map_err(|e| e.to_string())??;
    let map: ExpectationMap = ui.jira_status_expectations.clone();
    let grace = Duration::minutes(i64::from(ui.jira_merge_grace_mins));
    let mut lookups = Vec::new();
    for key in r2_candidates(&left, &links, &issues, &map, now) {
        match super::lookup::find(client.clone(), glab.clone(), db, &key, true).await {
            Ok(l) => lookups.push(l),
            // A lookup that failed decides nothing; R2 is not raised.
            Err(e) => log::info!("jira: drift lookup for a ticket failed: {e}"),
        }
    }
    let mut firing = open_facts(&links, &issues, &map, now);
    firing.extend(merged_facts(&lookups, &issues, &map, now, grace));
    Ok(observer.first_observations(firing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jira::keys::FoundIn;
    use crate::jira::lookup::Search;
    use crate::jira::model::{JiraIssue, JiraIssueType, JiraStatus, PrState, StatusCategory};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn cached(category: StatusCategory, read_mins_ago: i64) -> Cached {
        Cached {
            issue: JiraIssue {
                key: "ABC-1".into(),
                id: None,
                summary: "Fix login".into(),
                issue_type: JiraIssueType {
                    name: "Story".into(),
                    hierarchy_level: Some(0),
                    icon_url: None,
                },
                status: JiraStatus {
                    name: "S".into(),
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
            },
            read_at: now() - Duration::minutes(read_mins_ago),
            parent_read_at: None,
        }
    }

    fn issues(c: Cached) -> HashMap<String, Cached> {
        HashMap::from([("ABC-1".to_string(), c)])
    }

    fn link(role: PrRole, current: bool, state: Option<PrState>, draft: bool) -> JiraLink {
        JiraLink {
            key: "ABC-1".into(),
            found_in: FoundIn::Branch,
            current,
            target: LinkTarget::Pr {
                source: Source::default(),
                role,
                repo: "octocat/hello-world".into(),
                number: 7,
                title: "Fix login".into(),
                url: "https://github.com/octocat/hello-world/pull/7".into(),
                head_ref: "abc-1-fix".into(),
                is_draft: draft,
                state,
                author: None,
                merged_at: (state == Some(PrState::Merged)).then(|| "2026-01-02T11:00:00Z".into()),
            },
        }
    }

    fn whole() -> Search {
        Search::Asked {
            at: "2026-01-02T11:59:00Z".into(),
            total: 1,
            read: 1,
            truncated: false,
            scope: vec!["octocat".into()],
            unsearched: vec![],
        }
    }

    fn lookup(search: Search) -> KeyLookup {
        KeyLookup {
            key: "ABC-1".into(),
            links: vec![
                link(PrRole::Authored, false, None, false),
                link(PrRole::Searched, false, Some(PrState::Merged), false),
            ],
            search,
            gitlab: None,
        }
    }

    #[test]
    fn r1_fires_for_your_open_pull_request_on_a_to_do_ticket() {
        let open = [link(PrRole::Authored, true, None, false)];
        let got = open_facts(
            &open,
            &issues(cached(StatusCategory::New, 1)),
            &ExpectationMap::new(),
            now(),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].breakage().kind, NoticeKind::JiraNotStarted);
        assert_eq!(got[0].breakage().jira_key.as_deref(), Some("ABC-1"));
        assert_eq!(got[0].breakage().title, "ABC-1 Fix login");
    }

    #[test]
    fn r1_does_not_fire_for_a_draft_a_review_an_old_status_or_progress() {
        let m = ExpectationMap::new();
        let to_do = issues(cached(StatusCategory::New, 1));
        assert!(open_facts(
            &[link(PrRole::Authored, true, None, true)],
            &to_do,
            &m,
            now()
        )
        .is_empty());
        assert!(open_facts(
            &[link(PrRole::Reviewing, true, None, false)],
            &to_do,
            &m,
            now()
        )
        .is_empty());
        assert!(open_facts(
            &[link(PrRole::Authored, false, None, false)],
            &to_do,
            &m,
            now()
        )
        .is_empty());
        let old = issues(cached(StatusCategory::New, 60));
        assert!(
            open_facts(
                &[link(PrRole::Authored, true, None, false)],
                &old,
                &m,
                now()
            )
            .is_empty(),
            "a status read an hour ago does not decide a notice"
        );
        let doing = issues(cached(StatusCategory::InProgress, 1));
        assert!(open_facts(
            &[link(PrRole::Authored, true, None, false)],
            &doing,
            &m,
            now()
        )
        .is_empty());
        let unknown = issues(cached(StatusCategory::Unknown, 1));
        assert!(open_facts(
            &[link(PrRole::Authored, true, None, false)],
            &unknown,
            &m,
            now()
        )
        .is_empty());
    }

    #[test]
    fn r2_fires_only_as_fact() {
        let m = ExpectationMap::new();
        let doing = issues(cached(StatusCategory::InProgress, 1));
        let g = Duration::minutes(15);
        let got = merged_facts(&[lookup(whole())], &doing, &m, now(), g);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].breakage().kind, NoticeKind::JiraNotMoved);
        // Qualified states never notify: a partial search, or one not made.
        let partial = Search::Asked {
            at: "2026-01-02T11:59:00Z".into(),
            total: 30,
            read: 20,
            truncated: true,
            scope: vec!["octocat".into()],
            unsearched: vec![],
        };
        assert!(merged_facts(&[lookup(partial)], &doing, &m, now(), g).is_empty());
        let not_asked = Search::NotAsked {
            reason: "not signed in to GitHub".into(),
        };
        assert!(merged_facts(&[lookup(not_asked)], &doing, &m, now(), g).is_empty());
        // Within the grace period.
        assert!(merged_facts(&[lookup(whole())], &doing, &m, now(), Duration::hours(2)).is_empty());
        let done = issues(cached(StatusCategory::Done, 1));
        assert!(merged_facts(&[lookup(whole())], &done, &m, now(), g).is_empty());
    }

    #[test]
    fn r2_is_about_your_own_pull_request() {
        let mut l = lookup(whole());
        l.links.retain(|x| {
            matches!(
                x.target,
                LinkTarget::Pr {
                    role: PrRole::Searched,
                    ..
                }
            )
        });
        let doing = issues(cached(StatusCategory::InProgress, 1));
        assert!(
            merged_facts(
                &[l],
                &doing,
                &ExpectationMap::new(),
                now(),
                Duration::minutes(15)
            )
            .is_empty(),
            "a teammate's merge is shown on the page, not announced"
        );
    }

    #[test]
    fn each_pair_is_announced_once_and_never_what_was_true_at_the_start() {
        let open = [link(PrRole::Authored, true, None, false)];
        let m = ExpectationMap::new();
        let fire = || open_facts(&open, &issues(cached(StatusCategory::New, 1)), &m, now());
        let mut o = Observer::default();
        assert!(o.first_observations(fire()).is_empty(), "true at launch");
        assert!(o.first_observations(fire()).is_empty());

        let mut o = Observer::default();
        assert!(o.first_observations(Vec::new()).is_empty());
        assert_eq!(o.first_observations(fire()).len(), 1, "became true");
        assert!(o.first_observations(fire()).is_empty(), "not per poll");
        assert!(o.first_observations(Vec::new()).is_empty());
        assert!(
            o.first_observations(fire()).is_empty(),
            "nor again after a flap"
        );
    }

    #[test]
    fn r2_candidates_skip_open_work_and_settled_tickets() {
        let m = ExpectationMap::new();
        let left = vec!["ABC-1".to_string()];
        let doing = issues(cached(StatusCategory::InProgress, 1));
        assert_eq!(r2_candidates(&left, &[], &doing, &m, now()), left);
        let open = [link(PrRole::Authored, true, None, false)];
        assert!(r2_candidates(&left, &open, &doing, &m, now()).is_empty());
        let done = issues(cached(StatusCategory::Done, 1));
        assert!(r2_candidates(&left, &[], &done, &m, now()).is_empty());
        let old = issues(cached(StatusCategory::InProgress, 60));
        assert!(r2_candidates(&left, &[], &old, &m, now()).is_empty());
        let many: Vec<String> = (0..9).map(|i| format!("ABC-{i}")).collect();
        let all: HashMap<String, Cached> = many
            .iter()
            .map(|k| (k.clone(), cached(StatusCategory::InProgress, 1)))
            .collect();
        assert_eq!(
            r2_candidates(&many, &[], &all, &m, now()).len(),
            MAX_LOOKUPS
        );
    }
}
