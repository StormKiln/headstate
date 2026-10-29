//! Reading issues through the cache (#2032).
//!
//! A status comes from a batched search and is kept for [`FRESH`]; within
//! that, a repeat ask spawns nothing. A parent comes from one `workitem
//! view` per key -- search cannot return it -- and is kept for
//! [`PARENT_TTL`]. At most [`PARENT_BUDGET`] parents are read per call; the
//! rest go out with `parent_checked: false`, which the page shows as "not
//! loaded yet" and asks again for shortly.

use super::cache::{self, Cached};
use super::cli;
use super::model::{IssueBatch, JiraIssue};
use crate::store::{open_db, StoreError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

/// How long a status read stands before it is read again.
const FRESH: Duration = Duration::seconds(60);
/// How long a parent read stands. Moving an issue to another epic is rare,
/// and a parent costs a call of its own.
const PARENT_TTL: Duration = Duration::hours(12);
/// Parents read per call. About a second each, four at a time.
const PARENT_BUDGET: usize = 8;
const PARENT_PARALLEL: usize = 4;

/// Run `work` against the database on the blocking pool. A database that
/// cannot be opened is reported, and the caller carries on without it:
/// a cache that failed must not fail the read.
async fn with_db<T, F>(db: &Path, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
{
    let db = db.to_path_buf();
    tokio::task::spawn_blocking(move || open_db(&db).and_then(|conn| work(&conn)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// What Jira says about `keys`, from the cache where it is fresh.
pub async fn issues(bin: &Path, db: &Path, site: Option<&str>, keys: &[String]) -> IssueBatch {
    let mut wanted: Vec<String> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    for k in keys {
        if !cli::is_key(k) {
            invalid.push(k.clone());
        } else if !wanted.contains(k) {
            wanted.push(k.clone());
        }
    }
    let lookup = wanted.clone();
    let mut cached = with_db(db, move |c| cache::load(c, &lookup))
        .await
        .unwrap_or_else(|e| {
            log::warn!("jira cache: could not read: {e}");
            HashMap::new()
        });
    let now = Utc::now();
    let stale: Vec<String> = wanted
        .iter()
        .filter(|k| cached.get(*k).is_none_or(|c| now - c.read_at >= FRESH))
        .cloned()
        .collect();

    let mut batch = if stale.is_empty() {
        IssueBatch::default()
    } else {
        cli::issues_at(bin, &stale).await
    };
    batch.missing.extend(invalid);
    remember_reads(db, &batch.issues, now).await;

    // Fresh from the cache, then this read on top with its stored parent.
    let mut rows: Vec<Cached> = Vec::new();
    for key in &wanted {
        if !stale.contains(key) {
            if let Some(c) = cached.remove(key) {
                rows.push(c);
            }
        }
    }
    for issue in std::mem::take(&mut batch.issues) {
        let prior = cached.remove(&issue.key);
        rows.push(fresh_row(issue, prior, now));
    }
    // A read that FAILED is not "Jira returned nothing": a key it left
    // missing is answered from the last read instead (#1044).
    if batch.error.is_some() {
        batch.missing.retain(|k| match cached.remove(k) {
            Some(c) => {
                rows.push(c);
                false
            }
            None => true,
        });
    }
    batch.issues = finish(bin, db, site, rows).await;
    batch
}

/// Open work assigned to the signed-in user, with parents as for `issues`.
pub async fn assigned(bin: &Path, db: &Path, site: Option<&str>, limit: usize) -> IssueBatch {
    let mut batch = cli::assigned_at(bin, limit).await;
    let now = Utc::now();
    remember_reads(db, &batch.issues, now).await;
    let keys: Vec<String> = batch.issues.iter().map(|i| i.key.clone()).collect();
    let mut cached = with_db(db, move |c| cache::load(c, &keys))
        .await
        .unwrap_or_default();
    let rows = std::mem::take(&mut batch.issues)
        .into_iter()
        .map(|issue| {
            let prior = cached.remove(&issue.key);
            fresh_row(issue, prior, now)
        })
        .collect();
    batch.issues = finish(bin, db, site, rows).await;
    batch
}

/// A status just read, carrying the parent the cache already holds.
fn fresh_row(mut issue: JiraIssue, prior: Option<Cached>, now: DateTime<Utc>) -> Cached {
    let parent_read_at = prior.as_ref().and_then(|p| p.parent_read_at);
    if parent_read_at.is_some() {
        issue.parent_key = prior.and_then(|p| p.issue.parent_key);
    }
    Cached {
        issue,
        read_at: now,
        parent_read_at,
    }
}

async fn remember_reads(db: &Path, issues: &[JiraIssue], now: DateTime<Utc>) {
    if issues.is_empty() {
        return;
    }
    let issues = issues.to_vec();
    if let Err(e) = with_db(db, move |c| cache::save_read(c, &issues, now)).await {
        log::warn!("jira cache: could not store issues: {e}");
    }
}

/// Read the parents that are unknown or old, within the budget, then turn
/// rows into what the UI is sent.
async fn finish(
    bin: &Path,
    db: &Path,
    site: Option<&str>,
    mut rows: Vec<Cached>,
) -> Vec<JiraIssue> {
    let now = Utc::now();
    let mut due: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].parent_read_at.is_none_or(|t| now - t >= PARENT_TTL))
        .collect();
    // Never-read first: an old answer is still an answer.
    due.sort_by_key(|&i| rows[i].parent_read_at.is_some());
    due.truncate(PARENT_BUDGET);

    for chunk in due.chunks(PARENT_PARALLEL) {
        let mut set = tokio::task::JoinSet::new();
        for &i in chunk {
            let bin = bin.to_path_buf();
            let key = rows[i].issue.key.clone();
            set.spawn(async move { (i, cli::parent_at(&bin, &key).await) });
        }
        let mut read: Vec<(String, Option<String>)> = Vec::new();
        while let Some(Ok((i, got))) = set.join_next().await {
            match got {
                Ok(parent) => {
                    rows[i].issue.parent_key = parent.clone();
                    rows[i].parent_read_at = Some(now);
                    read.push((rows[i].issue.key.clone(), parent));
                }
                // Left unchecked, and asked again next time.
                Err(e) => log::debug!("jira: parent of {} not read: {e}", rows[i].issue.key),
            }
        }
        if !read.is_empty() {
            let stored = with_db(db, move |c| {
                read.iter()
                    .try_for_each(|(k, p)| cache::save_parent(c, k, p.as_deref(), now))
            })
            .await;
            if let Err(e) = stored {
                log::warn!("jira cache: could not store parents: {e}");
            }
        }
    }
    rows.iter()
        .map(|c| {
            let mut issue = c.resolved();
            issue.url = super::parse::browse_url(site, &issue.key);
            issue
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A stand-in `acli`: search answers from `search`, view from `view`,
    /// and every call is appended to `calls` so a test can count them.
    fn fake(dir: &Path, search: &str, view: &str) -> PathBuf {
        let bin = dir.join("acli");
        let calls = dir.join("calls");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$3\" >> '{}'\ncase \"$3\" in search) echo '{search}';; view) echo '{view}';; esac\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        bin
    }

    fn calls(dir: &Path, what: &str) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l == what)
            .count()
    }

    const ONE: &str = r#"[{"key":"ABC-1","fields":{"summary":"s","status":{"name":"To Do","statusCategory":{"key":"new"}}}}]"#;
    const VIEW: &str = r#"{"key":"ABC-1","fields":{"parent":{"key":"ABC-2"}}}"#;

    #[tokio::test]
    async fn parents_are_read_once_and_statuses_are_cached_briefly() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake(dir.path(), ONE, VIEW);
        let db = dir.path().join("t.db");
        let site = ["acme", "atlassian", "net"].join(".");
        let keys = vec!["ABC-1".to_string()];

        let first = issues(&bin, &db, Some(&site), &keys).await;
        let i = &first.issues[0];
        assert_eq!(i.parent_key.as_deref(), Some("ABC-2"));
        assert!(i.parent_checked);
        assert_eq!(i.url, Some(format!("https://{site}/browse/ABC-1")));
        assert_eq!(
            (calls(dir.path(), "search"), calls(dir.path(), "view")),
            (1, 1)
        );

        // Within FRESH: nothing is spawned at all.
        let again = issues(&bin, &db, Some(&site), &keys).await;
        assert_eq!(again.issues, first.issues);
        assert_eq!(
            (calls(dir.path(), "search"), calls(dir.path(), "view")),
            (1, 1)
        );
    }

    #[tokio::test]
    async fn a_parent_that_could_not_be_read_is_unchecked_not_absent() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake(dir.path(), ONE, "null");
        let got = issues(&bin, &dir.path().join("t.db"), None, &["ABC-1".into()]).await;
        let i = &got.issues[0];
        assert_eq!(i.parent_key, None);
        assert!(!i.parent_checked, "a null reply is not 'no parent'");
        assert_eq!(i.url, None, "no site, no link");
    }

    #[tokio::test]
    async fn a_failed_read_answers_from_the_last_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let ok = fake(dir.path(), ONE, VIEW);
        issues(&ok, &db, None, &["ABC-1".into()]).await;
        // Age the status past FRESH so the next call reads again.
        let old = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        open_db(&db)
            .unwrap()
            .execute("UPDATE jira_issue SET read_at = ?1", [old])
            .unwrap();
        let broken = dir.path().join("broken");
        std::fs::write(&broken, "#!/bin/sh\necho 'network down' >&2; exit 1\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o700)).unwrap();

        let got = issues(&broken, &db, None, &["ABC-1".into()]).await;
        assert_eq!(got.issues.len(), 1, "partial is not nothing");
        assert!(got.missing.is_empty());
        assert!(got.error.is_some(), "and the failure is still reported");
        assert_eq!(got.issues[0].parent_key.as_deref(), Some("ABC-2"));
    }

    #[tokio::test]
    async fn invalid_keys_are_missing_without_a_call() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake(dir.path(), "[]", VIEW);
        let got = issues(
            &bin,
            &dir.path().join("t.db"),
            None,
            &["abc-1) OR (x".into()],
        )
        .await;
        assert_eq!(got.missing, ["abc-1) OR (x"]);
        assert_eq!(calls(dir.path(), "search"), 0);
    }
}
