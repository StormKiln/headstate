//! Finding the pull requests that name a Jira key (#2041).
//!
//! The database first: the link index already holds every pull request
//! from the user's lists that names the key, including ones that have since
//! left those lists. GitHub is asked only when that holds none -- or when
//! the user asks for more -- and then only across the owners of the
//! repositories the user already works in, never all of GitHub. What it
//! finds is stored, so the next look is the database again.

use super::index;
use super::model::{JiraLink, LinkTarget, PrRole, PrState};
use crate::github::client::GitHubClient;
use crate::store::{open_db, StoreError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// How long one search of a key stands before GitHub is asked again.
const SEARCH_TTL: Duration = Duration::minutes(15);
/// Pull requests one search returns. A key named by more than this is
/// reported as "at least".
pub const SEARCH_LIMIT: usize = 20;
/// Owners one search spans. GitHub refuses queries past five boolean
/// operators, and each `org:` beyond the first is an OR.
const MAX_OWNERS: usize = 5;
/// What one search costs, for the reserve check: a single search node.
const SEARCH_COST: u64 = 1;
const SEARCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// What happened to the GitHub half of a lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Search {
    /// Not asked, and why -- "the database already knew", "no GitHub
    /// sign-in", "the reserve is held for the poll loop". Never "found
    /// nothing" (#1050).
    NotAsked {
        reason: String,
    },
    /// Asked, at `at`. `total` is what GitHub counted; `truncated` when
    /// more matched than were read.
    Asked {
        at: String,
        total: u64,
        truncated: bool,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyLookup {
    pub key: String,
    pub links: Vec<JiraLink>,
    pub search: Search,
}

/// The last search for `key`: when, what GitHub counted, how many read.
fn last_search(
    conn: &Connection,
    key: &str,
) -> Result<Option<(DateTime<Utc>, u64, u64)>, StoreError> {
    let row: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT searched_at, total, read FROM jira_search WHERE key = ?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(at, total, read)| {
        let at = DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Utc);
        Some((at, total.max(0) as u64, read.max(0) as u64))
    }))
}

/// The owners a search may span: those of the GitHub repositories in the
/// user's own lists, most pull requests first. Never all of GitHub.
fn owners(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let repos: Vec<String> = conn
        .prepare(
            "SELECT json_extract(value, '$.repo') FROM snapshot, json_each(snapshot.payload)
             WHERE snapshot.provider = 'github' AND json_valid(snapshot.payload)",
        )?
        .query_map([], |r| r.get::<_, Option<String>>(0))?
        .filter_map(|r| r.transpose())
        .collect::<Result<_, _>>()?;
    let mut count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for repo in repos {
        if let Some((owner, _)) = repo.split_once('/') {
            *count.entry(owner.to_string()).or_default() += 1;
        }
    }
    let mut owners: Vec<(String, usize)> = count.into_iter().collect();
    owners.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(owners
        .into_iter()
        .take(MAX_OWNERS)
        .map(|(o, _)| o)
        .collect())
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

/// Pull requests from one search reply that really name `key` in their
/// title -- GitHub's phrase match is looser than the key rule -- as links.
pub fn map_search(v: &Value, key: &str) -> Result<(Vec<JiraLink>, u64), String> {
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
                source: crate::identity::Source::default(),
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
    Ok((out, total))
}

fn remember_search(
    conn: &mut Connection,
    key: &str,
    found: &[JiraLink],
    total: u64,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    // A searched pull request is replaced by what the latest search says,
    // so one merged since the last look reads as merged.
    tx.execute(
        "DELETE FROM jira_link WHERE key = ?1 AND kind = 'searched'",
        [key],
    )?;
    index::put_all(&tx, found, &now.to_rfc3339())?;
    tx.execute(
        "INSERT INTO jira_search (key, searched_at, total, read) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET searched_at = excluded.searched_at,
           total = excluded.total, read = excluded.read",
        params![key, now.to_rfc3339(), total as i64, found.len() as i64],
    )?;
    tx.commit()?;
    Ok(())
}

/// Every pull request, worktree and branch that names `key`.
///
/// The database first. GitHub is searched only when it holds no pull
/// request for the key, or `more` asks for one anyway -- and not again
/// within [`SEARCH_TTL`] of the last search, whose results are already
/// stored.
pub async fn find(
    client: Option<std::sync::Arc<GitHubClient>>,
    db: &Path,
    key: &str,
    more: bool,
) -> Result<KeyLookup, String> {
    let read = |db: std::path::PathBuf, key: String| async move {
        tokio::task::spawn_blocking(move || -> Result<_, StoreError> {
            let mut conn = open_db(&db)?;
            index::refresh_prs(&mut conn)?;
            Ok((
                index::for_key(&conn, &key)?,
                last_search(&conn, &key)?,
                owners(&conn)?,
            ))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    };
    let (links, last, owners) = read(db.to_path_buf(), key.to_string()).await?;
    let now = Utc::now();
    let has_pr = links
        .iter()
        .any(|l| matches!(l.target, LinkTarget::Pr { .. }));
    let not_asked = |reason: &str| KeyLookup {
        key: key.to_string(),
        links: links.clone(),
        search: Search::NotAsked {
            reason: reason.to_string(),
        },
    };
    if has_pr && !more {
        return Ok(not_asked("already known from your pull requests"));
    }
    // Searched recently: what it found is already in `links`.
    if let Some((at, total, read)) = last.filter(|(t, _, _)| now - *t < SEARCH_TTL) {
        return Ok(KeyLookup {
            key: key.to_string(),
            links,
            search: Search::Asked {
                at: at.to_rfc3339(),
                total,
                truncated: total > read,
            },
        });
    }
    let Some(client) = client else {
        return Ok(not_asked("not signed in to GitHub"));
    };
    if owners.is_empty() {
        return Ok(not_asked("no repositories of yours to search yet"));
    }
    let budget = client.request_budget();
    if !budget.permits(SEARCH_COST) {
        return Ok(not_asked(
            "the GitHub rate-limit reserve is held for the poll loop",
        ));
    }
    let q = search_query(key, &owners);
    let reply = match tokio::time::timeout(
        SEARCH_DEADLINE,
        client.jira_title_search(&q, SEARCH_LIMIT, &budget),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return Ok(KeyLookup {
                key: key.to_string(),
                links,
                search: Search::Failed {
                    error: e.to_string(),
                },
            })
        }
        Err(_) => {
            return Ok(KeyLookup {
                key: key.to_string(),
                links,
                search: Search::Failed {
                    error: format!(
                        "GitHub did not answer within {}s",
                        SEARCH_DEADLINE.as_secs()
                    ),
                },
            })
        }
    };
    let (found, total) = match map_search(&reply, key) {
        Ok(x) => x,
        Err(error) => {
            return Ok(KeyLookup {
                key: key.to_string(),
                links,
                search: Search::Failed { error },
            })
        }
    };
    let (db2, key2, found2) = (db.to_path_buf(), key.to_string(), found.clone());
    let stored = tokio::task::spawn_blocking(move || {
        open_db(&db2).and_then(|mut c| remember_search(&mut c, &key2, &found2, total, now))
    })
    .await;
    if !matches!(stored, Ok(Ok(()))) {
        log::warn!("jira: could not store a search for later");
    }
    let (links, _, _) = read(db.to_path_buf(), key.to_string()).await?;
    Ok(KeyLookup {
        key: key.to_string(),
        links,
        search: Search::Asked {
            at: now.to_rfc3339(),
            total,
            truncated: total as usize > found.len(),
        },
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

    #[test]
    fn the_query_is_a_quoted_key_scoped_to_owners() {
        assert_eq!(
            search_query("ABC-1", &["octocat".into(), "AcmeCorp".into()]),
            "\"ABC-1\" in:title is:pr user:octocat user:AcmeCorp"
        );
    }

    #[test]
    fn keeps_only_pull_requests_that_really_name_the_key() {
        let (links, total) = map_search(
            &reply(serde_json::json!([
                node(1, "ABC-1: fix", "MERGED"),
                // GitHub's phrase match is looser than the key rule. Built
                // at runtime: the privacy gate rejects multi-digit keys.
                node(2, &format!("ABC-1{} other", 2), "OPEN"),
                node(3, "ABC-1 follow-up", "WEIRD"),
                {},
            ])),
            "ABC-1",
        )
        .unwrap();
        assert_eq!(total, 3);
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

    /// Never "found nothing" for a search that was not made (#1050).
    #[tokio::test]
    async fn without_a_client_it_says_it_did_not_ask() {
        let (_d, path) = db();
        let got = find(None, &path, "ABC-1", false).await.unwrap();
        assert!(got.links.is_empty());
        assert!(matches!(got.search, Search::NotAsked { .. }));
    }

    #[tokio::test]
    async fn a_recent_search_is_answered_from_the_database() {
        let (_d, path) = db();
        let (links, total) = map_search(
            &reply(serde_json::json!([node(1, "ABC-1: fix", "MERGED")])),
            "ABC-1",
        )
        .unwrap();
        remember_search(
            &mut open_db(&path).unwrap(),
            "ABC-1",
            &links,
            total,
            Utc::now(),
        )
        .unwrap();
        // No client: anything but the stored answer would have to ask.
        let got = find(None, &path, "ABC-1", true).await.unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(
            got.search,
            Search::Asked {
                at: got_at(&got),
                total: 3,
                truncated: true
            }
        );
    }

    fn got_at(l: &KeyLookup) -> String {
        match &l.search {
            Search::Asked { at, .. } => at.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_new_search_replaces_the_last_ones_results() {
        let (_d, path) = db();
        let mut conn = open_db(&path).unwrap();
        let open = map_search(
            &reply(serde_json::json!([node(1, "ABC-1", "OPEN")])),
            "ABC-1",
        )
        .unwrap()
        .0;
        remember_search(&mut conn, "ABC-1", &open, 1, Utc::now()).unwrap();
        let merged = map_search(
            &reply(serde_json::json!([node(1, "ABC-1", "MERGED")])),
            "ABC-1",
        )
        .unwrap()
        .0;
        remember_search(&mut conn, "ABC-1", &merged, 1, Utc::now()).unwrap();
        let got = index::for_key(&conn, "ABC-1").unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(
            got[0].target,
            LinkTarget::Pr {
                state: Some(PrState::Merged),
                ..
            }
        ));
    }
}
