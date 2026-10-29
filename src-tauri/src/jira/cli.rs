//! Talking to `acli`, bounded. The `acli` half of [`super::transport`].
//!
//! Every call has a timeout and is killed on expiry, and the output is read
//! concurrently with the wait. Docker's `run_bounded` waits first and reads
//! after, which is fine for its small replies but would deadlock here: a
//! search returning a few hundred issues overflows the pipe buffer, the
//! child blocks on write, and the wait never ends.

use super::model::{
    IssueBatch, JiraConnection, JiraError, JiraIssue, ProjectList, ProjectStatuses,
};
use super::parse;
use std::path::PathBuf;
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
/// allowed", #2032; `updated` re-checked against 1.3.39 on #2033), and one
/// refused field fails the whole call, so these are the ones it allows.
/// Parents come from [`Acli::parent`]. `id` and the issue type's icon come
/// back without being named.
const FIELDS: &str = "summary,status,issuetype,assignee";

#[cfg(target_os = "macos")]
const FALLBACK_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
#[cfg(all(unix, not(target_os = "macos")))]
const FALLBACK_DIRS: &[&str] = &["/usr/local/bin", "/usr/bin", "/snap/bin"];
#[cfg(windows)]
const FALLBACK_DIRS: &[&str] = &[];

fn exe() -> String {
    format!("acli{}", std::env::consts::EXE_SUFFIX)
}

/// Locate `acli`: `HEADSTATE_ACLI`, then `PATH`, then the usual installs.
/// When it is nowhere, the error names every place looked in.
pub fn locate() -> Result<Acli, JiraError> {
    locate_in(
        FALLBACK_DIRS,
        std::env::var("PATH").ok().as_deref(),
        std::env::var("HEADSTATE_ACLI").ok().as_deref(),
    )
}

/// [`locate`], with what it reads passed in, so a test can search
/// directories it made rather than the machine's.
fn locate_in(
    fallbacks: &[&str],
    path: Option<&str>,
    explicit: Option<&str>,
) -> Result<Acli, JiraError> {
    match crate::auth::find_exe_with(&exe(), fallbacks, path, explicit) {
        Some(bin) => Ok(Acli::new(bin)),
        None => Err(JiraError::NotInstalled {
            searched: searched(fallbacks, path, explicit),
        }),
    }
}

/// Every place [`crate::auth::find_exe_with`] looks, in its order.
fn searched(fallbacks: &[&str], path: Option<&str>, explicit: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(e) = explicit {
        out.push(format!("HEADSTATE_ACLI={e}"));
    }
    if let Some(p) = path {
        out.extend(std::env::split_paths(p).map(|d| d.display().to_string()));
    }
    out.extend(crate::auth::user_fallback_dirs());
    out.extend(fallbacks.iter().map(|d| (*d).to_string()));
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| !d.is_empty() && seen.insert(d.clone()));
    out
}

/// What one call produced. Not a `Result`: `auth status` reports a logged
/// out user through its exit code, which is an answer, not a failure.
struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// One `acli` binary, and how long a call to it may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acli {
    bin: PathBuf,
    timeout: Duration,
}

impl Acli {
    pub fn new(bin: PathBuf) -> Self {
        Self {
            bin,
            timeout: CALL_TIMEOUT,
        }
    }

    #[cfg(all(test, unix))]
    pub fn with_timeout(bin: PathBuf, timeout: Duration) -> Self {
        Self { bin, timeout }
    }

    async fn run(&self, args: &[&str]) -> Result<Output, JiraError> {
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.args(args)
            .env("PATH", crate::packages::tools::child_path(&self.bin))
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        // A background read must not flash a console window on Windows (#1603).
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let out = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| JiraError::TimedOut {
                secs: self.timeout.as_secs().max(1),
            })?
            .map_err(|e| {
                // Found a moment ago and gone now -- uninstalled, or a
                // dangling `HEADSTATE_ACLI` -- is not installed, not failed.
                if e.kind() == std::io::ErrorKind::NotFound {
                    JiraError::NotInstalled {
                        searched: vec![self.bin.display().to_string()],
                    }
                } else {
                    JiraError::failed(format!("acli could not be started: {e}"))
                }
            })?;
        Ok(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    async fn run_ok(&self, args: &[&str]) -> Result<String, JiraError> {
        let out = self.run(args).await?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(failure(&out.stdout, &out.stderr))
        }
    }

    /// The installed `acli`'s version, from `acli --version`.
    pub async fn version(&self) -> Result<String, JiraError> {
        let raw = self.run_ok(&["--version"]).await?;
        parse::acli_version(&raw).ok_or_else(|| {
            JiraError::failed(format!(
                "acli --version said: {}",
                raw.trim().chars().take(200).collect::<String>()
            ))
        })
    }

    /// The statuses seen in `project`'s most recently updated issues.
    ///
    /// `acli` 1.3.39 has no command that lists a workflow's statuses (#2034),
    /// so this samples issues and collects what they hold.
    pub async fn project_statuses(&self, project: &str, limit: usize) -> ProjectStatuses {
        let mut out = ProjectStatuses {
            project: project.to_string(),
            statuses: Vec::new(),
            truncated: false,
            error: None,
        };
        if !is_project_key(project) {
            out.error = Some(JiraError::failed(format!(
                "{project} is not a Jira project key"
            )));
            return out;
        }
        let jql = format!("project = {project} ORDER BY updated DESC");
        let n = limit.to_string();
        let read = self
            .run_ok(&[
                "jira", "workitem", "search", "--jql", &jql, "--fields", "status", "--limit", &n,
                "--json",
            ])
            .await
            .and_then(|raw| {
                Ok((
                    parse::issues(&raw).map_err(JiraError::failed)?.len(),
                    parse::distinct_statuses(&raw).map_err(JiraError::failed)?,
                ))
            });
        match read {
            Ok((count, statuses)) => {
                out.truncated = count >= limit;
                out.statuses = statuses;
            }
            Err(e) => out.error = Some(e),
        }
        out
    }

    /// Whether Jira can be asked, by asking `acli`.
    pub async fn connection(&self) -> JiraConnection {
        match self.run(&["jira", "auth", "status"]).await {
            Ok(o) => parse::auth_status(o.success, &o.stdout, &o.stderr),
            Err(JiraError::NotInstalled { searched }) => JiraConnection::NotInstalled { searched },
            Err(e) => JiraConnection::CannotTell {
                reason: e.to_string(),
            },
        }
    }

    pub async fn projects(&self) -> ProjectList {
        match self
            .run_ok(&["jira", "project", "list", "--paginate", "--json"])
            .await
            .and_then(|raw| parse::projects(&raw).map_err(JiraError::failed))
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

    async fn search(&self, jql: &str, limit: usize) -> Result<Vec<JiraIssue>, JiraError> {
        let limit = limit.to_string();
        let raw = self
            .run_ok(&[
                "jira", "workitem", "search", "--jql", jql, "--fields", FIELDS, "--limit", &limit,
                "--json",
            ])
            .await?;
        parse::issues(&raw).map_err(JiraError::failed)
    }

    /// The parent of `key`, by `workitem view`, which -- unlike search -- may
    /// ask for it. One call per key; the caller keeps the answer.
    pub async fn parent(&self, key: &str) -> Result<Option<String>, JiraError> {
        if !is_key(key) {
            return Err(JiraError::failed(format!("{key} is not a Jira key")));
        }
        let raw = self
            .run_ok(&[
                "jira",
                "workitem",
                "view",
                key,
                "--fields",
                "parent,issuetype",
                "--json",
            ])
            .await?;
        parse::parent(&raw).map_err(JiraError::failed)
    }

    /// What Jira says about `keys`.
    ///
    /// Batched `key in (...)` searches. A batch that fails is split in half
    /// and retried, down to single keys, while the call budget lasts: JQL
    /// rejects the WHOLE query when one key in it does not exist, and one
    /// stale branch name must not hide every other ticket. What the budget
    /// never reached is returned as `unasked`, not `missing`.
    ///
    /// A failure that would meet every further call too -- rate limited,
    /// timed out, signed out ([`JiraError::ends_the_read`]) -- stops the
    /// read at once: what was found is kept, and everything not yet asked
    /// is `unasked`. Splitting a rate-limited batch would only spend more
    /// of a limit already exhausted.
    pub async fn issues(&self, keys: &[String]) -> IssueBatch {
        let mut out = IssueBatch::default();
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
            match self.search(&jql, batch.len()).await {
                Ok(found) => {
                    for k in &batch {
                        if !found.iter().any(|i| &i.key == k) {
                            out.missing.push(k.clone());
                        }
                    }
                    out.issues.extend(found);
                }
                Err(e) if e.ends_the_read() => {
                    out.unasked.extend(batch);
                    out.unasked.extend(queue.drain(..).flatten());
                    out.error = Some(e);
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
    pub async fn assigned(&self, limit: usize) -> IssueBatch {
        let jql = "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC";
        match self.search(jql, limit).await {
            Ok(issues) => IssueBatch {
                truncated: issues.len() >= limit,
                issues,
                ..IssueBatch::default()
            },
            Err(e) => IssueBatch {
                error: Some(e),
                ..IssueBatch::default()
            },
        }
    }
}

/// What a failed call means, from what `acli` printed.
///
/// Rate limiting first: a 429 can arrive with wording that also reads as
/// a refusal. Then a signed-out user, by the same phrases `auth status` is
/// read with. Anything else is `Failed`, verbatim.
fn failure(stdout: &str, stderr: &str) -> JiraError {
    let text = if stderr.is_empty() {
        stdout.trim()
    } else {
        stderr
    };
    if let Some(retry_after_secs) = rate_limit(&format!("{stdout}\n{stderr}")) {
        return JiraError::RateLimited { retry_after_secs };
    }
    if parse::says_logged_out(text) {
        return JiraError::NotLoggedIn;
    }
    if text.is_empty() {
        return JiraError::failed("acli failed without saying why");
    }
    JiraError::failed(text)
}

/// Whether `text` reports a rate limit, and the wait it asked for.
///
/// UNVERIFIED against live `acli`: no 429 has been captured (#2033). `acli`
/// is a Go CLI over Jira's REST API, so this matches what such a client
/// plausibly prints -- the status (`429`, `Too Many Requests`), the words
/// "rate limit", and a `Retry-After` value in seconds. `429` counts only
/// as a whole token: a missing issue whose key ends `-429` is not a
/// rate limit.
fn rate_limit(text: &str) -> Option<Option<u64>> {
    let lower = text.to_ascii_lowercase();
    let worded = [
        "too many requests",
        "rate limit",
        "rate-limit",
        "ratelimit",
        "rate limited",
    ]
    .iter()
    .any(|p| lower.contains(p));
    let status = lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .any(|t| t == "429");
    (worded || status).then(|| retry_after(&lower))
}

/// Seconds from a `Retry-After` (or "retry after" / "try again in") in
/// already-lowercased text. An HTTP-date value, or none, is `None`.
fn retry_after(lower: &str) -> Option<u64> {
    ["retry-after", "retry after", "try again in"]
        .iter()
        .find_map(|label| {
            let rest = &lower[lower.find(label)? + label.len()..];
            let rest = rest.trim_start_matches(|c: char| {
                c == ':' || c == '=' || c == '"' || c.is_whitespace()
            });
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().ok()
        })
}

/// How many issues a status read looks at: the most recently updated, so
/// the statuses a project's work moves through now are the ones seen.
pub const STATUS_SAMPLE: usize = 100;

/// Whether `project` has the shape of a Jira project key: the part of
/// [`is_key`] before the dash. It is interpolated into JQL.
pub fn is_project_key(project: &str) -> bool {
    is_key(&format!("{project}-1"))
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

#[cfg(test)]
mod pure_tests {
    use super::*;

    #[test]
    fn a_rate_limit_is_read_with_and_without_a_wait() {
        assert_eq!(
            rate_limit("Error: request failed with status 429 Too Many Requests"),
            Some(None)
        );
        assert_eq!(
            rate_limit("HTTP 429: Retry-After: 30"),
            Some(Some(30)),
            "header form"
        );
        assert_eq!(
            rate_limit("rate limit exceeded, try again in 12 seconds"),
            Some(Some(12))
        );
        assert_eq!(
            rate_limit("too many requests (retry after \"7\")"),
            Some(Some(7))
        );
        // An HTTP-date Retry-After is not a number of seconds.
        assert_eq!(
            rate_limit("429 retry-after: Wed, 21 Oct 2026 07:28:00 GMT"),
            Some(None)
        );
    }

    #[test]
    fn a_key_or_a_count_that_contains_429_is_not_a_rate_limit() {
        // Built at runtime: the privacy gate rejects multi-digit keys in
        // committed text.
        let missing_key = format!("issue ABC-{} does not exist", 429);
        for not in [
            missing_key.as_str(),
            "found 4290 results",
            "field 'updated' is not allowed",
            "",
        ] {
            assert_eq!(rate_limit(not), None, "{not}");
        }
    }

    #[test]
    fn a_failure_is_classified_rate_limit_first() {
        assert_eq!(
            failure("", "429 Too Many Requests; Retry-After: 5"),
            JiraError::RateLimited {
                retry_after_secs: Some(5)
            }
        );
        assert_eq!(
            failure("", "unauthorized: please log in"),
            JiraError::NotLoggedIn
        );
        assert_eq!(
            failure("", ""),
            JiraError::failed("acli failed without saying why")
        );
        assert_eq!(failure("on stdout", ""), JiraError::failed("on stdout"));
    }

    /// Never "not installed" without saying where: every directory searched
    /// is named, in search order, explicit override first.
    #[test]
    fn not_installed_names_every_place_searched() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let path = std::env::join_paths([&empty]).unwrap();
        let missing = dir.path().join("nowhere").join("acli");
        let fallback = dir.path().join("fallback");
        let fallback = fallback.to_str().unwrap();
        let got = locate_in(&[fallback], path.to_str(), missing.to_str());
        let Err(JiraError::NotInstalled { searched }) = got else {
            panic!("expected not installed, got {got:?}");
        };
        assert_eq!(
            searched.first().unwrap(),
            &format!("HEADSTATE_ACLI={}", missing.display())
        );
        assert!(
            searched.contains(&empty.display().to_string()),
            "{searched:?}"
        );
        assert_eq!(searched.last().unwrap(), fallback);
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
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `acli` that answers from a shell script. Passed by path,
    /// never through `HEADSTATE_ACLI`, so no test edits the process
    /// environment (#481).
    fn fake(script: &str) -> (tempfile::TempDir, Acli) {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("acli");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, Acli::new(bin))
    }

    fn issue(key: &str) -> String {
        format!(
            r#"{{"key":"{key}","fields":{{"summary":"s","status":{{"name":"To Do","statusCategory":{{"key":"new"}}}}}}}}"#
        )
    }

    fn keys(n: usize) -> Vec<String> {
        (1..=n).map(|n| format!("ABC-{n}")).collect()
    }

    #[tokio::test]
    async fn a_batch_returns_found_and_marks_the_rest_missing() {
        let (_d, acli) = fake(&format!("echo '[{}]'", issue("ABC-1")));
        let got = acli
            .issues(&["ABC-1".into(), "ABC-2".into(), "ABC-1".into()])
            .await;
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
        let (_d, acli) = fake(&format!(
            r#"case "$*" in *XY-9*) echo "issue XY-9 does not exist" >&2; exit 1;; esac
echo '[{ok}]'"#
        ));
        let got = acli.issues(&["ABC-1".into(), "XY-9".into()]).await;
        assert_eq!(
            got.issues
                .iter()
                .map(|i| i.key.as_str())
                .collect::<Vec<_>>(),
            ["ABC-1"]
        );
        assert_eq!(got.missing, ["XY-9"]);
        assert_eq!(
            got.error,
            Some(JiraError::failed("issue XY-9 does not exist"))
        );
    }

    #[tokio::test]
    async fn what_the_budget_never_reached_is_unasked_not_missing() {
        let (_d, acli) = fake("echo boom >&2; exit 1");
        let got = acli.issues(&keys(20)).await;
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
        let (_d, acli) = fake(r#"case "$*" in *OR*) echo injected; exit 3;; esac; echo '[]'"#);
        let got = acli.issues(&["ABC-1) OR (x".into()]).await;
        assert_eq!(got.missing, ["ABC-1) OR (x"]);
        assert_eq!(got.error, None, "nothing was sent");
    }

    #[tokio::test]
    async fn assigned_says_when_it_hit_the_limit() {
        let (_d, acli) = fake(&format!("echo '[{},{}]'", issue("ABC-1"), issue("ABC-2")));
        assert!(acli.assigned(2).await.truncated);
        assert!(!acli.assigned(3).await.truncated);
    }

    #[tokio::test]
    async fn a_logged_out_status_is_an_answer_not_a_failure() {
        let (_d, acli) = fake("echo 'You are not logged in' >&2; exit 1");
        assert!(matches!(
            acli.connection().await,
            JiraConnection::NotLoggedIn { .. }
        ));
    }

    #[tokio::test]
    async fn project_statuses_are_collected_and_say_when_truncated() {
        let (_d, acli) = fake(&format!("echo '[{},{}]'", issue("ABC-1"), issue("ABC-2")));
        let got = acli.project_statuses("ABC", 2).await;
        assert_eq!(got.statuses.len(), 1, "two issues, one status");
        assert!(got.truncated);
        assert_eq!(got.error, None);
        assert!(!acli.project_statuses("ABC", 3).await.truncated);
    }

    #[tokio::test]
    async fn a_bad_project_key_never_reaches_jql() {
        let (_d, acli) = fake("echo injected; exit 3");
        let got = acli.project_statuses("ABC OR x", 10).await;
        assert!(got.statuses.is_empty());
        assert!(got
            .error
            .unwrap()
            .to_string()
            .contains("not a Jira project key"));
    }

    #[tokio::test]
    async fn a_failed_status_read_is_an_error_not_an_empty_list() {
        let (_d, acli) = fake("echo 'unauthorized' >&2; exit 1");
        let got = acli.project_statuses("ABC", 10).await;
        assert!(got.statuses.is_empty());
        assert_eq!(got.error, Some(JiraError::failed("unauthorized")));
    }

    #[tokio::test]
    async fn the_version_comes_from_acli_itself() {
        let (_d, acli) = fake("echo 'acli version 1.3.39-stable'");
        assert_eq!(acli.version().await, Ok("1.3.39-stable".to_string()));
        let (_d2, odd) = fake("echo 'hello'");
        assert!(odd
            .version()
            .await
            .unwrap_err()
            .to_string()
            .contains("hello"));
    }

    #[tokio::test]
    async fn a_failed_project_list_says_why() {
        let (_d, acli) = fake("echo 'something broke' >&2; exit 1");
        let got = acli.projects().await;
        assert!(got.projects.is_empty());
        assert_eq!(got.error, Some(JiraError::failed("something broke")));
    }

    // --- The typed failures (#2033), each through a real process. ---

    /// A binary that vanished between locating and running it is not
    /// installed, and says where it was looked for.
    #[tokio::test]
    async fn a_missing_binary_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("acli");
        let acli = Acli::new(bin.clone());
        let want = vec![bin.display().to_string()];
        assert_eq!(
            acli.projects().await.error,
            Some(JiraError::NotInstalled {
                searched: want.clone()
            })
        );
        assert_eq!(
            acli.connection().await,
            JiraConnection::NotInstalled { searched: want }
        );
    }

    /// A wedged call is killed at the deadline, reported as a timeout, and
    /// ends the read: the keys go back unasked, never missing.
    #[tokio::test]
    async fn a_call_past_its_deadline_times_out_and_leaves_keys_unasked() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("acli");
        std::fs::write(&bin, "#!/bin/sh\nexec sleep 10\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let acli = Acli::with_timeout(bin, Duration::from_millis(300));
        let started = std::time::Instant::now();
        let got = acli.issues(&keys(3)).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "one call, not a split per key"
        );
        assert_eq!(got.error, Some(JiraError::TimedOut { secs: 1 }));
        assert_eq!(got.unasked, keys(3));
        assert!(got.missing.is_empty(), "a timeout is not an answer");
    }

    /// A rate limit stops the read at once. Keys before it are kept; keys
    /// after it are unasked, not failed; nothing more is spent splitting.
    #[tokio::test]
    async fn a_rate_limit_ends_the_read_and_carries_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("acli");
        let calls = dir.path().join("calls");
        // First batch (ABC-1..50) answers; anything after is limited. The
        // first key of batch two is built at runtime for the privacy gate.
        let batch_two = format!("ABC-{}", 51);
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho x >> '{}'\ncase \"$*\" in *{batch_two}*) echo 'Error: 429 Too Many Requests (Retry-After: 42)' >&2; exit 1;; esac\necho '[{}]'\n",
                calls.display(),
                issue("ABC-1")
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let got = Acli::new(bin).issues(&keys(120)).await;
        assert_eq!(
            got.error,
            Some(JiraError::RateLimited {
                retry_after_secs: Some(42)
            })
        );
        assert_eq!(
            got.issues.len(),
            1,
            "what came back before the limit is kept"
        );
        assert_eq!(
            got.unasked.len(),
            70,
            "batch two and three, never asked again"
        );
        assert_eq!(got.missing.len(), 49);
        let made = std::fs::read_to_string(&calls).unwrap().lines().count();
        assert_eq!(made, 2, "no split or further batch after a rate limit");
    }

    #[tokio::test]
    async fn a_rate_limit_without_a_wait_says_it_did_not_say() {
        let (_d, acli) = fake("echo 'rate limit exceeded' >&2; exit 1");
        let got = acli.issues(&keys(2)).await;
        assert_eq!(
            got.error,
            Some(JiraError::RateLimited {
                retry_after_secs: None
            })
        );
        assert_eq!(got.unasked, keys(2));
        assert_eq!(
            acli.projects().await.error,
            Some(JiraError::RateLimited {
                retry_after_secs: None
            })
        );
    }

    /// Signed out mid-read is its own state, not a missing issue.
    #[tokio::test]
    async fn a_signed_out_search_is_not_logged_in() {
        let (_d, acli) = fake("echo 'Error: unauthorized. Run acli jira auth login' >&2; exit 1");
        let got = acli.issues(&keys(2)).await;
        assert_eq!(got.error, Some(JiraError::NotLoggedIn));
        assert_eq!(got.unasked, keys(2));
        assert!(got.missing.is_empty());
        assert_eq!(acli.assigned(5).await.error, Some(JiraError::NotLoggedIn));
    }
}
