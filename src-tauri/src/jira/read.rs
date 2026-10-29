//! Reading issues through the cache (#2032).
//!
//! A status comes from a batched search and is kept for [`FRESH`]; within
//! that, a repeat ask spawns nothing. A parent comes from one `workitem
//! view` per key -- search cannot return it -- and is kept for
//! [`PARENT_TTL`]. At most [`PARENT_BUDGET`] parents are read per call; the
//! rest go out with `parent_checked: false`, which the page shows as "not
//! loaded yet" and asks again for shortly.
//!
//! A read that ends early -- rate limited, timed out, signed out -- reads
//! no parents at all: the next call would meet the same refusal.

use super::cache::{self, Cached};
use super::cli;
use super::model::{IssueBatch, JiraError, JiraIssue};
use super::transport::Transport;
use crate::store::{open_db, StoreError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
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
///
/// `fresh` reads every key from Jira whatever the cache holds: opening an
/// issue asks for it now, not for what was read a minute ago (#2036).
pub async fn issues(
    t: &Transport,
    db: &Path,
    site: Option<&str>,
    keys: &[String],
    fresh: bool,
) -> IssueBatch {
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
    let mut stale: Vec<String> = wanted
        .iter()
        .filter(|k| fresh || cached.get(*k).is_none_or(|c| now - c.read_at >= FRESH))
        .cloned()
        .collect();
    // Never read first, then oldest: when the call budget runs out, the
    // keys left unasked are the ones with the newest answer, so a long
    // list is read round-robin across calls rather than its head forever.
    stale.sort_by_key(|k| cached.get(k).map(|c| c.read_at));

    let mut batch = if stale.is_empty() {
        IssueBatch::default()
    } else {
        t.issues(&stale).await
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
    // missing, or never reached because a rate limit or timeout ended it,
    // is answered from the last read instead (#1044), marked stale. An old
    // answer is still an answer; no answer at all stays missing or unasked.
    let mut served_stale: HashSet<String> = HashSet::new();
    if batch.error.is_some() {
        let mut from_cache = |k: &String| match cached.remove(k) {
            Some(c) => {
                served_stale.insert(c.issue.key.clone());
                rows.push(c);
                false
            }
            None => true,
        };
        batch.missing.retain(&mut from_cache);
        batch.unasked.retain(&mut from_cache);
    }
    finish_batch(t, db, site, rows, &mut batch).await;
    for issue in &mut batch.issues {
        issue.stale = served_stale.contains(&issue.key);
    }
    batch
}

/// Open work assigned to the signed-in user, with parents as for `issues`.
///
/// A search that fails outright is answered with the last list that was
/// read, every issue marked stale, the error kept and `list_read_at`
/// saying how old the list is (#2036). Partial is not nothing.
pub async fn assigned(t: &Transport, db: &Path, site: Option<&str>, limit: usize) -> IssueBatch {
    let mut batch = t.assigned(limit).await;
    let now = Utc::now();
    if batch.error.is_some() && batch.issues.is_empty() {
        return last_assigned(t, db, site, batch).await;
    }
    remember_reads(db, &batch.issues, now).await;
    let keys: Vec<String> = batch.issues.iter().map(|i| i.key.clone()).collect();
    if batch.error.is_none() {
        let list = cache::AssignedList {
            keys: keys.clone(),
            truncated: batch.truncated,
            read_at: now,
        };
        if let Err(e) = with_db(db, move |c| cache::save_assigned(c, &list)).await {
            log::warn!("jira cache: could not store the assigned list: {e}");
        }
    }
    batch.list_read_at = Some(now.to_rfc3339());
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
    finish_batch(t, db, site, rows, &mut batch).await;
    batch
}

/// The last assigned list read, for a search that failed. No parent is
/// read: the search that just failed says the next call may too.
async fn last_assigned(
    t: &Transport,
    db: &Path,
    site: Option<&str>,
    mut batch: IssueBatch,
) -> IssueBatch {
    let stored = with_db(db, |c| {
        let Some(list) = cache::load_assigned(c)? else {
            return Ok(None);
        };
        let rows = cache::load(c, &list.keys)?;
        Ok(Some((list, rows)))
    })
    .await
    .unwrap_or_else(|e| {
        log::warn!("jira cache: could not read the assigned list: {e}");
        None
    });
    let Some((list, mut rows)) = stored else {
        return batch;
    };
    // In the order the list was read; a key whose row has since gone is
    // dropped rather than invented.
    let rows: Vec<Cached> = list.keys.iter().filter_map(|k| rows.remove(k)).collect();
    let (mut issues, _) = finish(t, db, site, rows, false).await;
    for issue in &mut issues {
        issue.stale = true;
    }
    batch.issues = issues;
    batch.truncated = list.truncated;
    batch.list_read_at = Some(list.read_at.to_rfc3339());
    batch
}

/// Fill `batch.issues` from `rows`, reading parents unless the read has
/// already been ended. A parent read that ends it is reported on the
/// batch, unless the batch already carries a reason.
async fn finish_batch(
    t: &Transport,
    db: &Path,
    site: Option<&str>,
    rows: Vec<Cached>,
    batch: &mut IssueBatch,
) {
    let may_call = !batch.error.as_ref().is_some_and(JiraError::ends_the_read);
    let (issues, ended) = finish(t, db, site, rows, may_call).await;
    batch.issues = issues;
    if let Some(e) = ended {
        batch.error.get_or_insert(e);
    }
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
/// rows into what the UI is sent. With `may_call` false no parent is read.
/// Returns the failure that ended the parent reads, if one did.
async fn finish(
    t: &Transport,
    db: &Path,
    site: Option<&str>,
    mut rows: Vec<Cached>,
    may_call: bool,
) -> (Vec<JiraIssue>, Option<JiraError>) {
    let now = Utc::now();
    let mut ended: Option<JiraError> = None;
    let mut due: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].parent_read_at.is_none_or(|t| now - t >= PARENT_TTL))
        .collect();
    // Never-read first: an old answer is still an answer.
    due.sort_by_key(|&i| rows[i].parent_read_at.is_some());
    due.truncate(if may_call { PARENT_BUDGET } else { 0 });

    for chunk in due.chunks(PARENT_PARALLEL) {
        if ended.is_some() {
            break;
        }
        let mut set = tokio::task::JoinSet::new();
        for &i in chunk {
            let t = t.clone();
            let key = rows[i].issue.key.clone();
            set.spawn(async move { (i, t.parent(&key).await) });
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
                Err(e) => {
                    log::debug!("jira: parent of {} not read: {e}", rows[i].issue.key);
                    if e.ends_the_read() {
                        ended.get_or_insert(e);
                    }
                }
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
    let issues = rows
        .iter()
        .map(|c| {
            let mut issue = c.resolved();
            issue.url = super::parse::browse_url(site, &issue.key);
            issue
        })
        .collect();
    (issues, ended)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::jira::cli::Acli;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `acli`: search answers from `search`, view from `view`,
    /// and every call is appended to `calls` so a test can count them.
    fn fake(dir: &Path, search: &str, view: &str) -> Transport {
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
        Transport::Acli(Acli::new(bin))
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

        let first = issues(&bin, &db, Some(&site), &keys, false).await;
        let i = &first.issues[0];
        assert_eq!(i.parent_key.as_deref(), Some("ABC-2"));
        assert!(i.parent_checked);
        assert_eq!(i.url, Some(format!("https://{site}/browse/ABC-1")));
        assert_eq!(
            (calls(dir.path(), "search"), calls(dir.path(), "view")),
            (1, 1)
        );

        // Within FRESH: nothing is spawned at all.
        let again = issues(&bin, &db, Some(&site), &keys, false).await;
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
        let got = issues(
            &bin,
            &dir.path().join("t.db"),
            None,
            &["ABC-1".into()],
            false,
        )
        .await;
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
        issues(&ok, &db, None, &["ABC-1".into()], false).await;
        // Age the status past FRESH so the next call reads again.
        let old = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        open_db(&db)
            .unwrap()
            .execute("UPDATE jira_issue SET read_at = ?1", [&old])
            .unwrap();
        let broken = dir.path().join("broken");
        std::fs::write(&broken, "#!/bin/sh\necho 'network down' >&2; exit 1\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o700)).unwrap();

        let broken = Transport::Acli(Acli::new(broken));
        let got = issues(&broken, &db, None, &["ABC-1".into()], false).await;
        assert_eq!(got.issues.len(), 1, "partial is not nothing");
        assert!(got.missing.is_empty());
        assert!(got.error.is_some(), "and the failure is still reported");
        assert_eq!(got.issues[0].parent_key.as_deref(), Some("ABC-2"));
        assert!(got.issues[0].stale, "served from the cache after a failure");
        assert_eq!(
            got.issues[0].read_at.as_deref(),
            Some(old.as_str()),
            "with the age it has"
        );
    }

    fn script(dir: &Path, body: &str) -> Transport {
        let bin = dir.join("acli");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        Transport::Acli(Acli::new(bin))
    }

    /// Opening an issue reads it now: `fresh` goes past the FRESH window
    /// that a plain repeat is answered from (#2036).
    #[tokio::test]
    async fn a_fresh_read_bypasses_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake(dir.path(), ONE, VIEW);
        let db = dir.path().join("t.db");
        let keys = vec!["ABC-1".to_string()];
        issues(&bin, &db, None, &keys, false).await;
        issues(&bin, &db, None, &keys, false).await;
        assert_eq!(
            calls(dir.path(), "search"),
            1,
            "the repeat came from the cache"
        );
        let got = issues(&bin, &db, None, &keys, true).await;
        assert_eq!(calls(dir.path(), "search"), 2, "fresh asked Jira again");
        assert!(!got.issues[0].stale);
        assert!(got.issues[0].read_at.is_some());
    }

    /// A fresh read that fails still answers from the cache, marked stale.
    #[tokio::test]
    async fn a_fresh_read_that_fails_is_answered_stale() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        issues(
            &fake(dir.path(), ONE, VIEW),
            &db,
            None,
            &["ABC-1".into()],
            false,
        )
        .await;
        let down = tempfile::tempdir().unwrap();
        let broken = script(down.path(), "echo 'network down' >&2; exit 1");
        let got = issues(&broken, &db, None, &["ABC-1".into()], true).await;
        assert_eq!(got.issues.len(), 1);
        assert!(got.issues[0].stale);
        assert!(got.error.is_some());
    }

    /// A failed assigned search shows the last list read, stale, with the
    /// error and the list's age; with no list ever read it is the error
    /// alone -- not an empty list (#2036).
    #[tokio::test]
    async fn a_failed_assigned_search_falls_back_to_the_last_list() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let down = tempfile::tempdir().unwrap();
        let limited = script(down.path(), "echo 'HTTP 429 Too Many Requests' >&2; exit 1");

        let never = assigned(&limited, &db, None, 10).await;
        assert!(never.issues.is_empty());
        assert_eq!(never.list_read_at, None, "no list was ever read");
        assert!(never.error.is_some());

        let ok = assigned(&fake(dir.path(), ONE, VIEW), &db, None, 10).await;
        assert_eq!(ok.error, None);
        let read_at = ok.list_read_at.clone().expect("a read list says when");
        assert!(!ok.issues[0].stale);

        let got = assigned(&limited, &db, None, 10).await;
        assert_eq!(
            got.error,
            Some(JiraError::RateLimited {
                retry_after_secs: None
            })
        );
        assert_eq!(got.issues.len(), 1, "the last list, not nothing");
        assert_eq!(got.issues[0].key, "ABC-1");
        assert!(got.issues[0].stale);
        assert_eq!(got.issues[0].parent_key.as_deref(), Some("ABC-2"));
        assert_eq!(got.list_read_at, Some(read_at));
    }

    /// With more keys due than one read may ask, the oldest go first.
    #[tokio::test]
    async fn the_oldest_answers_are_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let first = issues(
            &fake(dir.path(), ONE, VIEW),
            &db,
            None,
            &["ABC-1".into()],
            false,
        )
        .await;
        let mut other = first.issues[0].clone();
        other.key = "ABC-3".into();
        let conn = open_db(&db).unwrap();
        let now = Utc::now();
        cache::save_read(&conn, &first.issues, now - Duration::minutes(2)).unwrap();
        cache::save_read(&conn, &[other], now - Duration::minutes(9)).unwrap();
        let out = tempfile::tempdir().unwrap();
        let bin = script(
            out.path(),
            &format!(
                "echo \"$*\" >> '{}'; echo '[]'",
                out.path().join("args").display()
            ),
        );
        issues(
            &bin,
            &db,
            None,
            &["ABC-1".into(), "ABC-3".into(), "ABC-4".into()],
            false,
        )
        .await;
        let args = std::fs::read_to_string(out.path().join("args")).unwrap();
        let line = args.lines().find(|l| l.contains("key in")).unwrap();
        let (x, y, z) = (
            line.find("ABC-4").unwrap(),
            line.find("ABC-3").unwrap(),
            line.find("ABC-1").unwrap(),
        );
        assert!(x < y && y < z, "never read, then oldest: {line}");
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
            false,
        )
        .await;
        assert_eq!(got.missing, ["abc-1) OR (x"]);
        assert_eq!(calls(dir.path(), "search"), 0);
    }

    /// A rate-limited read keeps what the cache knew, leaves what it never
    /// knew unasked, and spends nothing on parents: the next call would be
    /// refused too (#2033).
    #[tokio::test]
    async fn a_rate_limited_read_answers_from_the_cache_and_reads_no_parents() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        issues(
            &fake(dir.path(), ONE, VIEW),
            &db,
            None,
            &["ABC-1".into()],
            false,
        )
        .await;
        let old = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        open_db(&db)
            .unwrap()
            .execute(
                "UPDATE jira_issue SET read_at = ?1, parent_read_at = ?1",
                [&old],
            )
            .unwrap();
        // Make the parent due again, so only the rate limit can stop it.
        open_db(&db)
            .unwrap()
            .execute("UPDATE jira_issue SET parent_read_at = NULL", [])
            .unwrap();
        let limited_dir = tempfile::tempdir().unwrap();
        let limited = limited_dir.path().join("acli");
        std::fs::write(
            &limited,
            format!(
                "#!/bin/sh\necho \"$3\" >> '{}'\necho 'HTTP 429 Too Many Requests' >&2; exit 1\n",
                limited_dir.path().join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&limited, std::fs::Permissions::from_mode(0o700)).unwrap();
        let limited = Transport::Acli(Acli::new(limited));

        let got = issues(
            &limited,
            &db,
            None,
            &["ABC-1".into(), "ABC-3".into()],
            false,
        )
        .await;
        assert_eq!(
            got.error,
            Some(JiraError::RateLimited {
                retry_after_secs: None
            })
        );
        assert_eq!(got.issues.len(), 1, "the cached answer still stands");
        assert_eq!(got.unasked, ["ABC-3"], "never known, never asked");
        assert!(got.missing.is_empty());
        assert_eq!(calls(limited_dir.path(), "search"), 1);
        assert_eq!(
            calls(limited_dir.path(), "view"),
            0,
            "no parent read after a rate limit"
        );
    }
}
