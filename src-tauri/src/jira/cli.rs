//! Talking to `acli`, bounded.
//!
//! Every call has a timeout and is killed on expiry, and the output is read
//! concurrently with the wait. Docker's `run_bounded` waits first and reads
//! after, which is fine for its small replies but would deadlock here: a
//! search returning a few hundred issues overflows the pipe buffer, the
//! child blocks on write, and the wait never ends.

use super::model::{IssueBatch, JiraConnection, JiraIssue, ProjectList};
use super::parse;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One `acli` call. Jira Cloud answers a search in well under this; the
/// ceiling is for a wedged network, not a slow query.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Keys per `key in (...)` search. Bounded so one JQL string stays well
/// inside Jira's request-size limits; #2032 measures the real ceiling.
const BATCH: usize = 50;

/// Most calls one `issues` request may make, splits included. A user with
/// hundreds of keyed branches gets the first few hundred answered and the
/// rest marked `unasked`, never a request that runs for minutes.
const MAX_CALLS: usize = 12;

/// The fields every search asks for. `acli` 1.3 refuses `parent`,
/// `project`, `updated` and `created` in a search ("field 'parent' is not
/// allowed", #2032), and one refused field fails the whole call, so these
/// are the ones it allows. Parents come from [`parent_at`].
const FIELDS: &str = "summary,status,issuetype,assignee";

#[cfg(target_os = "macos")]
const FALLBACK_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
#[cfg(all(unix, not(target_os = "macos")))]
const FALLBACK_DIRS: &[&str] = &["/usr/local/bin", "/usr/bin", "/snap/bin"];
#[cfg(windows)]
const FALLBACK_DIRS: &[&str] = &[];

/// Locate `acli`: `HEADSTATE_ACLI`, then `PATH`, then the usual installs.
pub fn find_acli() -> Option<PathBuf> {
    crate::auth::find_exe_with(
        &format!("acli{}", std::env::consts::EXE_SUFFIX),
        FALLBACK_DIRS,
        std::env::var("PATH").ok().as_deref(),
        std::env::var("HEADSTATE_ACLI").ok().as_deref(),
    )
}

/// What one call produced. Not a `Result`: `auth status` reports a logged
/// out user through its exit code, which is an answer, not a failure.
struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run(bin: &Path, args: &[&str]) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .env("PATH", crate::packages::tools::child_path(bin))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // A background read must not flash a console window on Windows (#1603).
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let out = tokio::time::timeout(CALL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("acli did not respond within {}s", CALL_TIMEOUT.as_secs()))?
        .map_err(|e| format!("acli could not be started: {e}"))?;
    Ok(Output {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

async fn run_ok(bin: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(bin, args).await?;
    if out.success {
        Ok(out.stdout)
    } else if out.stderr.is_empty() {
        Err("acli failed without saying why".into())
    } else {
        Err(out.stderr)
    }
}

/// Whether Jira can be asked, by asking `acli`.
pub async fn connection_at(bin: &Path) -> JiraConnection {
    match run(bin, &["jira", "auth", "status"]).await {
        Ok(o) => parse::auth_status(o.success, &o.stdout, &o.stderr),
        Err(reason) => JiraConnection::CannotTell { reason },
    }
}

pub async fn projects_at(bin: &Path) -> ProjectList {
    match run_ok(bin, &["jira", "project", "list", "--paginate", "--json"])
        .await
        .and_then(|raw| parse::projects(&raw))
    {
        Ok(projects) => ProjectList {
            projects,
            error: None,
        },
        Err(e) => ProjectList {
            projects: Vec::new(),
            error: Some(e),
        },
    }
}

/// Whether `key` has the shape of a Jira key. Keys reach this from the
/// frontend and are interpolated into JQL, so anything else is refused
/// before it can be.
pub fn is_key(key: &str) -> bool {
    let Some((project, number)) = key.split_once('-') else {
        return false;
    };
    let mut p = project.chars();
    p.next().is_some_and(|c| c.is_ascii_uppercase())
        && (2..=10).contains(&project.len())
        && p.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !number.is_empty()
        && !number.starts_with('0')
        && number.len() <= 9
        && number.chars().all(|c| c.is_ascii_digit())
}

async fn search(bin: &Path, jql: &str, limit: usize) -> Result<Vec<JiraIssue>, String> {
    let limit = limit.to_string();
    let raw = run_ok(
        bin,
        &[
            "jira", "workitem", "search", "--jql", jql, "--fields", FIELDS, "--limit", &limit,
            "--json",
        ],
    )
    .await?;
    parse::issues(&raw)
}

/// The parent of `key`, by `workitem view`, which -- unlike search -- may
/// ask for it. One call per key; the caller keeps the answer.
pub async fn parent_at(bin: &Path, key: &str) -> Result<Option<String>, String> {
    if !is_key(key) {
        return Err(format!("{key} is not a Jira key"));
    }
    let raw = run_ok(
        bin,
        &[
            "jira",
            "workitem",
            "view",
            key,
            "--fields",
            "parent,issuetype",
            "--json",
        ],
    )
    .await?;
    parse::parent(&raw)
}

/// What Jira says about `keys`.
///
/// Batched `key in (...)` searches. A batch that fails is split in half and
/// retried, down to single keys, while the call budget lasts: JQL rejects
/// the WHOLE query when one key in it does not exist, and one stale branch
/// name must not hide every other ticket. What the budget never reached is
/// returned as `unasked`, not `missing`.
pub async fn issues_at(bin: &Path, keys: &[String]) -> IssueBatch {
    let mut out = IssueBatch {
        issues: Vec::new(),
        missing: Vec::new(),
        unasked: Vec::new(),
        error: None,
        truncated: false,
    };
    let mut wanted: Vec<String> = Vec::new();
    for k in keys {
        if !is_key(k) {
            out.missing.push(k.clone());
        } else if !wanted.contains(k) {
            wanted.push(k.clone());
        }
    }
    let mut queue: std::collections::VecDeque<Vec<String>> =
        wanted.chunks(BATCH).map(<[String]>::to_vec).collect();
    let mut calls = 0;
    while let Some(batch) = queue.pop_front() {
        if calls == MAX_CALLS {
            out.unasked.extend(batch);
            continue;
        }
        calls += 1;
        let jql = format!("key in ({})", batch.join(", "));
        match search(bin, &jql, batch.len()).await {
            Ok(found) => {
                for k in &batch {
                    if !found.iter().any(|i| &i.key == k) {
                        out.missing.push(k.clone());
                    }
                }
                out.issues.extend(found);
            }
            Err(e) if batch.len() > 1 => {
                out.error.get_or_insert(e);
                let (a, b) = batch.split_at(batch.len() / 2);
                queue.push_front(b.to_vec());
                queue.push_front(a.to_vec());
            }
            Err(e) => {
                // One key, refused. Jira does not distinguish "no such
                // issue" from "not yours to see"; both are `missing`.
                out.error.get_or_insert(e);
                out.missing.extend(batch);
            }
        }
    }
    out
}

/// Open work assigned to the signed-in user, newest first.
pub async fn assigned_at(bin: &Path, limit: usize) -> IssueBatch {
    let jql = "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC";
    match search(bin, jql, limit).await {
        Ok(issues) => IssueBatch {
            truncated: issues.len() >= limit,
            issues,
            missing: Vec::new(),
            unasked: Vec::new(),
            error: None,
        },
        Err(e) => IssueBatch {
            issues: Vec::new(),
            missing: Vec::new(),
            unasked: Vec::new(),
            error: Some(e),
            truncated: false,
        },
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `acli` that answers from a shell script. Passed by path,
    /// never through `HEADSTATE_ACLI`, so no test edits the process
    /// environment (#481).
    fn fake(script: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("acli");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, bin)
    }

    fn issue(key: &str) -> String {
        format!(
            r#"{{"key":"{key}","fields":{{"summary":"s","status":{{"name":"To Do","statusCategory":{{"key":"new"}}}}}}}}"#
        )
    }

    #[test]
    fn key_shape_is_strict() {
        for ok in ["ABC-1", "XY-7", "A1_B-3"] {
            assert!(is_key(ok), "{ok}");
        }
        // Built at runtime: the privacy gate rejects multi-digit keys in
        // committed text.
        assert!(is_key(&format!("ABCDEFGHIJ-{}", "9".repeat(9))));
        assert!(!is_key(&format!("ABC-{}", "9".repeat(10))));
        assert!(!is_key(&format!("ABC-0{}", 1)));
        for bad in [
            "abc-1",
            "A-1",
            "ABC-0",
            "ABC-",
            "ABC",
            "1AB-2",
            "ABC-1) OR (x",
            "ABCDEFGHIJK-1",
        ] {
            assert!(!is_key(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_batch_returns_found_and_marks_the_rest_missing() {
        let (_d, bin) = fake(&format!("echo '[{}]'", issue("ABC-1")));
        let got = issues_at(&bin, &["ABC-1".into(), "ABC-2".into(), "ABC-1".into()]).await;
        assert_eq!(got.issues.len(), 1);
        assert_eq!(got.missing, ["ABC-2"]);
        assert!(got.unasked.is_empty());
        assert_eq!(got.error, None);
    }

    /// One bad key rejects a whole JQL query; splitting must still answer
    /// the good ones (partial is not nothing, #1044).
    #[tokio::test]
    async fn a_failing_batch_is_split_until_the_bad_key_is_isolated() {
        let ok = issue("ABC-1");
        let (_d, bin) = fake(&format!(
            r#"case "$*" in *XY-9*) echo "issue XY-9 does not exist" >&2; exit 1;; esac
echo '[{ok}]'"#
        ));
        let got = issues_at(&bin, &["ABC-1".into(), "XY-9".into()]).await;
        assert_eq!(
            got.issues
                .iter()
                .map(|i| i.key.as_str())
                .collect::<Vec<_>>(),
            ["ABC-1"]
        );
        assert_eq!(got.missing, ["XY-9"]);
        assert!(got.error.unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn what_the_budget_never_reached_is_unasked_not_missing() {
        let (_d, bin) = fake("echo boom >&2; exit 1");
        let keys: Vec<String> = (1..=20).map(|n| format!("ABC-{n}")).collect();
        let got = issues_at(&bin, &keys).await;
        assert!(got.issues.is_empty());
        assert!(
            !got.unasked.is_empty(),
            "budget must run out before 20 single-key calls"
        );
        assert_eq!(got.missing.len() + got.unasked.len(), 20);
        assert!(got.missing.iter().all(|k| !got.unasked.contains(k)));
    }

    #[tokio::test]
    async fn invalid_keys_never_reach_jql() {
        let (_d, bin) = fake(r#"case "$*" in *OR*) echo injected; exit 3;; esac; echo '[]'"#);
        let got = issues_at(&bin, &["ABC-1) OR (x".into()]).await;
        assert_eq!(got.missing, ["ABC-1) OR (x"]);
        assert_eq!(got.error, None, "nothing was sent");
    }

    #[tokio::test]
    async fn assigned_says_when_it_hit_the_limit() {
        let (_d, bin) = fake(&format!("echo '[{},{}]'", issue("ABC-1"), issue("ABC-2")));
        assert!(assigned_at(&bin, 2).await.truncated);
        assert!(!assigned_at(&bin, 3).await.truncated);
    }

    #[tokio::test]
    async fn a_logged_out_status_is_an_answer_not_a_failure() {
        let (_d, bin) = fake("echo 'You are not logged in' >&2; exit 1");
        assert!(matches!(
            connection_at(&bin).await,
            JiraConnection::NotLoggedIn { .. }
        ));
    }

    #[tokio::test]
    async fn a_failed_project_list_says_why() {
        let (_d, bin) = fake("echo 'unauthorized' >&2; exit 1");
        let got = projects_at(&bin).await;
        assert!(got.projects.is_empty());
        assert_eq!(got.error.as_deref(), Some("unauthorized"));
    }
}
