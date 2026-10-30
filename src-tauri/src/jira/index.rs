//! Where each Jira key is named, kept in `jira_link` (#2035, #2041).
//!
//! Built in the backend from what the database already holds, rather than
//! re-parsed by the frontend on every render:
//!
//! - pull requests from the cached lists (`snapshot`), re-read only when
//!   one of those lists was written since the last build;
//! - worktrees and local branches, noted as each scan finishes.
//!
//! A pull request that leaves its list is kept with `current = 0`, so a
//! ticket can still find it without asking GitHub (#2041). Nothing else
//! in the database remembers merged pull requests -- the stats history
//! that did went with PR Stats in #1827 -- so this accumulates them from
//! the day it is switched on.

use super::keys::{keys_for_pr, keys_in, FoundIn};
use super::model::{CheckedPr, JiraLink, LinkTarget, PrRole};
use crate::identity::{Provider, Source};
use crate::store::source_cache::{load_source_snapshot, SnapshotData};
use crate::store::{settings, CachedList, StoreError};
use rusqlite::{params, Connection, Transaction};
use std::collections::HashSet;

fn kind(t: &LinkTarget) -> &'static str {
    match t {
        // Apart from "pr", so a search never overwrites the row a list
        // wrote for the same pull request, or the reverse.
        LinkTarget::Pr {
            role: PrRole::Searched,
            ..
        } => "searched",
        LinkTarget::Pr { .. } => "pr",
        LinkTarget::Worktree { .. } => "worktree",
        LinkTarget::Branch { .. } => "branch",
    }
}

fn reference(t: &LinkTarget) -> String {
    match t {
        LinkTarget::Pr { url, .. } => url.clone(),
        LinkTarget::Worktree { path, .. } => path.clone(),
        LinkTarget::Branch { repo_path, branch } => format!("{repo_path}\n{branch}"),
    }
}

fn put(tx: &Transaction, link: &JiraLink, now: &str) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO jira_link (key, kind, ref, current, payload, seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(kind, ref, key) DO UPDATE SET
           current = excluded.current, payload = excluded.payload, seen_at = excluded.seen_at",
        params![
            link.key,
            kind(&link.target),
            reference(&link.target),
            link.current,
            serde_json::to_string(link)?,
            now,
        ],
    )?;
    Ok(())
}

/// Store `links` as current, in the caller's transaction.
pub fn put_all(tx: &Transaction, links: &[JiraLink], now: &str) -> Result<(), StoreError> {
    links.iter().try_for_each(|l| put(tx, l, now))
}

/// Links for one target, one per key it names.
fn links(keys: Vec<(String, FoundIn)>, target: LinkTarget) -> Vec<JiraLink> {
    keys.into_iter()
        .map(|(key, found_in)| JiraLink {
            key,
            found_in,
            current: true,
            target: target.clone(),
        })
        .collect()
}

/// What the pull-request part was built from: every cached list's write
/// time and size. Write times are to the second, so the size is what tells
/// two writes in one second apart; the next poll moves the time anyway.
fn fingerprint(conn: &Connection) -> Result<String, StoreError> {
    let lists: Vec<String> = conn
        .prepare(
            "SELECT provider, host, id, fetched_at, length(payload) FROM snapshot
             ORDER BY provider, host, id",
        )?
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(lists.join(";"))
}

/// Re-read pull requests into the index if any list changed since the
/// last build. Returns whether it rebuilt.
pub fn refresh_prs(conn: &mut Connection) -> Result<bool, StoreError> {
    let print = fingerprint(conn)?;
    if settings::get::<String>(conn, settings::keys::JIRA_INDEX_FINGERPRINT)?.as_deref()
        == Some(print.as_str())
    {
        return Ok(false);
    }
    let lists: Vec<(String, String, i64)> = conn
        .prepare("SELECT provider, host, id FROM snapshot")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.transaction()?;
    for (provider, host, id) in lists {
        let Ok(provider) = serde_json::from_value::<Provider>(provider.into()) else {
            continue;
        };
        let (list, role) = match id {
            1 => (CachedList::Authored, PrRole::Authored),
            2 => (CachedList::Reviewing, PrRole::Reviewing),
            _ => continue,
        };
        let source = Source { provider, host };
        let found: Vec<JiraLink> = match load_source_snapshot(&tx, &source, list)?.data {
            SnapshotData::Available { prs, .. } => prs
                .into_iter()
                .flat_map(|pr| {
                    links(
                        keys_for_pr(&pr.title, Some(&pr.head_ref)),
                        LinkTarget::Pr {
                            source: pr.source,
                            role,
                            repo: pr.repo,
                            number: pr.number,
                            title: pr.title,
                            url: pr.url,
                            head_ref: pr.head_ref,
                            is_draft: pr.is_draft,
                            state: None,
                            author: Some(pr.author),
                            merged_at: None,
                        },
                    )
                })
                .collect(),
            SnapshotData::GitLabAvailable { mrs, .. } => mrs
                .into_iter()
                .flat_map(|mr| {
                    links(
                        keys_for_pr(&mr.title, Some(&mr.head_ref)),
                        LinkTarget::Pr {
                            source: mr.source,
                            role,
                            repo: mr.repo,
                            number: mr.number,
                            title: mr.title,
                            url: mr.url,
                            head_ref: mr.head_ref,
                            is_draft: mr.is_draft,
                            state: None,
                            author: Some(mr.author),
                            merged_at: None,
                        },
                    )
                })
                .collect(),
            // A list that could not be read says nothing about what left
            // it: its links keep whatever `current` they had.
            _ => continue,
        };
        // This list was read, so whatever it no longer holds has left it.
        let role = match role {
            PrRole::Authored => "authored",
            PrRole::Reviewing => "reviewing",
            PrRole::Searched => "searched",
        };
        let provider = serde_json::to_value(source.provider)?;
        tx.execute(
            "UPDATE jira_link SET current = 0 WHERE kind = 'pr'
               AND json_extract(payload, '$.target.role') = ?1
               AND json_extract(payload, '$.target.source.provider') = ?2
               AND json_extract(payload, '$.target.source.host') = ?3",
            params![role, provider.as_str(), source.host],
        )?;
        for link in &found {
            put(&tx, link, &now)?;
        }
    }
    settings::set(&tx, settings::keys::JIRA_INDEX_FINGERPRINT, &print)?;
    tx.commit()?;
    Ok(true)
}

/// The open pull requests in every readable authored list, as stored
/// (#2046). Read BEFORE [`refresh_prs`] by a caller that wants both: the
/// links are then at least as new as this, so a pull request listed here
/// cannot be missing a key a newer list gave it.
///
/// A list that could not be read contributes nothing, so its pull requests
/// stay "not looked at" rather than becoming "names no ticket".
pub fn checked_authored(conn: &Connection) -> Result<Vec<CheckedPr>, StoreError> {
    let sources: Vec<(String, String)> = conn
        .prepare("SELECT provider, host FROM snapshot WHERE id = 1")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for (provider, host) in sources {
        let Ok(provider) = serde_json::from_value::<Provider>(provider.into()) else {
            continue;
        };
        let source = Source { provider, host };
        match load_source_snapshot(conn, &source, CachedList::Authored)?.data {
            SnapshotData::Available { prs, .. } => out.extend(prs.into_iter().map(|p| CheckedPr {
                url: p.url,
                title: p.title,
                head_ref: p.head_ref,
            })),
            SnapshotData::GitLabAvailable { mrs, .. } => {
                out.extend(mrs.into_iter().map(|m| CheckedPr {
                    url: m.url,
                    title: m.title,
                    head_ref: m.head_ref,
                }))
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Replace one repository's worktree links with what a scan just found.
pub fn note_worktrees(
    conn: &mut Connection,
    repos: &[(String, Vec<(String, String)>)],
) -> Result<(), StoreError> {
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.transaction()?;
    for (repo, worktrees) in repos {
        tx.execute(
            "DELETE FROM jira_link WHERE kind = 'worktree'
               AND json_extract(payload, '$.target.repo') = ?1",
            [repo],
        )?;
        for (path, branch) in worktrees {
            let keys = keys_in(branch, FoundIn::Branch)
                .into_iter()
                .map(|k| (k, FoundIn::Branch))
                .collect();
            let target = LinkTarget::Worktree {
                repo: repo.clone(),
                path: path.clone(),
                branch: branch.clone(),
            };
            for link in links(keys, target) {
                put(&tx, &link, &now)?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replace one repository's local-branch links with a scan's branch names.
pub fn note_branches(
    conn: &mut Connection,
    repo_path: &str,
    branches: &[String],
) -> Result<(), StoreError> {
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM jira_link WHERE kind = 'branch'
           AND json_extract(payload, '$.target.repo_path') = ?1",
        [repo_path],
    )?;
    for branch in branches {
        let keys = keys_in(branch, FoundIn::Branch)
            .into_iter()
            .map(|k| (k, FoundIn::Branch))
            .collect();
        let target = LinkTarget::Branch {
            repo_path: repo_path.to_string(),
            branch: branch.clone(),
        };
        for link in links(keys, target) {
            put(&tx, &link, &now)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// The project part of a key: everything before the last `-`.
fn project(key: &str) -> &str {
    key.rsplit_once('-').map_or(key, |(p, _)| p)
}

/// Rows as links. `current` is the column's, not the payload's: marking a
/// pull request as gone updates the column alone.
fn read(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<JiraLink>, StoreError> {
    let rows: Vec<(String, bool)> = conn
        .prepare(sql)?
        .query_map(args, |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows
        .into_iter()
        // A row this build cannot read is skipped; the next build rewrites it.
        .filter_map(|(p, current)| {
            let mut link = serde_json::from_str::<JiraLink>(&p).ok()?;
            link.current = current;
            Some(link)
        })
        .collect())
}

/// Every stored link whose project is in `projects`.
pub fn load(conn: &Connection, projects: &HashSet<String>) -> Result<Vec<JiraLink>, StoreError> {
    let mut links = read(
        conn,
        "SELECT payload, current FROM jira_link ORDER BY key, kind, ref",
        &[],
    )?;
    links.retain(|l| projects.contains(project(&l.key)));
    Ok(links)
}

/// Keys named by one of the user's OWN pull requests that left its list
/// since `since` (RFC 3339), whose project is in `projects`: the tickets a
/// merge may just have left behind (#2043). `seen_at` is when the pull
/// request was last seen listed, so this is "listed until recently".
pub fn recently_left(
    conn: &Connection,
    projects: &HashSet<String>,
    since: &str,
) -> Result<Vec<String>, StoreError> {
    let mut keys: Vec<String> = conn
        .prepare(
            "SELECT key FROM jira_link WHERE kind = 'pr' AND current = 0
               AND json_extract(payload, '$.target.role') = 'authored' AND seen_at >= ?1
             GROUP BY key ORDER BY MAX(seen_at) DESC",
        )?
        .query_map([since], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    keys.retain(|k| projects.contains(project(k)));
    Ok(keys)
}

/// Every stored link naming `key`, whatever its project. For a jump to a
/// ticket the user typed, where the dictionary has already been checked.
pub fn for_key(conn: &Connection, key: &str) -> Result<Vec<JiraLink>, StoreError> {
    read(
        conn,
        "SELECT payload, current FROM jira_link WHERE key = ?1 ORDER BY kind, ref",
        &[&key],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::{CiState, MergeState, MergeStateStatus, PullRequest, ReviewState};
    use crate::store::source_cache::{save_owned_source_snapshot, Coverage};
    use chrono::Utc;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn pr(number: u64, title: &str, head: &str) -> PullRequest {
        PullRequest {
            observation: None,
            source: Default::default(),
            id: format!("PR_{number}"),
            number,
            title: title.into(),
            url: format!("https://github.com/octocat/hello-world/pull/{number}"),
            repo: "octocat/hello-world".into(),
            head_ref: head.into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            head_repo: None,
            base_ref: "main".into(),
            author: "octocat".into(),
            is_draft: false,
            created_at: Utc::now(),
            ready_at: None,
            updated_at: Utc::now(),
            ci: CiState::Success,
            merge: MergeState::Mergeable,
            merge_status: MergeStateStatus::Clean,
            review: ReviewState::Approved,
            in_merge_queue: false,
            labels: Vec::new(),
            comment_count: 0,
            unresolved_threads: 0,
            unresolved_threads_floor: false,
            requested_reviewers: Vec::new(),
            assignees: Vec::new(),
            latest_reviews: Vec::new(),
            requested_reviewers_total: 0,
            assignees_total: 0,
            latest_reviews_total: 0,
            labels_total: 0,
        }
    }

    fn save(conn: &Connection, prs: &[PullRequest]) {
        save_owned_source_snapshot(
            conn,
            &Source::default(),
            CachedList::Authored,
            prs,
            &Coverage::Unknown,
            Some("octocat"),
        )
        .unwrap();
    }

    fn all() -> HashSet<String> {
        ["ABC", "XY"].iter().map(|s| s.to_string()).collect()
    }

    fn keys(links: &[JiraLink]) -> Vec<(&str, bool)> {
        links.iter().map(|l| (l.key.as_str(), l.current)).collect()
    }

    #[test]
    fn indexes_titles_and_branches_and_applies_the_dictionary_on_read() {
        let mut conn = db();
        save(
            &conn,
            &[pr(1, "ABC-1 fix", "xy-2-fix"), pr(2, "UTF-8 bump", "main")],
        );
        assert!(refresh_prs(&mut conn).unwrap());
        let got = load(&conn, &all()).unwrap();
        assert_eq!(keys(&got), [("ABC-1", true), ("XY-2", true)]);
        assert_eq!(got[1].found_in, FoundIn::Branch);
        // Not a project: stored as a candidate, never returned.
        assert!(got.iter().all(|l| l.key != "UTF-8"));
        let only_abc: HashSet<String> = ["ABC".to_string()].into();
        assert_eq!(keys(&load(&conn, &only_abc).unwrap()), [("ABC-1", true)]);
    }

    /// #2046: every authored pull request the index read is listed, with
    /// or without a key, as it was read -- the frontend tells "names no
    /// ticket" from "not looked at yet" by it.
    #[test]
    fn checked_lists_every_authored_pr_as_read() {
        let conn = db();
        assert!(checked_authored(&conn).unwrap().is_empty());
        save(&conn, &[pr(1, "ABC-1 fix", "x"), pr(2, "No key", "y")]);
        let got = checked_authored(&conn).unwrap();
        let titles: Vec<&str> = got.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["ABC-1 fix", "No key"]);
        assert_eq!(got[1].head_ref, "y");
        assert!(got[1].url.ends_with("/pull/2"));
    }

    #[test]
    fn an_unchanged_database_is_not_reread() {
        let mut conn = db();
        save(&conn, &[pr(1, "ABC-1", "x")]);
        assert!(refresh_prs(&mut conn).unwrap());
        assert!(!refresh_prs(&mut conn).unwrap());
    }

    /// The point of keeping them (#2041): a PR that left the open list is
    /// still found, marked as no longer current, not forgotten.
    #[test]
    fn a_pr_that_left_its_list_is_kept_as_not_current() {
        let mut conn = db();
        save(&conn, &[pr(1, "ABC-1", "x"), pr(2, "ABC-2", "y")]);
        refresh_prs(&mut conn).unwrap();
        save(&conn, &[pr(2, "ABC-2", "y")]);
        refresh_prs(&mut conn).unwrap();
        assert_eq!(
            keys(&load(&conn, &all()).unwrap()),
            [("ABC-1", false), ("ABC-2", true)]
        );
    }

    #[test]
    fn recently_left_names_own_pull_requests_that_left_since() {
        let mut conn = db();
        save(&conn, &[pr(1, "ABC-1", "x"), pr(2, "ABC-2", "y")]);
        refresh_prs(&mut conn).unwrap();
        save(&conn, &[pr(2, "ABC-2", "y")]);
        refresh_prs(&mut conn).unwrap();
        assert_eq!(
            recently_left(&conn, &all(), "2000-01-01T00:00:00Z").unwrap(),
            ["ABC-1"],
            "still listed is not left"
        );
        assert!(
            recently_left(&conn, &all(), "2999-01-01T00:00:00Z")
                .unwrap()
                .is_empty(),
            "left before `since` is not recent"
        );
        assert!(
            recently_left(&conn, &HashSet::new(), "2000-01-01T00:00:00Z")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn for_key_finds_a_ticket_whatever_the_dictionary() {
        let mut conn = db();
        save(&conn, &[pr(7, "ZZ-3 ship it", "x")]);
        refresh_prs(&mut conn).unwrap();
        assert!(load(&conn, &all()).unwrap().is_empty());
        let got = for_key(&conn, "ZZ-3").unwrap();
        assert!(matches!(got[0].target, LinkTarget::Pr { number: 7, .. }));
    }

    #[test]
    fn a_rescan_replaces_a_repositorys_worktrees_and_branches() {
        let mut conn = db();
        let wt = |b: &str| (format!("/w/{b}"), b.to_string());
        note_worktrees(
            &mut conn,
            &[("r".into(), vec![wt("abc-1-a"), wt("xy-2-b")])],
        )
        .unwrap();
        note_worktrees(&mut conn, &[("r".into(), vec![wt("xy-2-b")])]).unwrap();
        note_branches(&mut conn, "/repo", &["abc-4".into(), "main".into()]).unwrap();
        note_branches(&mut conn, "/repo", &["abc-5".into()]).unwrap();
        assert_eq!(
            keys(&load(&conn, &all()).unwrap()),
            [("ABC-5", true), ("XY-2", true)]
        );
    }
}
