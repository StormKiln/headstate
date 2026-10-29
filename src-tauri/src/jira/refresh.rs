//! Jira statuses kept fresh in the background (#2036).
//!
//! Every [`CADENCE`], while Jira is switched on, one pass reads the issues
//! the link index names (current links only) and the assigned-to-me list
//! into the `jira_issue` cache -- whether or not the window is open, which
//! is what desktop notifications (#2043) need. The reads are the same ones
//! the page makes ([`super::read`]): batched, partial results kept, and
//! anything read in the last minute not asked again.
//!
//! Switched off, the loop parks and spawns nothing; turning Jira on wakes
//! it ([`wake`]). Told to wait by a rate limit, it waits that long -- or
//! [`RATE_LIMIT_DEFAULT`] when Jira did not say -- and System Health shows
//! it retrying, not stalled.
//!
//! Reported as `jira-refresh` ([`crate::background::JIRA_REFRESH`]).

use super::model::JiraError;
use super::read;
use super::transport::Transport;
use crate::background::{Cycle, Task};
use crate::store::{open_db, settings};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use tokio::sync::Notify;

/// How often a pass runs: slower than any pull-request poll, since a
/// ticket's status moves on human time and every pass costs Jira calls.
pub const CADENCE: Duration = Duration::from_secs(10 * 60);

/// How long to wait after a rate limit that did not say for how long.
pub const RATE_LIMIT_DEFAULT: Duration = Duration::from_secs(5 * 60);

static WAKE: Notify = Notify::const_new();

/// Wake the loop: Jira was switched on or off. A permit is stored if the
/// loop is not waiting yet, so a switch made during a pass is not lost.
pub fn wake() {
    WAKE.notify_one();
}

/// What one pass did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pass {
    /// Keys asked for: linked, then assigned. Zero when there was nothing
    /// to ask -- not "Jira answered nothing".
    pub linked: usize,
    /// Why the pass did not do its job, if it did not. A per-key refusal
    /// among answers (a key that no longer exists) is not that: the rest
    /// were read.
    pub error: Option<JiraError>,
}

/// What the loop does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Jira is off: wait for [`wake`], with no deadline.
    Park,
    /// Wait this long; `retrying` when Jira asked for it.
    Wait { wait: Duration, retrying: bool },
}

/// How long to wait after a pass that ended with `error`.
pub fn next_wait(error: Option<&JiraError>) -> Next {
    match error {
        Some(JiraError::RateLimited { retry_after_secs }) => Next::Wait {
            // At least a second: "retry after 0" must not become a spin.
            wait: retry_after_secs
                .map(|s| Duration::from_secs(s.max(1)))
                .unwrap_or(RATE_LIMIT_DEFAULT),
            retrying: true,
        },
        _ => Next::Wait {
            wait: CADENCE,
            retrying: false,
        },
    }
}

/// The error that fails a read, if one does: one that ended it, or any
/// error when nothing at all was read fresh.
fn verdict(batch: &super::IssueBatch) -> Option<JiraError> {
    let e = batch.error.as_ref()?;
    let read_any = batch.issues.iter().any(|i| !i.stale);
    (e.ends_the_read() || !read_any).then(|| e.clone())
}

/// The keys of every current link in the user's projects, less the ones
/// excluded in Settings. Pull requests are re-indexed first if a list
/// changed, so a pull request polled while the window was closed counts.
fn linked_keys(db: &Path, excluded: &HashSet<String>) -> Result<Vec<String>, String> {
    let mut conn = open_db(db).map_err(|e| e.to_string())?;
    // No dictionary stored yet: the page reads it on first use. Without it
    // a key cannot be told from a look-alike, so nothing is asked.
    let projects: HashSet<String> =
        settings::get::<Vec<String>>(&conn, settings::keys::JIRA_PROJECTS)
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !excluded.contains(p))
            .collect();
    if projects.is_empty() {
        return Ok(Vec::new());
    }
    super::index::refresh_prs(&mut conn).map_err(|e| e.to_string())?;
    let mut keys: Vec<String> = super::index::load(&conn, &projects)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|l| l.current)
        .map(|l| l.key)
        .collect();
    keys.dedup();
    Ok(keys)
}

/// One pass: linked issues, then the assigned list. Stops at a failure
/// that ends a read -- the assigned search would meet it too.
pub async fn pass(t: &Transport, db: &Path, site: Option<&str>, excluded: HashSet<String>) -> Pass {
    let path = db.to_path_buf();
    let keys = match tokio::task::spawn_blocking(move || linked_keys(&path, &excluded)).await {
        Ok(Ok(keys)) => keys,
        Ok(Err(e)) => {
            return Pass {
                linked: 0,
                error: Some(JiraError::failed(format!(
                    "could not read the link index: {e}"
                ))),
            }
        }
        Err(e) => {
            return Pass {
                linked: 0,
                error: Some(JiraError::failed(e.to_string())),
            }
        }
    };
    let mut out = Pass {
        linked: keys.len(),
        error: None,
    };
    if !keys.is_empty() {
        let batch = read::issues(t, db, site, &keys, false).await;
        out.error = verdict(&batch);
        if out.error.as_ref().is_some_and(JiraError::ends_the_read) {
            return out;
        }
    }
    let assigned = read::assigned(t, db, site, super::commands::ASSIGNED_LIMIT).await;
    let assigned_error = verdict(&assigned);
    // The failure that says what to do wins, as on `IssueBatch::error`.
    if assigned_error
        .as_ref()
        .is_some_and(JiraError::ends_the_read)
        || out.error.is_none()
    {
        out.error = assigned_error.or(out.error);
    }
    out
}

/// One turn of the loop, recorded on `task`. `locate` runs only when Jira
/// is on: switched off, nothing is looked for and nothing is spawned.
pub async fn turn(
    task: &Task,
    on: bool,
    locate: impl FnOnce() -> Result<Transport, JiraError>,
    db: &Path,
    site: Option<&str>,
    excluded: HashSet<String>,
) -> (Next, bool) {
    if !on {
        task.park();
        return (Next::Park, false);
    }
    let error = match locate() {
        Ok(t) => pass(&t, db, site, excluded).await.error,
        Err(e) => Some(e),
    };
    let crossed = task.record_cycle(&[match &error {
        Some(e) => Cycle::Failed(crate::redact::redact(&e.to_string())),
        None => Cycle::Ok,
    }]);
    let next = next_wait(error.as_ref());
    if let Next::Wait { wait, retrying } = next {
        if retrying {
            task.retry_in(wait);
        } else {
            task.tick(wait);
        }
    }
    (next, crossed)
}

/// Start once at desktop setup.
pub fn spawn(app: tauri::AppHandle) {
    use crate::background::{announce_degraded, JIRA_REFRESH};
    tauri::async_runtime::spawn(async move {
        // A rate limit outlives a switch off and on: the wait Jira asked
        // for is kept until it is over.
        let mut hold: Option<tokio::time::Instant> = None;
        loop {
            let a = app.clone();
            let prefs =
                crate::commands::off_main_thread(move || crate::commands::read_ui_prefs(&a)).await;
            let on = prefs.jira_enabled;
            if let Some(until) = hold.filter(|u| on && *u > tokio::time::Instant::now()) {
                JIRA_REFRESH.retry_in(until - tokio::time::Instant::now());
                tokio::select! {
                    _ = WAKE.notified() => {}
                    _ = tokio::time::sleep_until(until) => {}
                }
                continue;
            }
            let db = crate::commands::db_path(&app);
            let site = super::commands::site(&app).await;
            let excluded = prefs.jira_excluded_projects.into_iter().collect();
            let (next, crossed) = turn(
                &JIRA_REFRESH,
                on,
                Transport::locate,
                &db,
                site.as_deref(),
                excluded,
            )
            .await;
            announce_degraded(&app, &JIRA_REFRESH, crossed);
            match next {
                Next::Park => WAKE.notified().await,
                Next::Wait { wait, retrying } => {
                    hold = retrying.then(|| tokio::time::Instant::now() + wait);
                    tokio::select! {
                        _ = WAKE.notified() => {}
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cadence_is_slower_than_any_pull_request_poll() {
        assert!(CADENCE >= Duration::from_secs(600));
        assert_eq!(
            next_wait(None),
            Next::Wait {
                wait: CADENCE,
                retrying: false
            }
        );
        assert_eq!(
            next_wait(Some(&JiraError::failed("x"))),
            Next::Wait {
                wait: CADENCE,
                retrying: false
            }
        );
    }

    #[test]
    fn a_rate_limit_waits_as_long_as_it_asked_or_the_default() {
        let limited = |s| JiraError::RateLimited {
            retry_after_secs: s,
        };
        assert_eq!(
            next_wait(Some(&limited(Some(120)))),
            Next::Wait {
                wait: Duration::from_secs(120),
                retrying: true
            }
        );
        assert_eq!(
            next_wait(Some(&limited(None))),
            Next::Wait {
                wait: RATE_LIMIT_DEFAULT,
                retrying: true
            },
            "did not say is not retry now"
        );
        assert_eq!(
            next_wait(Some(&limited(Some(0)))),
            Next::Wait {
                wait: Duration::from_secs(1),
                retrying: true
            }
        );
    }

    #[tokio::test]
    async fn switched_off_it_parks_and_looks_for_nothing() {
        let task = Task::new("t");
        let dir = tempfile::tempdir().unwrap();
        let (next, crossed) = turn(
            &task,
            false,
            || panic!("located while switched off"),
            &dir.path().join("t.db"),
            None,
            HashSet::new(),
        )
        .await;
        assert_eq!(next, Next::Park);
        assert!(!crossed);
        let s = task.snapshot();
        assert!(s.parked);
        assert_eq!((s.last_success_ms, s.total_failures), (None, 0));
    }
}

/// The pass against a stand-in `acli` that records every call.
#[cfg(all(test, unix))]
mod against_acli {
    use super::*;
    use crate::jira::cli::Acli;
    use std::os::unix::fs::PermissionsExt;

    const ONE: &str = r#"[{"key":"ABC-1","fields":{"summary":"s","status":{"name":"To Do","statusCategory":{"key":"new"}}}}]"#;
    const VIEW: &str = r#"{"key":"ABC-1","fields":{"parent":null}}"#;

    fn acli(dir: &Path, body: &str) -> Transport {
        let bin = dir.join("acli");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                dir.join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        Transport::Acli(Acli::new(bin))
    }

    fn answering(dir: &Path) -> Transport {
        acli(
            dir,
            &format!("case \"$3\" in search) echo '{ONE}';; view) echo '{VIEW}';; esac"),
        )
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A database whose index names ABC-1 on a local branch, in a
    /// project the dictionary knows.
    fn indexed(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("t.db");
        let mut conn = open_db(&db).unwrap();
        settings::set(&conn, settings::keys::JIRA_PROJECTS, &vec!["ABC"]).unwrap();
        crate::jira::index::note_branches(&mut conn, "/r", &["ABC-1-thing".to_string()]).unwrap();
        db
    }

    #[tokio::test]
    async fn a_pass_reads_linked_and_assigned_issues_into_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let db = indexed(dir.path());
        let t = answering(dir.path());
        let task = Task::new("t");
        let (next, _) = turn(&task, true, || Ok(t), &db, None, HashSet::new()).await;
        assert_eq!(
            next,
            Next::Wait {
                wait: CADENCE,
                retrying: false
            }
        );
        let searches: Vec<String> = calls(dir.path())
            .into_iter()
            .filter(|c| c.contains("search"))
            .collect();
        assert_eq!(
            searches.len(),
            2,
            "linked keys, then assigned: {searches:?}"
        );
        assert!(searches[0].contains("key in (ABC-1)"), "{searches:?}");
        assert!(searches[1].contains("currentUser()"), "{searches:?}");
        let conn = open_db(&db).unwrap();
        let cached = crate::jira::cache::load(&conn, &["ABC-1".into()]).unwrap();
        assert!(cached.contains_key("ABC-1"), "the read reached the cache");
        assert!(crate::jira::cache::load_assigned(&conn).unwrap().is_some());
        let s = task.snapshot();
        assert!(s.last_success_ms.is_some());
        assert!(!s.parked);
        assert_eq!(s.retry_at_ms, None);
    }

    #[tokio::test]
    async fn an_excluded_project_is_not_asked_about() {
        let dir = tempfile::tempdir().unwrap();
        let db = indexed(dir.path());
        let t = answering(dir.path());
        let got = pass(&t, &db, None, HashSet::from(["ABC".to_string()])).await;
        assert_eq!(got.linked, 0);
        assert!(
            !calls(dir.path()).iter().any(|c| c.contains("key in")),
            "{:?}",
            calls(dir.path())
        );
    }

    #[tokio::test]
    async fn a_rate_limit_stops_the_pass_and_waits_as_asked() {
        let dir = tempfile::tempdir().unwrap();
        let db = indexed(dir.path());
        let t = acli(
            dir.path(),
            "echo 'HTTP 429 Too Many Requests; Retry-After: 120' >&2; exit 1",
        );
        let task = Task::new("t");
        let (next, _) = turn(&task, true, || Ok(t), &db, None, HashSet::new()).await;
        assert_eq!(
            next,
            Next::Wait {
                wait: Duration::from_secs(120),
                retrying: true
            }
        );
        assert_eq!(
            calls(dir.path()).len(),
            1,
            "nothing more asked after the limit"
        );
        let s = task.snapshot();
        assert!(s.retry_at_ms.is_some(), "shown retrying");
        assert_eq!(s.consecutive_failures, 1);
        assert!(s.last_error.unwrap().contains("rate limiting"));
    }

    #[tokio::test]
    async fn a_key_jira_refuses_does_not_fail_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        {
            let mut conn = open_db(&db).unwrap();
            settings::set(&conn, settings::keys::JIRA_PROJECTS, &vec!["ABC"]).unwrap();
            crate::jira::index::note_branches(
                &mut conn,
                "/r",
                &["ABC-1-a".to_string(), "ABC-2-b".to_string()],
            )
            .unwrap();
        }
        // ABC-2 makes any search naming it fail, as Jira does for a key
        // that does not exist; the rest answer.
        let t = acli(
            dir.path(),
            &format!(
                "case \"$*\" in *ABC-2*) echo 'issue does not exist' >&2; exit 1;; esac\n\
                 case \"$3\" in search) echo '{ONE}';; view) echo '{VIEW}';; esac"
            ),
        );
        let task = Task::new("t");
        turn(&task, true, || Ok(t), &db, None, HashSet::new()).await;
        let s = task.snapshot();
        assert_eq!(s.total_failures, 0, "{:?}", s.last_error);
        assert!(s.last_success_ms.is_some());
    }
}
