//! Finding the pull requests that name a Jira key (#2041, #2040).
//!
//! The database first: the link index already holds every pull request
//! from the user's lists that names the key, including ones that have since
//! left those lists. GitHub and GitLab are asked only when that holds none
//! -- or when the user asks for more -- and then only across what the user
//! already works in: the owners of their GitHub repositories, and the
//! GitLab projects in their merge-request lists. Never all of either. What
//! a search finds is stored, so the next look is the database again.
//!
//! Each half reports what it asked -- the owners or projects searched and
//! those left out -- so "nothing found" is never wider than the search
//! that found it (#1050).

use super::index;
use super::model::{JiraLink, LinkTarget, PrRole, PrState};
use crate::github::client::GitHubClient;
use crate::identity::{Provider, Source};
use crate::store::{open_db, StoreError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How long one search of a key stands before it is asked again.
const SEARCH_TTL: Duration = Duration::minutes(15);
/// Pull requests one GitHub search returns, and merge requests one GitLab
/// project returns. Past this the answer is reported as "at least".
pub const SEARCH_LIMIT: usize = 20;
/// Owners one GitHub search spans. GitHub refuses queries past five boolean
/// operators, and each `user:` beyond the first is an OR.
const MAX_OWNERS: usize = 5;
/// GitLab projects one lookup searches: one request each.
const MAX_PROJECTS: usize = 5;
/// What one search costs, for the reserve check: a single search node.
const SEARCH_COST: u64 = 1;
const SEARCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

const ALREADY_KNOWN: &str = "already known from your pull requests";

/// What happened to one provider's half of a lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Search {
    /// Not asked, and why -- "the database already knew", "no GitHub
    /// sign-in", "the reserve is held for the poll loop". Never "found
    /// nothing" (#1050).
    NotAsked {
        reason: String,
    },
    /// Asked, at `at`, across `scope` -- GitHub owners, or GitLab projects
    /// as `host/path`. `unsearched` were candidates left out: past the cap,
    /// or a project that did not answer. `total` is what the provider
    /// counted, `read` how many results were read; `truncated` when more
    /// matched than were read. When the provider did not count, `total`
    /// equals `read` and `truncated` says whether that is only a floor.
    Asked {
        at: String,
        total: u64,
        read: u64,
        truncated: bool,
        scope: Vec<String>,
        unsearched: Vec<String>,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyLookup {
    pub key: String,
    pub links: Vec<JiraLink>,
    /// The GitHub half.
    pub search: Search,
    /// The GitLab half. `None` when the user has no GitLab lists at all,
    /// so there was nothing of theirs to search -- not a search that found
    /// nothing.
    #[serde(default)]
    pub gitlab: Option<Search>,
}

/// One stored search: the `Asked` half, kept per key and provider.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Searched {
    at: DateTime<Utc>,
    total: u64,
    read: u64,
    truncated: bool,
    scope: Vec<String>,
    unsearched: Vec<String>,
}

impl Searched {
    fn as_search(&self) -> Search {
        Search::Asked {
            at: self.at.to_rfc3339(),
            total: self.total,
            read: self.read,
            truncated: self.truncated,
            scope: self.scope.clone(),
            unsearched: self.unsearched.clone(),
        }
    }
}

fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Github => "github",
        Provider::Gitlab => "gitlab",
    }
}

/// The last search for `key` on `provider`.
fn last_search(
    conn: &Connection,
    key: &str,
    provider: Provider,
) -> Result<Option<Searched>, StoreError> {
    type Row = (String, i64, i64, bool, String, String);
    let row: Option<Row> = conn
        .query_row(
            "SELECT searched_at, total, read, truncated, scope, unsearched
             FROM jira_key_search WHERE key = ?1 AND provider = ?2",
            params![key, provider_name(provider)],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    Ok(
        row.and_then(|(at, total, read, truncated, scope, unsearched)| {
            Some(Searched {
                at: DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Utc),
                total: total.max(0) as u64,
                read: read.max(0) as u64,
                truncated,
                // A row whose scope cannot be read cannot say what it covered:
                // it is treated as no search, and asked again.
                scope: serde_json::from_str(&scope).ok()?,
                unsearched: serde_json::from_str(&unsearched).ok()?,
            })
        }),
    )
}

/// Rank names by how often they occur, most first, ties by name.
fn ranked(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in names {
        *count.entry(n).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = count.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.into_iter().map(|(n, _)| n).collect()
}

/// The owners of the GitHub repositories in the user's own lists, most
/// pull requests first. All of them: the caller searches the first
/// [`MAX_OWNERS`] and reports the rest as unsearched.
fn owners(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let repos: Vec<String> = conn
        .prepare(
            "SELECT json_extract(value, '$.repo') FROM snapshot, json_each(snapshot.payload)
             WHERE snapshot.provider = 'github' AND json_valid(snapshot.payload)",
        )?
        .query_map([], |r| r.get::<_, Option<String>>(0))?
        .filter_map(|r| r.transpose())
        .collect::<Result<_, _>>()?;
    Ok(ranked(repos.into_iter().filter_map(|repo| {
        repo.split_once('/').map(|(owner, _)| owner.to_string())
    })))
}

/// What the GitLab half may search.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GitLabScope {
    /// No GitLab lists at all.
    Unused,
    /// The saved host cannot be read, so no project can be asked.
    NoHost,
    /// Projects on the configured host, most merge requests first, and
    /// projects in lists from another host, which are not searched.
    On {
        host: String,
        projects: Vec<String>,
        elsewhere: Vec<String>,
    },
}

/// The GitLab projects in the user's own merge-request lists.
fn gitlab_scope(conn: &Connection) -> Result<GitLabScope, StoreError> {
    let rows: Vec<(String, String)> = conn
        .prepare(
            "SELECT snapshot.host, json_extract(value, '$.repo')
             FROM snapshot, json_each(snapshot.payload)
             WHERE snapshot.provider = 'gitlab' AND json_valid(snapshot.payload)",
        )?
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .filter_map(|r| match r {
            Ok((h, Some(p))) => Some(Ok((h, p))),
            Ok((_, None)) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Ok(GitLabScope::Unused);
    }
    let Ok(host) = crate::gitlab::host::read_host(conn) else {
        return Ok(GitLabScope::NoHost);
    };
    let (here, there): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(h, _)| *h == host);
    Ok(GitLabScope::On {
        projects: ranked(here.into_iter().map(|(_, p)| p)),
        elsewhere: ranked(there.into_iter().map(|(h, p)| format!("{h}/{p}"))),
        host,
    })
}

/// The search string. The key is quoted so GitHub matches it as a phrase;
/// `is:pr` keeps issues out.
pub fn search_query(key: &str, owners: &[String]) -> String {
    let scope = owners
        .iter()
        .map(|o| format!("user:{o}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("\"{key}\" in:title is:pr {scope}")
}

/// Pull requests from one GitHub search reply that really name `key` in
/// their title -- GitHub's phrase match is looser than the key rule -- as
/// links, with what GitHub counted and how many results were read.
pub fn map_search(v: &Value, key: &str) -> Result<(Vec<JiraLink>, u64, u64), String> {
    let search = v
        .pointer("/data/search")
        .filter(|s| s.is_object())
        .ok_or("GitHub's reply held no search result")?;
    let total = search
        .get("issueCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let nodes = search
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("GitHub's search reply held no list")?;
    let mut out = Vec::new();
    for n in nodes {
        let (Some(number), Some(title), Some(url), Some(repo)) = (
            n.get("number").and_then(Value::as_u64),
            n.get("title").and_then(Value::as_str),
            n.get("url").and_then(Value::as_str),
            n.pointer("/repository/nameWithOwner")
                .and_then(Value::as_str),
        ) else {
            continue;
        };
        let head = n
            .get("headRefName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let found = super::keys::keys_for_pr(title, Some(head));
        let Some((_, found_in)) = found.into_iter().find(|(k, _)| k == key) else {
            continue;
        };
        let state = match n.get("state").and_then(Value::as_str) {
            Some("OPEN") => PrState::Open,
            Some("MERGED") => PrState::Merged,
            Some("CLOSED") => PrState::Closed,
            // A state this build does not know is not guessed at.
            _ => continue,
        };
        out.push(JiraLink {
            key: key.to_string(),
            found_in,
            current: state == PrState::Open,
            target: LinkTarget::Pr {
                // The search runs against the signed-in github.com client.
                source: Source::default(),
                role: PrRole::Searched,
                repo: repo.to_string(),
                number,
                title: title.to_string(),
                url: url.to_string(),
                head_ref: head.to_string(),
                is_draft: n.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
                state: Some(state),
                author: n
                    .pointer("/author/login")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        });
    }
    Ok((out, total, nodes.len() as u64))
}

/// Merge requests from one GitLab project's search that really name `key`,
/// as links. GitLab's `search` is a substring match, looser still.
pub fn map_gitlab_rows(rows: &[Value], key: &str, host: &str) -> Vec<JiraLink> {
    let source = Source {
        provider: Provider::Gitlab,
        host: host.to_string(),
    };
    let prefix = format!("https://{host}/");
    rows.iter()
        .filter_map(|v| {
            let number = v.get("iid")?.as_u64()?;
            let title = v.get("title")?.as_str()?;
            let url = v.get("web_url")?.as_str()?;
            let repo = url
                .strip_prefix(&prefix)?
                .strip_suffix(&format!("/-/merge_requests/{number}"))?;
            if repo.is_empty() {
                return None;
            }
            let head = v
                .get("source_branch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (_, found_in) = super::keys::keys_for_pr(title, Some(head))
                .into_iter()
                .find(|(k, _)| k == key)?;
            let state = match v.get("state")?.as_str()? {
                "opened" => PrState::Open,
                "merged" => PrState::Merged,
                "closed" => PrState::Closed,
                // `locked` and anything newer: not guessed at.
                _ => return None,
            };
            Some(JiraLink {
                key: key.to_string(),
                found_in,
                current: state == PrState::Open,
                target: LinkTarget::Pr {
                    source: source.clone(),
                    role: PrRole::Searched,
                    repo: repo.to_string(),
                    number,
                    title: title.to_string(),
                    url: url.to_string(),
                    head_ref: head.to_string(),
                    is_draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
                    state: Some(state),
                    author: v
                        .pointer("/author/username")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            })
        })
        .collect()
}

/// Several GitLab projects' answers as one: the projects that answered are
/// the scope, those that did not join the unsearched, and what they found
/// is kept. `Err` only when none answered.
type ProjectAnswer = (String, Result<(Vec<Value>, Option<u64>), String>);

fn combine_gitlab(
    answers: Vec<ProjectAnswer>,
    mut unsearched: Vec<String>,
    key: &str,
    host: &str,
    at: DateTime<Utc>,
) -> Result<(Vec<JiraLink>, Searched), String> {
    let mut found = Vec::new();
    let (mut total, mut read, mut truncated) = (0u64, 0u64, false);
    let mut scope = Vec::new();
    let mut first_error = None;
    for (project, outcome) in answers {
        let label = format!("{host}/{project}");
        match outcome {
            Ok((rows, counted)) => {
                let n = rows.len() as u64;
                read += n;
                total += counted.map_or(n, |c| c.max(n));
                truncated |= counted.map_or(rows.len() >= SEARCH_LIMIT, |c| c > n);
                found.extend(map_gitlab_rows(&rows, key, host));
                scope.push(label);
            }
            Err(e) => {
                first_error.get_or_insert(e);
                unsearched.push(label);
            }
        }
    }
    if scope.is_empty() {
        return Err(first_error.unwrap_or_else(|| "no GitLab project answered".into()));
    }
    Ok((
        found,
        Searched {
            at,
            total,
            read,
            truncated,
            scope,
            unsearched,
        },
    ))
}

fn remember_search(
    conn: &mut Connection,
    provider: Provider,
    key: &str,
    found: &[JiraLink],
    s: &Searched,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    // A searched pull request is replaced by what the latest search on its
    // provider says, so one merged since the last look reads as merged.
    tx.execute(
        "DELETE FROM jira_link WHERE key = ?1 AND kind = 'searched'
           AND json_extract(payload, '$.target.source.provider') = ?2",
        params![key, provider_name(provider)],
    )?;
    index::put_all(&tx, found, &s.at.to_rfc3339())?;
    tx.execute(
        "INSERT INTO jira_key_search
           (key, provider, searched_at, total, read, truncated, scope, unsearched)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(key, provider) DO UPDATE SET searched_at = excluded.searched_at,
           total = excluded.total, read = excluded.read, truncated = excluded.truncated,
           scope = excluded.scope, unsearched = excluded.unsearched",
        params![
            key,
            provider_name(provider),
            s.at.to_rfc3339(),
            s.total as i64,
            s.read as i64,
            s.truncated,
            serde_json::to_string(&s.scope)?,
            serde_json::to_string(&s.unsearched)?,
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// One half's outcome, and what to store when it searched just now.
struct Half {
    search: Search,
    store: Option<(Vec<JiraLink>, Searched)>,
}

impl Half {
    fn said(search: Search) -> Self {
        Self {
            search,
            store: None,
        }
    }
    fn not_asked(reason: &str) -> Self {
        Self::said(Search::NotAsked {
            reason: reason.to_string(),
        })
    }
    fn failed(error: String) -> Self {
        Self::said(Search::Failed { error })
    }
    fn found(links: Vec<JiraLink>, s: Searched) -> Self {
        Self {
            search: s.as_search(),
            store: Some((links, s)),
        }
    }
}

fn fresh(last: Option<Searched>, now: DateTime<Utc>) -> Option<Searched> {
    last.filter(|s| now - s.at < SEARCH_TTL)
}

async fn github_half(
    client: Option<std::sync::Arc<GitHubClient>>,
    owners: Vec<String>,
    last: Option<Searched>,
    key: &str,
    now: DateTime<Utc>,
) -> Half {
    // Searched recently: what it found is already in the database.
    if let Some(s) = fresh(last, now) {
        return Half::said(s.as_search());
    }
    let Some(client) = client else {
        return Half::not_asked("not signed in to GitHub");
    };
    if owners.is_empty() {
        return Half::not_asked("no repositories of yours to search yet");
    }
    let budget = client.request_budget();
    if !budget.permits(SEARCH_COST) {
        return Half::not_asked("the GitHub rate-limit reserve is held for the poll loop");
    }
    let scope: Vec<String> = owners.iter().take(MAX_OWNERS).cloned().collect();
    let unsearched: Vec<String> = owners.iter().skip(MAX_OWNERS).cloned().collect();
    let q = search_query(key, &scope);
    let reply = match tokio::time::timeout(
        SEARCH_DEADLINE,
        client.jira_title_search(&q, SEARCH_LIMIT, &budget),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Half::failed(e.to_string()),
        Err(_) => {
            return Half::failed(format!(
                "GitHub did not answer within {}s",
                SEARCH_DEADLINE.as_secs()
            ))
        }
    };
    match map_search(&reply, key) {
        Ok((found, total, read)) => Half::found(
            found,
            Searched {
                at: now,
                total: total.max(read),
                read,
                truncated: total > read,
                scope,
                unsearched,
            },
        ),
        Err(error) => Half::failed(error),
    }
}

async fn gitlab_half(
    glab: Option<PathBuf>,
    scope: GitLabScope,
    last: Option<Searched>,
    key: &str,
    now: DateTime<Utc>,
) -> Option<Half> {
    let (host, projects, elsewhere) = match scope {
        GitLabScope::Unused => return None,
        GitLabScope::NoHost => {
            return Some(Half::not_asked("the saved GitLab host could not be read"))
        }
        GitLabScope::On {
            host,
            projects,
            elsewhere,
        } => (host, projects, elsewhere),
    };
    if let Some(s) = fresh(last, now) {
        return Some(Half::said(s.as_search()));
    }
    let Some(glab) = glab else {
        return Some(Half::not_asked("the GitLab CLI (glab) is not installed"));
    };
    if projects.is_empty() {
        return Some(Half::not_asked(&format!(
            "no projects of yours on {host} to search yet"
        )));
    }
    let asked: Vec<String> = projects.iter().take(MAX_PROJECTS).cloned().collect();
    let mut unsearched: Vec<String> = projects
        .iter()
        .skip(MAX_PROJECTS)
        .map(|p| format!("{host}/{p}"))
        .collect();
    unsearched.extend(elsewhere);
    let deadline = tokio::time::Instant::now() + SEARCH_DEADLINE;
    let answers = match crate::gitlab::queues::search::search_titles(
        &glab,
        &host,
        &asked,
        key,
        SEARCH_LIMIT,
        deadline,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => return Some(Half::failed(e.to_string())),
    };
    let answers = answers
        .into_iter()
        .map(|a| (a.project, a.outcome.map_err(|e| e.to_string())))
        .collect();
    Some(match combine_gitlab(answers, unsearched, key, &host, now) {
        Ok((found, s)) => Half::found(found, s),
        Err(error) => Half::failed(error),
    })
}

/// What the database holds for a lookup.
struct Held {
    links: Vec<JiraLink>,
    github: Option<Searched>,
    gitlab: Option<Searched>,
    owners: Vec<String>,
    gitlab_scope: GitLabScope,
}

async fn read_held(db: PathBuf, key: String) -> Result<Held, String> {
    tokio::task::spawn_blocking(move || -> Result<_, StoreError> {
        let mut conn = open_db(&db)?;
        index::refresh_prs(&mut conn)?;
        Ok(Held {
            links: index::for_key(&conn, &key)?,
            github: last_search(&conn, &key, Provider::Github)?,
            gitlab: last_search(&conn, &key, Provider::Gitlab)?,
            owners: owners(&conn)?,
            gitlab_scope: gitlab_scope(&conn)?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Every pull request, worktree and branch that names `key`.
///
/// The database first. GitHub and GitLab are searched only when it holds
/// no pull request for the key, or `more` asks for one anyway -- each at
/// most once per [`SEARCH_TTL`], whose results are already stored. `glab`
/// is the GitLab CLI, when installed.
pub async fn find(
    client: Option<std::sync::Arc<GitHubClient>>,
    glab: Option<PathBuf>,
    db: &Path,
    key: &str,
    more: bool,
) -> Result<KeyLookup, String> {
    let held = read_held(db.to_path_buf(), key.to_string()).await?;
    let now = Utc::now();
    let has_pr = held
        .links
        .iter()
        .any(|l| matches!(l.target, LinkTarget::Pr { .. }));
    if has_pr && !more {
        let gitlab = (held.gitlab_scope != GitLabScope::Unused).then(|| Search::NotAsked {
            reason: ALREADY_KNOWN.into(),
        });
        return Ok(KeyLookup {
            key: key.to_string(),
            links: held.links,
            search: Search::NotAsked {
                reason: ALREADY_KNOWN.into(),
            },
            gitlab,
        });
    }
    let (github, gitlab) = tokio::join!(
        github_half(client, held.owners, held.github, key, now),
        gitlab_half(glab, held.gitlab_scope, held.gitlab, key, now),
    );
    let mut stored_any = false;
    for (provider, half) in [
        (Provider::Github, Some(&github)),
        (Provider::Gitlab, gitlab.as_ref()),
    ] {
        let Some((found, s)) = half.and_then(|h| h.store.clone()) else {
            continue;
        };
        stored_any = true;
        let (db2, key2) = (db.to_path_buf(), key.to_string());
        let stored = tokio::task::spawn_blocking(move || {
            open_db(&db2).and_then(|mut c| remember_search(&mut c, provider, &key2, &found, &s))
        })
        .await;
        if !matches!(stored, Ok(Ok(()))) {
            log::warn!("jira: could not store a search for later");
        }
    }
    let links = if stored_any {
        read_held(db.to_path_buf(), key.to_string()).await?.links
    } else {
        held.links
    };
    Ok(KeyLookup {
        key: key.to_string(),
        links,
        search: github.search,
        gitlab: gitlab.map(|h| h.search),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        open_db(&path).unwrap();
        (dir, path)
    }

    fn reply(nodes: Value) -> Value {
        serde_json::json!({"data": {"search": {"issueCount": 3, "nodes": nodes}}})
    }

    fn node(number: u64, title: &str, state: &str) -> Value {
        serde_json::json!({
            "number": number, "title": title, "state": state, "isDraft": false,
            "url": format!("https://github.com/octocat/hello-world/pull/{number}"),
            "headRefName": "x", "author": {"login": "octocat"},
            "repository": {"nameWithOwner": "octocat/hello-world"}
        })
    }

    fn mr(iid: u64, title: &str, state: &str) -> Value {
        serde_json::json!({
            "iid": iid, "title": title, "state": state, "draft": true,
            "web_url": format!("https://gitlab.example.test/group/app/-/merge_requests/{iid}"),
            "source_branch": "feature", "author": {"username": "dana"}
        })
    }

    fn searched(scope: &[&str]) -> Searched {
        Searched {
            at: Utc::now(),
            total: 3,
            read: 1,
            truncated: true,
            scope: scope.iter().map(|s| s.to_string()).collect(),
            unsearched: vec!["later".into()],
        }
    }

    #[test]
    fn the_query_is_a_quoted_key_scoped_to_owners() {
        assert_eq!(
            search_query("ABC-1", &["octocat".into(), "AcmeCorp".into()]),
            "\"ABC-1\" in:title is:pr user:octocat user:AcmeCorp"
        );
    }

    #[test]
    fn keeps_only_pull_requests_that_really_name_the_key() {
        let (links, total, read) = map_search(
            &reply(serde_json::json!([
                node(1, "ABC-1: fix", "MERGED"),
                // GitHub's phrase match is looser than the key rule. Built
                // at runtime: the privacy gate rejects multi-digit keys.
                node(2, &format!("ABC-1{} other", 2), "OPEN"),
                node(3, "ABC-1 follow-up", "WEIRD"),
            ])),
            "ABC-1",
        )
        .unwrap();
        assert_eq!((total, read), (3, 3), "read counts results, not matches");
        assert_eq!(links.len(), 1);
        assert!(!links[0].current, "merged is not open");
        assert!(matches!(
            links[0].target,
            LinkTarget::Pr {
                state: Some(PrState::Merged),
                role: PrRole::Searched,
                ..
            }
        ));
    }

    #[test]
    fn a_reply_without_a_search_is_a_failure_not_none_found() {
        assert!(map_search(&serde_json::json!({"data": null}), "ABC-1").is_err());
        assert!(map_search(
            &serde_json::json!({"data": {"search": {"issueCount": 0}}}),
            "ABC-1"
        )
        .is_err());
    }

    #[test]
    fn gitlab_rows_are_merge_requests_on_their_host() {
        let links = map_gitlab_rows(
            &[
                mr(4, "ABC-1 tidy", "merged"),
                mr(5, &format!("ABC-1{} other", 3), "opened"),
                mr(6, "ABC-1 held", "locked"),
                serde_json::json!({}),
            ],
            "ABC-1",
            "gitlab.example.test",
        );
        assert_eq!(links.len(), 1);
        match &links[0].target {
            LinkTarget::Pr {
                source,
                repo,
                number,
                state,
                is_draft,
                head_ref,
                ..
            } => {
                assert_eq!(source.provider, Provider::Gitlab);
                assert_eq!(source.host, "gitlab.example.test");
                assert_eq!(repo, "group/app");
                assert_eq!(*number, 4);
                assert_eq!(*state, Some(PrState::Merged));
                assert!(*is_draft);
                assert_eq!(head_ref, "feature");
            }
            other => panic!("{other:?}"),
        }
    }

    /// Partial is not nothing: a project that failed is named unsearched,
    /// and what the others found is kept.
    #[test]
    fn a_project_that_did_not_answer_is_unsearched_not_empty() {
        let full: Vec<Value> = (1..=SEARCH_LIMIT as u64)
            .map(|i| mr(i, "unrelated", "opened"))
            .collect();
        let (found, s) = combine_gitlab(
            vec![
                (
                    "group/app".into(),
                    Ok((vec![mr(4, "ABC-1", "opened")], Some(1))),
                ),
                ("group/web".into(), Err("GitLab request timed out".into())),
                ("group/big".into(), Ok((full, None))),
            ],
            vec!["gitlab.example.test/group/old".into()],
            "ABC-1",
            "gitlab.example.test",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            s.scope,
            vec![
                "gitlab.example.test/group/app",
                "gitlab.example.test/group/big"
            ]
        );
        assert_eq!(
            s.unsearched,
            vec![
                "gitlab.example.test/group/old",
                "gitlab.example.test/group/web"
            ]
        );
        assert_eq!(s.read, 1 + SEARCH_LIMIT as u64);
        assert!(s.truncated, "an uncounted full page is only a floor");
    }

    #[test]
    fn no_project_answering_is_a_failure() {
        let got = combine_gitlab(
            vec![("group/app".into(), Err("GitLab request failed".into()))],
            vec![],
            "ABC-1",
            "gitlab.example.test",
            Utc::now(),
        );
        assert_eq!(got.unwrap_err(), "GitLab request failed");
    }

    /// Never "found nothing" for a search that was not made (#1050).
    #[tokio::test]
    async fn without_a_client_it_says_it_did_not_ask() {
        let (_d, path) = db();
        let got = find(None, None, &path, "ABC-1", false).await.unwrap();
        assert!(got.links.is_empty());
        assert!(matches!(got.search, Search::NotAsked { .. }));
        assert_eq!(got.gitlab, None, "no GitLab lists: nothing to search");
    }

    #[tokio::test]
    async fn a_recent_search_is_answered_from_the_database_with_its_scope() {
        let (_d, path) = db();
        let (links, _, _) = map_search(
            &reply(serde_json::json!([node(1, "ABC-1: fix", "MERGED")])),
            "ABC-1",
        )
        .unwrap();
        let s = searched(&["octocat"]);
        remember_search(
            &mut open_db(&path).unwrap(),
            Provider::Github,
            "ABC-1",
            &links,
            &s,
        )
        .unwrap();
        // No client: anything but the stored answer would have to ask.
        let got = find(None, None, &path, "ABC-1", true).await.unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.search, s.as_search());
    }

    #[test]
    fn each_provider_replaces_only_its_own_results() {
        let (_d, path) = db();
        let mut conn = open_db(&path).unwrap();
        let gh = map_search(
            &reply(serde_json::json!([node(1, "ABC-1", "OPEN")])),
            "ABC-1",
        )
        .unwrap()
        .0;
        remember_search(&mut conn, Provider::Github, "ABC-1", &gh, &searched(&[])).unwrap();
        let gl = map_gitlab_rows(&[mr(4, "ABC-1", "opened")], "ABC-1", "gitlab.example.test");
        remember_search(&mut conn, Provider::Gitlab, "ABC-1", &gl, &searched(&[])).unwrap();
        let merged = map_search(
            &reply(serde_json::json!([node(1, "ABC-1", "MERGED")])),
            "ABC-1",
        )
        .unwrap()
        .0;
        remember_search(
            &mut conn,
            Provider::Github,
            "ABC-1",
            &merged,
            &searched(&[]),
        )
        .unwrap();
        let got = index::for_key(&conn, "ABC-1").unwrap();
        assert_eq!(got.len(), 2, "the GitLab result survives a GitHub search");
        assert!(got.iter().any(|l| matches!(
            l.target,
            LinkTarget::Pr {
                state: Some(PrState::Merged),
                ..
            }
        )));
        assert_eq!(
            last_search(&conn, "ABC-1", Provider::Gitlab)
                .unwrap()
                .map(|s| s.read),
            Some(1)
        );
    }

    #[test]
    fn gitlab_scope_is_the_projects_in_your_lists_on_the_saved_host() {
        let (_d, path) = db();
        let conn = open_db(&path).unwrap();
        assert_eq!(gitlab_scope(&conn).unwrap(), GitLabScope::Unused);
        let host = crate::gitlab::host::read_host(&conn).unwrap();
        let row = |repo: &str| serde_json::json!({ "repo": repo });
        for (h, id, payload) in [
            (
                host.as_str(),
                1,
                serde_json::json!([row("group/app"), row("group/web"), row("group/app")]),
            ),
            ("gitlab.other.test", 2, serde_json::json!([row("team/old")])),
        ] {
            conn.execute(
                "INSERT INTO snapshot (provider, host, id, payload, fetched_at)
                 VALUES ('gitlab', ?1, ?2, ?3, '2026-01-01T00:00:00Z')",
                params![h, id, payload.to_string()],
            )
            .unwrap();
        }
        assert_eq!(
            gitlab_scope(&conn).unwrap(),
            GitLabScope::On {
                host,
                projects: vec!["group/app".into(), "group/web".into()],
                elsewhere: vec!["gitlab.other.test/team/old".into()],
            }
        );
    }
}
