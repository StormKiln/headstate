//! What "Report this" can say about the machine it runs on (#1575).
//!
//! The banner that most needs a report -- "Background refresh failed:
//! GitHub request timed out after 30s" -- carried nothing but its own
//! sentence. The maintainer's first questions are always the same: which
//! request, how long it actually took, whether it is every poll or one in
//! twenty, how much of the hourly budget was left, which `gh`, which
//! build. None of that is in the error string, and all of it is known
//! here.
//!
//! # Scrubbed on this side, and again on the other
//!
//! Every free-text field goes through [`crate::redact::redact`] before it
//! leaves this module, so no caller -- the webview or a paired phone --
//! receives a token or a home path. The frontend then runs its own
//! `scrub()` over the whole report, which ALSO removes repository names:
//! a report goes to a public tracker, which this bundle does not.
//!
//! # Absent is not zero
//!
//! Every figure that could not be read is `None`, never `0` or `""`, and
//! the report prints it as "unknown". A budget nobody has reported yet
//! is a cold start, not an exhausted pool; a poll history with no ticks
//! is "not polled yet", not "never failed".

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How many ticks the history keeps.
///
/// Twenty ticks is twenty to forty minutes at the default cadence, long
/// enough to tell "every poll" from "one in ten" and short enough that
/// the report stays a paragraph.
pub const HISTORY_LEN: usize = 20;

/// The last lines of the diagnostic log a report carries.
///
/// The frontend trims further to fit GitHub's URL limit; this is only
/// the ceiling on what crosses the bridge.
pub const LOG_LINES: usize = 80;

/// Bytes read from the end of the log to find [`LOG_LINES`] lines.
const LOG_BYTES: u32 = 16 * 1024;

/// How long `gh --version` may take before the report says "unknown".
///
/// A local binary answering its own version. If it has not answered by
/// then, it is not going to in time for a report the user is waiting on.
const GH_PROBE: Duration = Duration::from_secs(3);

/// One finished poll tick, as the report describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollTickRecord {
    /// When the tick finished, in Unix milliseconds.
    pub at_unix_ms: u64,
    /// Whether the open-pull-request search answered.
    pub ok: bool,
    /// How long that search took, including a timeout.
    pub fetch_ms: u64,
    /// The failure, redacted. `None` on success.
    pub error: Option<String>,
    /// The ceiling it hit, when the failure was a timeout.
    pub timed_out_after_secs: Option<u64>,
    /// Consecutive failures including this one. Zero on success.
    pub attempt: u32,
    /// What the review-queue fetch in the same tick did: `ok`, `failed`,
    /// `timed out` or `skipped`.
    pub reviewing: String,
}

/// The poll loop's recent past, for a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollReport {
    /// Which request the banner's failures come from.
    pub operation: String,
    /// The per-fetch ceiling.
    pub fetch_timeout_secs: u64,
    /// The whole tick's ceiling, shared by both fetches.
    pub tick_timeout_secs: u64,
    /// The configured focused interval. `None` when it could not be read.
    pub focused_interval_secs: Option<u64>,
    /// How long the loop last chose to wait. `None` before the first tick.
    pub last_wait_secs: Option<u64>,
    /// Ticks since launch.
    pub ticks_recorded: u64,
    /// The most recent ticks, oldest first.
    pub recent: Vec<PollTickRecord>,
    /// Failures among `recent`.
    pub failures_in_recent: u32,
    /// Seconds since the last successful tick. `None` when there has not
    /// been one since launch -- which `ticks_recorded` distinguishes from
    /// "not polled yet".
    pub last_success_secs_ago: Option<u64>,
}

/// The rolling record behind [`PollReport`].
#[derive(Debug, Default)]
pub struct PollHistory {
    ticks: VecDeque<PollTickRecord>,
    total: u64,
    last_success_unix_ms: Option<u64>,
    last_wait_secs: Option<u64>,
}

impl PollHistory {
    pub fn push(&mut self, tick: PollTickRecord) {
        self.total += 1;
        if tick.ok {
            self.last_success_unix_ms = Some(tick.at_unix_ms);
        }
        if self.ticks.len() == HISTORY_LEN {
            self.ticks.pop_front();
        }
        self.ticks.push_back(tick);
    }

    pub fn note_wait(&mut self, secs: u64) {
        self.last_wait_secs = Some(secs);
    }

    pub fn report(&self, now_unix_ms: u64, focused_interval_secs: Option<u64>) -> PollReport {
        PollReport {
            operation: "the open pull request search (GraphQL)".to_string(),
            fetch_timeout_secs: crate::poll::FETCH_TIMEOUT.as_secs(),
            tick_timeout_secs: crate::poll::TICK_TIMEOUT.as_secs(),
            focused_interval_secs,
            last_wait_secs: self.last_wait_secs,
            ticks_recorded: self.total,
            recent: self.ticks.iter().cloned().collect(),
            failures_in_recent: self.ticks.iter().filter(|t| !t.ok).count() as u32,
            last_success_secs_ago: self
                .last_success_unix_ms
                .map(|at| now_unix_ms.saturating_sub(at) / 1000),
        }
    }
}

/// The loop's history. One loop per process, so one record.
static HISTORY: LazyLock<Mutex<PollHistory>> = LazyLock::new(Mutex::default);

/// Record a finished tick. Called by the poll loop only.
pub fn record_tick(tick: PollTickRecord) {
    if let Ok(mut h) = HISTORY.lock() {
        h.push(tick);
    }
}

/// Record how long the loop is about to wait.
pub fn note_wait(secs: u64) {
    if let Ok(mut h) = HISTORY.lock() {
        h.note_wait(secs);
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How the app was installed, in the bug form's own dropdown words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallGuess {
    /// One of the form's options exactly, or `None` when it cannot be
    /// told. Never a guess dressed as a fact: `basis` says what it rests
    /// on, and the report prints both.
    pub method: Option<String>,
    /// What the answer rests on, in a phrase.
    pub basis: String,
}

/// Decide the install method from what the process can see.
///
/// Pure, so it is testable without being any of these installs.
pub fn classify_install(os: &str, debug: bool, exe: Option<&Path>, appimage: bool) -> InstallGuess {
    let guess = |m: &str, basis: &str| InstallGuess {
        method: Some(m.to_string()),
        basis: basis.to_string(),
    };
    if debug {
        return guess("Built from source", "a debug build");
    }
    match os {
        "linux" if appimage => guess("Linux AppImage", "the AppImage runtime is set"),
        "linux" if exe.is_some_and(|p| p.starts_with("/usr")) => guess(
            "Linux .deb",
            "installed under /usr, where the .deb places it",
        ),
        "macos"
            if exe.is_some_and(|p| {
                p.components()
                    .any(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
            }) =>
        {
            guess(
                "macOS DMG",
                "a release build running from an application bundle",
            )
        }
        "windows" => guess("Windows installer", "a release build on Windows"),
        _ => InstallGuess {
            method: None,
            basis: "nothing identified the package".to_string(),
        },
    }
}

/// Everything a report can carry about this machine, redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticBundle {
    /// The running build's version, from the bundle rather than Cargo.
    pub app_version: String,
    pub os: String,
    pub arch: String,
    /// "macOS 15.6" and the like. `None` when the OS would not say.
    pub os_version: Option<String>,
    pub install: InstallGuess,
    /// The first line of `gh --version`, redacted. `None` with
    /// `gh_note` saying why.
    pub gh_version: Option<String>,
    pub gh_note: Option<String>,
    pub poll: PollReport,
    /// GraphQL points left this hour, as last reported. `None` before
    /// any response carried a figure.
    pub graphql_remaining: Option<u64>,
    /// REST (core) requests left this hour, as last reported.
    pub rest_remaining: Option<u64>,
    /// Whether `[diag]` lines are being written right now.
    pub diagnostics_on: bool,
    /// The log's last lines, redacted, when diagnostics are on.
    pub log_tail: Option<String>,
    /// Why there is no tail, when there is not.
    pub log_note: Option<String>,
}

/// The first line of `gh --version`, or why there is none.
pub async fn gh_version() -> (Option<String>, Option<String>) {
    let Some(gh) = crate::auth::find_gh() else {
        return (None, Some("gh was not found".to_string()));
    };
    let probe = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new(&gh).arg("--version").output()
    });
    match tokio::time::timeout(GH_PROBE, probe).await {
        Err(_) => (
            None,
            Some(format!(
                "gh --version did not answer within {}s",
                GH_PROBE.as_secs()
            )),
        ),
        Ok(Ok(Ok(out))) => {
            let text = String::from_utf8_lossy(if out.stdout.is_empty() {
                &out.stderr
            } else {
                &out.stdout
            })
            .into_owned();
            match text.lines().next().map(str::trim).filter(|l| !l.is_empty()) {
                Some(line) => (Some(crate::redact::redact(line)), None),
                None => (None, Some("gh --version printed nothing".to_string())),
            }
        }
        Ok(Ok(Err(e))) => (
            None,
            Some(crate::redact::redact(&format!(
                "gh --version could not run: {e}"
            ))),
        ),
        Ok(Err(e)) => (None, Some(format!("gh --version could not run: {e}"))),
    }
}

/// Build the bundle. The command in `commands.rs` supplies what only an
/// `AppHandle` can reach.
pub async fn bundle(
    app_version: String,
    focused_interval_secs: Option<u64>,
    log_file: Option<std::path::PathBuf>,
) -> DiagnosticBundle {
    let os = std::env::consts::OS;
    let exe = std::env::current_exe().ok();
    let install = classify_install(
        os,
        cfg!(debug_assertions),
        exe.as_deref(),
        std::env::var_os("APPIMAGE").is_some(),
    );
    let (gh_version, gh_note) = gh_version().await;
    let diagnostics_on = crate::diag::enabled();
    let (log_tail, log_note, os_version) = tauri::async_runtime::spawn_blocking(move || {
        let (tail, note) = log_tail(diagnostics_on, log_file.as_deref());
        (tail, note, sysinfo::System::long_os_version())
    })
    .await
    .unwrap_or_else(|e| (None, Some(format!("the log could not be read: {e}")), None));
    DiagnosticBundle {
        app_version,
        os: os.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        os_version,
        install,
        gh_version,
        gh_note,
        poll: poll_report(focused_interval_secs),
        graphql_remaining: crate::github::stats::budget::observed_remaining(),
        rest_remaining: crate::github::stats::budget::observed_rest_remaining(),
        diagnostics_on,
        log_tail,
        log_note,
    }
}

/// The last [`LOG_LINES`] lines of `file`, or why there are none.
pub fn log_tail(diagnostics_on: bool, file: Option<&Path>) -> (Option<String>, Option<String>) {
    if !diagnostics_on {
        return (None, Some("diagnostic logging is off".to_string()));
    }
    let Some(file) = file else {
        return (
            None,
            Some("the log directory could not be located".to_string()),
        );
    };
    match crate::diag::tail::read(file, LOG_BYTES) {
        Ok(t) => {
            // `\r\n` normalised before splitting on `\n`, or a Windows
            // log's lines keep a trailing `\r` each.
            let text = t.text.replace("\r\n", "\n");
            let lines: Vec<&str> = text.lines().collect();
            let keep = &lines[lines.len().saturating_sub(LOG_LINES)..];
            if keep.is_empty() {
                (None, Some("the log is empty".to_string()))
            } else {
                // `tail::read` redacts already; the path it returns is
                // deliberately not carried.
                (Some(keep.join("\n")), None)
            }
        }
        Err(crate::diag::tail::TailError::NotFound { .. }) => {
            (None, Some("no log has been written yet".to_string()))
        }
        Err(crate::diag::tail::TailError::Unreadable { why, .. }) => (
            None,
            Some(crate::redact::redact(&format!(
                "the log could not be read: {why}"
            ))),
        ),
    }
}

/// The poll history as of now.
pub fn poll_report(focused_interval_secs: Option<u64>) -> PollReport {
    let now = now_unix_ms();
    match HISTORY.lock() {
        Ok(h) => h.report(now, focused_interval_secs),
        Err(p) => p.into_inner().report(now, focused_interval_secs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(at: u64, ok: bool) -> PollTickRecord {
        PollTickRecord {
            at_unix_ms: at,
            ok,
            fetch_ms: 30_000,
            error: (!ok).then(|| "GitHub request timed out after 30s".to_string()),
            timed_out_after_secs: (!ok).then_some(30),
            attempt: u32::from(!ok),
            reviewing: "skipped".to_string(),
        }
    }

    /// No tick yet is not "never failed" and not "last success 0s ago".
    #[test]
    fn an_empty_history_states_nothing_it_does_not_know() {
        let r = PollHistory::default().report(10_000, None);
        assert_eq!(r.ticks_recorded, 0);
        assert_eq!(r.last_success_secs_ago, None);
        assert_eq!(r.last_wait_secs, None);
        assert_eq!(r.focused_interval_secs, None);
        assert!(r.recent.is_empty());
    }

    /// The history is bounded and counts failures in what it kept.
    #[test]
    fn the_history_keeps_the_last_ticks_and_counts_their_failures() {
        let mut h = PollHistory::default();
        h.push(tick(1_000, true));
        for i in 0..(HISTORY_LEN as u64 + 5) {
            h.push(tick(2_000 + i * 1_000, i % 2 == 0));
        }
        let r = h.report(100_000, Some(60));
        assert_eq!(r.recent.len(), HISTORY_LEN);
        assert_eq!(r.ticks_recorded, HISTORY_LEN as u64 + 6);
        assert_eq!(
            r.failures_in_recent as usize,
            r.recent.iter().filter(|t| !t.ok).count()
        );
        assert!(r.failures_in_recent > 0);
        // The last success is the newest ok tick, not the first.
        let newest_ok = r.recent.iter().rev().find(|t| t.ok).unwrap().at_unix_ms;
        assert_eq!(r.last_success_secs_ago, Some((100_000 - newest_ok) / 1000));
    }

    /// Failures only since launch: the last-success figure stays absent.
    #[test]
    fn only_failures_leave_the_last_success_unknown() {
        let mut h = PollHistory::default();
        h.push(tick(1_000, false));
        h.push(tick(2_000, false));
        let r = h.report(5_000, Some(60));
        assert_eq!(r.ticks_recorded, 2);
        assert_eq!(r.failures_in_recent, 2);
        assert_eq!(r.last_success_secs_ago, None);
    }

    /// Each dropdown answer is one of the bug form's options exactly --
    /// anything else would not select in the form.
    #[test]
    fn every_install_answer_is_a_bug_form_option() {
        let form = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(".github")
                .join("ISSUE_TEMPLATE")
                .join("bug_report.yml"),
        )
        .expect("the bug form is readable")
        .replace("\r\n", "\n");
        let cases = [
            classify_install("macos", true, None, false),
            classify_install("linux", false, None, true),
            classify_install("linux", false, Some(Path::new("/usr/bin/headstate")), false),
            classify_install(
                "macos",
                false,
                Some(Path::new(
                    "/Applications/Headstate.app/Contents/MacOS/headstate",
                )),
                false,
            ),
            classify_install("windows", false, None, false),
        ];
        for c in cases {
            let m = c.method.expect("each case identifies a method");
            assert!(
                form.contains(&format!("- {m}\n")),
                "{m:?} is not an option in bug_report.yml"
            );
        }
    }

    /// A binary somewhere unrecognised is not claimed to be anything.
    #[test]
    fn an_unrecognised_install_is_unknown_not_guessed() {
        let g = classify_install("linux", false, Some(Path::new("/opt/x/headstate")), false);
        assert_eq!(g.method, None);
        let g = classify_install("macos", false, Some(Path::new("/opt/x/headstate")), false);
        assert_eq!(g.method, None);
    }

    /// Off means no log, and the note says why rather than leaving a
    /// blank the report would print as nothing.
    #[test]
    fn a_log_tail_is_only_read_with_diagnostics_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("headstate.log");
        std::fs::write(&file, "one\r\ntwo\r\n").unwrap();
        let (tail, note) = log_tail(false, Some(&file));
        assert_eq!(tail, None);
        assert_eq!(note.as_deref(), Some("diagnostic logging is off"));

        let (tail, note) = log_tail(true, Some(&file));
        assert_eq!(tail.as_deref(), Some("one\ntwo"));
        assert_eq!(note, None);

        let (tail, note) = log_tail(true, Some(&dir.path().join("absent.log")));
        assert_eq!(tail, None);
        assert_eq!(note.as_deref(), Some("no log has been written yet"));
    }

    /// The tail is bounded to its last lines, and redacted.
    #[test]
    fn a_log_tail_keeps_its_last_lines_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("headstate.log");
        let mut body = String::new();
        for i in 0..(LOG_LINES + 40) {
            body.push_str(&format!("line {i} at /Users/alice/code\n"));
        }
        std::fs::write(&file, body).unwrap();
        let (tail, _) = log_tail(true, Some(&file));
        let tail = tail.unwrap();
        assert_eq!(tail.lines().count(), LOG_LINES);
        assert!(tail.ends_with(&format!("line {} at [path]", LOG_LINES + 39)));
        assert!(!tail.contains("alice"));
    }
}
