//! Which Claude Code sessions are running right now (#921, epic #910).
//!
//! # This module is a SEAM, and it is meant to be replaced
//!
//! #917 owns liveness properly: `claude/liveness.rs` in that change is a
//! three-state derivation (`Running` / `Dead` / `Unknown(why)`) keyed on
//! `(pid, pid_start_time)`, consulted per row, with the registry and the
//! recorded runs as its two sources. #921's overview page consumes that
//! answer and must not derive a second one -- two answers to one question
//! disagree the first time either changes, and on this page the
//! disagreement would show as a tile saying "3 running" beside a list
//! offering to resume one of the three.
//!
//! #917 had not merged when this landed, so this file supplies the ONE
//! fact [`super::overview::aggregate`] needs -- a set of session ids --
//! from the same source #917 reads, and nothing more. It deliberately
//! does not reimplement the tri-state: it answers "which ids are
//! positively running", and [`Live::failure`] carries the case where it
//! could not tell, so the page can say "could not tell" rather than
//! implying nothing is running.
//!
//! When #917 lands, `running_ids` becomes one call into `liveness::derive`
//! over the rows the list already resolved, and this module goes away.
//! `aggregate` takes the set as a parameter precisely so that swap is a
//! change to one call site.
//!
//! # Why an orphan registry file is not proof of a running session
//!
//! Measured on this machine (epic #910, the crash-signal comment): a
//! session SIGKILLed at 09:57 left its `~/.claude/sessions/<pid>.json`
//! behind, still present 20 seconds later, carrying a `status` frozen at
//! the moment of death -- or, in the #913 probe, no `status` at all.
//!
//! So the registry is a list of sessions that STARTED, not a list of
//! sessions that are running, and taking its file count as the live count
//! reports every crashed session as alive. Since a crashed session is
//! exactly what this page exists to help resurrect, that failure would
//! hide the rows the user came for.
//!
//! The file therefore has to be checked against the process table, and
//! against `procStart` as well as the pid: pids recycle, and a reissued
//! number would report a long-dead session as running. `health/runaway.rs`
//! already pairs `(pid, start_time)` for the same reason and is the
//! precedent this follows.
//!
//! # The session Headstate starts and cannot see (#1304)
//!
//! A user asked whether two Claudify "Run in terminal" clicks had left
//! orphaned sessions behind, and Headstate could not answer. Measured on
//! this machine, launching the shipped osascript preset with the exact
//! line `prompt_command` builds:
//!
//! ```text
//! process:  pid 47661, `claude '<prompt>'`, alive for minutes
//! ~/.claude/sessions/47661.json          ABSENT
//! ~/.claude/sessions/47661.<hash>.key    PRESENT
//! claude_run rows for pid 47661          0   (492 before, 492 after)
//! ```
//!
//! Repeated for an interactive `claude` with no prompt: same result. Five
//! live `claude` processes on the machine, and the registry published a
//! `.json` for three of them.
//!
//! So the gap is NOT that a launch is untracked because the pid Headstate
//! spawns is the terminal's. It is that **Claude Code itself publishes no
//! session record for these processes** -- only the `.key`. Every
//! downstream count keys on the `.json`: `crash.rs::record_running`
//! synthesizes the `claude_run` row from a registry sweep, so no `.json`
//! means no run row, which means the orphan predicate can never match and
//! the session is `never_observed` forever.
//!
//! Nothing here can invent the missing record. What it CAN do is stop
//! discarding the one piece of evidence that does exist: the `.key`
//! carries `procStart`, which is exactly the pid-reuse guard the `.json`
//! path already relies on. So a `.key` whose pid is confirmed alive is
//! reported through `unreadable` -- "running is at least N" -- rather than
//! being skipped as though nothing were there. That is the honest claim
//! and it is the whole extent of the claim: see [`key_only`] for why a
//! `.key` can never be promoted into a session, an orphan, or a run.
//!
//! # The format trap in `procStart`
//!
//! `procStart` is written as `Mon DD HH:MM:SS YYYY` in **UTC**, while
//! `ps -o lstart=` prints `DD Mon HH:MM:SS YYYY` in **local** time -- two
//! differences, and four hours apart on this machine. Both a string
//! comparison and a parse that fixes the field order but ignores the zone
//! fail in the same direction: every session reads as dead, and the
//! feature still LOOKS right because dead-but-resumable is the expected
//! state for 83% of rows.
//!
//! This module never reads `ps`. It parses `procStart` once as UTC and
//! compares epoch seconds against `sysinfo`'s `start_time()`, so the trap
//! stays in the text that is left behind.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How far apart the two clocks may be and still be the same process.
///
/// `procStart` is written by Claude Code from its own reading of the
/// process start time; `sysinfo` reads the kernel's. They are the same
/// instant in principle and were **0 seconds apart on all three live
/// sessions** when measured here -- so this window is slack for a
/// rounding difference or a platform that reports a coarser start time,
/// not a fudge covering a known disagreement.
///
/// It is small enough to still catch a recycled pid: a pid reissued
/// within 120 seconds of the original's start is the case this does not
/// distinguish, and the alternative -- no tolerance at all -- fails
/// closed on every platform whose start time is second-truncated
/// differently, which marks live sessions dead.
const START_TOLERANCE_SECS: i64 = 120;

/// The live session ids, and why the answer may be incomplete.
///
/// Two fields rather than a bare set, because an empty set has two very
/// different meanings and the page must not conflate them: "we read the
/// registry and nothing is running" is a settled answer, and "we could
/// not read the registry" is not. Rendering the second as the first is
/// #841's fail-open -- and here it would tell a user their running
/// session is not running.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// Sessions whose process was positively found, with a start time
    /// that matches what the registry recorded.
    pub ids: HashSet<String>,
    /// Why the registry could not be listed at all. `None` means it was
    /// read, so `ids` is a real answer.
    pub failure: Option<String>,
    /// Registry files present but unusable, with why. Each one hides a
    /// session whose state cannot be stated -- counted rather than
    /// skipped, because a skipped one silently becomes "not running".
    pub unreadable: Vec<String>,
}

/// `~/.claude/sessions`, where Claude Code publishes one file per
/// running session.
pub fn registry_dir() -> Option<PathBuf> {
    // `crate::auth::home_dir`, the same resolver `transcript::projects_dir`
    // uses -- so the two halves of `~/.claude` are never reached by two
    // different notions of where home is.
    crate::auth::home_dir().map(|h| h.join(".claude").join("sessions"))
}

/// What one registry file says.
struct Entry {
    session_id: String,
    pid: u32,
    /// `None` when the record carried no `procStart`. Such an entry
    /// cannot be checked against pid reuse, so it is NOT treated as
    /// running -- see [`running_ids`].
    proc_start: Option<i64>,
}

/// Parse `procStart`, which is `%a %b %e %H:%M:%S %Y` in UTC.
///
/// Returns epoch seconds. `None` for anything that does not parse, which
/// is handled as "cannot check" rather than as zero -- an epoch 0 would
/// compare as a 1970 start time and mark every session as pid-reused.
pub fn parse_proc_start(text: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text.trim(), "%a %b %e %H:%M:%S %Y")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// Read every registry record.
fn read_registry(dir: &Path) -> (Vec<Entry>, Option<String>, Vec<String>, Vec<KeyOnly>) {
    let listing = match std::fs::read_dir(dir) {
        Ok(l) => l,
        // A missing directory is NOT a failure: Claude Code creates it
        // when a session starts, so its absence on a machine with no
        // running sessions is the correct, settled answer "nothing is
        // running". Any other error is a failure we must report.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), None, Vec::new(), Vec::new())
        }
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("could not read {}: {e}", dir.display())),
                Vec::new(),
                Vec::new(),
            )
        }
    };

    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let mut key_candidates = Vec::new();
    // NOT `listing.flatten()`: that discards a per-entry error, and a
    // directory entry we could not read hides a session whose state we
    // then silently report as "not running".
    for item in listing {
        let path = match item {
            Ok(e) => e.path(),
            Err(e) => {
                unreadable.push(format!("a directory entry could not be read: {e}"));
                continue;
            }
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            // A `.key` beside its own `.json` is a companion file and
            // nothing more -- that is the case this used to skip
            // silently, and it is still skipped. But a `.key` with NO
            // sibling `.json` is a session Claude Code started and never
            // published a record for (#1304), which is not the same
            // thing at all. [`key_only`] tells the two apart; this loop
            // only collects the names, because deciding requires the
            // process table and that is [`running_ids`]'s job.
            if let Some(candidate) = key_only(&path, dir) {
                key_candidates.push(candidate);
            }
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                unreadable.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                unreadable.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let Some(session_id) = value.get("sessionId").and_then(|v| v.as_str()) else {
            unreadable.push(format!("{}: no sessionId", path.display()));
            continue;
        };
        let Some(pid) = value.get("pid").and_then(|v| v.as_u64()) else {
            unreadable.push(format!("{}: no pid", path.display()));
            continue;
        };
        entries.push(Entry {
            session_id: session_id.to_string(),
            pid: pid as u32,
            proc_start: value
                .get("procStart")
                .and_then(|v| v.as_str())
                .and_then(parse_proc_start),
        });
    }
    (entries, None, unreadable, key_candidates)
}

/// A session that published a `.key` and no `.json` (#1304).
///
/// Named for what it IS rather than for what it might mean: the only
/// two facts here are the pid the filename carries and the `procStart`
/// inside the file. There is deliberately no `session_id` field,
/// because the `.key` does not contain one -- see [`key_only`].
struct KeyOnly {
    pid: u32,
    /// `None` when the file carried no parseable `procStart`, which
    /// makes the record unusable rather than merely uninteresting: a
    /// pid with no start time cannot be checked against pid reuse.
    proc_start: Option<i64>,
    /// For the message, so a reader can go and look at the file.
    path: PathBuf,
}

/// Classify one non-`.json` registry file.
///
/// Returns `Some` only for a `.key` whose `<pid>.json` sibling does NOT
/// exist. The filename is `<pid>.<hash>.key`, so the pid is the first
/// dot-separated component.
///
/// # Why a `.key` cannot be promoted into a session, ever
///
/// **It carries no `sessionId`.** Measured (#1304): the whole file is
///
/// ```text
/// {"peerToken":"…","procStart":"Tue Sep 22 17:45:40 2026","pidDomain":"darwin"}
/// ```
///
/// That is the constraint that makes this change safe, and it is worth
/// stating as a guarantee rather than an accident. Every downstream
/// claim Headstate makes about a session -- resumable, archived,
/// orphaned -- is keyed by session id. Without one, a `.key` record
/// *cannot* reach `claude_run`, cannot be counted as an orphan, and
/// cannot move a session out of `never_observed`. So the honesty rules
/// in #1304 are enforced by the data rather than by our restraint: the
/// worst this can do is say "something is running that we cannot
/// name", which is the true statement.
fn key_only(path: &Path, dir: &Path) -> Option<KeyOnly> {
    if path.extension().and_then(|e| e.to_str()) != Some("key") {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let pid_text = name.split('.').next()?;
    // A non-numeric leading component is not a pid, so it is not one of
    // these records at all -- skipped rather than reported, the same way
    // an unrelated file in the directory always has been.
    let pid: u32 = pid_text.parse().ok()?;
    if dir.join(format!("{pid}.json")).exists() {
        // The companion case: its session is already a full record and
        // is counted from the `.json`. Reporting it here would double
        // count the sessions that work.
        return None;
    }
    let proc_start = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("procStart")
                .and_then(|p| p.as_str())
                .and_then(parse_proc_start)
        });
    Some(KeyOnly {
        pid,
        proc_start,
        path: path.to_path_buf(),
    })
}

/// The start time the OS reports for a pid, in epoch seconds.
///
/// A trait so the matching logic is testable without a real process:
/// the cases that matter are the ones a live machine cannot be made to
/// produce on demand -- a recycled pid, and a probe that could not look.
pub trait Probe {
    /// `Ok(None)` means the process is not there. `Err` means we could
    /// not tell, which is NOT the same thing and must not become "dead".
    fn start_time(&self, pid: u32) -> Result<Option<i64>, String>;
}

/// The real probe, over a `sysinfo::System` refreshed for just the pids
/// that could matter.
///
/// Scoped rather than a full refresh: on this machine that is 3 pids
/// against 1,461 sessions, so the cost is proportional to what is live
/// rather than to the history.
pub struct SysinfoProbe {
    system: sysinfo::System,
}

impl SysinfoProbe {
    pub fn for_pids(pids: &[u32]) -> Self {
        let mut system = sysinfo::System::new();
        if !pids.is_empty() {
            let wanted: Vec<sysinfo::Pid> =
                pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&wanted),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
        }
        Self { system }
    }
}

impl Probe for SysinfoProbe {
    fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
        // `sysinfo::Process::start_time()` is epoch SECONDS, which is
        // what makes the comparison with a parsed `procStart` sound --
        // pinned by `sysinfo_start_time_is_epoch_seconds` below rather
        // than taken from the docs.
        Ok(self
            .system
            .process(sysinfo::Pid::from_u32(pid))
            .map(|p| p.start_time() as i64))
    }
}

/// Which sessions are positively running.
///
/// The rule, and each arm is a decision rather than a fallthrough:
///
/// | registry record | process table | verdict |
/// |---|---|---|
/// | pid + `procStart` | found, start times agree | **running** |
/// | pid + `procStart` | found, start times differ | not running (pid reused) |
/// | pid + `procStart` | not found | not running (the orphan: a crash) |
/// | pid, no `procStart` | either | not running, and `unreadable` says why |
/// | — | probe errored | not running, and `unreadable` says why |
/// | `.key`, no `.json` | found, start times agree | **running but unnameable**, and `unreadable` says so (#1304) |
/// | `.key`, no `.json` | not found, or start times differ | nothing said: it ended and left the file |
///
/// Every "not running" here is a claim only about THIS set. The set is
/// used to subtract live sessions from the resumable count, so the
/// failure direction matters: a session wrongly excluded from `ids` is
/// offered as resumable when it is alive, which starts a second copy of
/// it. That is why the two "cannot tell" arms push onto `unreadable` --
/// the page renders that as "could not tell what is running" above the
/// counts, rather than letting the counts look settled.
pub fn running_ids<P: Probe>(entries_dir: &Path, probe_for: impl FnOnce(&[u32]) -> P) -> Live {
    let (entries, failure, mut unreadable, key_only) = read_registry(entries_dir);
    if failure.is_some() {
        return Live {
            ids: HashSet::new(),
            failure,
            unreadable,
        };
    }

    // Both sets of pids in one probe: `SysinfoProbe::for_pids` refreshes
    // exactly the pids it is given, so asking for them separately would
    // mean two scans of the process table for one answer.
    let pids: Vec<u32> = entries
        .iter()
        .map(|e| e.pid)
        .chain(key_only.iter().map(|k| k.pid))
        .collect();
    let probe = probe_for(&pids);

    let mut ids = HashSet::new();
    for entry in entries {
        let Some(recorded) = entry.proc_start else {
            unreadable.push(format!(
                "session {} records pid {} with no procStart, so a recycled pid \
                 cannot be ruled out",
                entry.session_id, entry.pid
            ));
            continue;
        };
        match probe.start_time(entry.pid) {
            Ok(Some(actual)) if (actual - recorded).abs() <= START_TOLERANCE_SECS => {
                ids.insert(entry.session_id);
            }
            // Found, but a different process wearing the same number.
            Ok(Some(_)) => {}
            // Not there: the orphan file, which is a crash signal rather
            // than a live session.
            Ok(None) => {}
            Err(e) => unreadable.push(format!(
                "could not check pid {} for session {}: {e}",
                entry.pid, entry.session_id
            )),
        }
    }

    // A `.key` with no `.json` is a session that IS running and that
    // nothing downstream can name (#1304). It goes in `unreadable`
    // rather than in `ids` for a reason that is the whole point of this
    // change: `ids` is a set of SESSION IDS, and this record has none,
    // so there is nothing to insert. `unreadable` is already defined as
    // "a session whose state cannot be stated", and the page already
    // renders it as "running is at least N rather than exactly N" --
    // which is exactly the claim this evidence supports.
    //
    // The same `procStart` guard as above, and for the sharper reason:
    // a bare pid with no start-time check would report whatever process
    // happens to hold that number as a leaked Claude session, and a
    // reader told "this is a leaked session" may go and kill it. A miss
    // is recoverable; that is not.
    for rec in key_only {
        let Some(recorded) = rec.proc_start else {
            unreadable.push(format!(
                "{}: a session record with no readable procStart, so whether \
                 pid {} is still that session cannot be checked",
                rec.path.display(),
                rec.pid
            ));
            continue;
        };
        match probe.start_time(rec.pid) {
            Ok(Some(actual)) if (actual - recorded).abs() <= START_TOLERANCE_SECS => {
                unreadable.push(format!(
                    "pid {} is running a Claude Code session that published no \
                     session id, so it is not counted in any figure below",
                    rec.pid
                ));
            }
            // A pid the file names that now belongs to something else,
            // or to nothing: the session ended and left its `.key`
            // behind. Not reported -- it is not running, so it is not
            // hiding from the running count, and saying anything here
            // would turn "we cleaned up" into a warning.
            Ok(Some(_)) | Ok(None) => {}
            Err(e) => unreadable.push(format!(
                "could not check pid {} from {}: {e}",
                rec.pid,
                rec.path.display()
            )),
        }
    }

    Live {
        ids,
        failure: None,
        unreadable,
    }
}

/// The whole answer for the default location.
pub fn running_ids_default() -> Live {
    match registry_dir() {
        Some(dir) => running_ids(&dir, SysinfoProbe::for_pids),
        None => Live {
            failure: Some("no home directory, so the live session registry is unreachable".into()),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe with scripted answers, so the three interesting cases are
    /// reachable without a real process.
    struct Fake(std::collections::HashMap<u32, Result<Option<i64>, String>>);
    impl Probe for Fake {
        fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
            self.0.get(&pid).cloned().unwrap_or(Ok(None))
        }
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// `std::env::temp_dir()`, not `/tmp`: two agents in this epic hit
    /// Windows-only failures from exactly that assumption.
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("headstate-live-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The trap: `procStart` is UTC in `Mon DD` order.
    ///
    /// Sabotage: parsing with `%a %e %b` (the `ps lstart` order) returns
    /// `None` for this string, and every session then lands in
    /// `unreadable` rather than in `ids` -- i.e. nothing is ever running.
    #[test]
    fn proc_start_parses_as_utc_in_month_day_order() {
        // 2026-09-11T09:43:48Z
        assert_eq!(
            parse_proc_start("Fri Sep 11 09:43:48 2026"),
            Some(1789119828)
        );
        // The `ps` rendering of the SAME instant is a different string in
        // a different zone, and must not parse as this format.
        assert_eq!(parse_proc_start("Fri 11 Sep 05:43:48 2026"), None);
    }

    /// `sysinfo` reports `start_time()` in epoch SECONDS.
    ///
    /// The load-bearing unit assumption behind the comparison. Pinned
    /// against this process, whose start time is necessarily in the past
    /// and within living memory -- milliseconds would be ~1000x larger
    /// and fail the upper bound.
    #[test]
    fn sysinfo_start_time_is_epoch_seconds() {
        let me = std::process::id();
        let probe = SysinfoProbe::for_pids(&[me]);
        let t = probe
            .start_time(me)
            .expect("our own process table entry is readable")
            .expect("we are running");
        let now = chrono::Utc::now().timestamp();
        assert!(t > 1_600_000_000, "not milliseconds, and not zero: {t}");
        assert!(t <= now + 5, "in the past: {t} vs {now}");
    }

    /// A matching start time is running.
    #[test]
    fn a_matching_start_time_is_running() {
        let dir = tmp("match");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"alive","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| {
            Fake([(100u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert!(live.ids.contains("alive"));
        assert!(live.unreadable.is_empty());
        assert_eq!(live.failure, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A recycled pid is NOT running.
    ///
    /// Sabotage: dropping the start-time comparison and keying on the pid
    /// alone puts `"recycled"` in `ids`, which would subtract a long-dead
    /// session from the resumable count -- hiding exactly the row the
    /// user came to resurrect.
    #[test]
    fn a_recycled_pid_is_not_running() {
        let dir = tmp("recycled");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"recycled","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| {
            // Same pid, a start time a day later: a different process.
            Fake(
                [(100u32, Ok(Some(1789119828 + 86400)))]
                    .into_iter()
                    .collect(),
            )
        });
        assert!(live.ids.is_empty(), "the number was reissued");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An orphan file -- a record whose process is gone -- is not running.
    ///
    /// Measured on this machine: a SIGKILLed session leaves its registry
    /// file behind. Sabotage: treating the file's presence as liveness
    /// counts every crashed session as alive, which removes it from the
    /// resumable list -- the exact opposite of what the page is for.
    #[test]
    fn an_orphaned_registry_file_is_not_running() {
        let dir = tmp("orphan");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"crashed","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| Fake([(100u32, Ok(None))].into_iter().collect()));
        assert!(live.ids.is_empty(), "the file survived the process");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A probe that could not look reports WHY, and claims nothing.
    #[test]
    fn a_probe_that_failed_is_reported_rather_than_read_as_dead() {
        let dir = tmp("probefail");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"unknowable","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| {
            Fake(
                [(100u32, Err("process table unreadable".into()))]
                    .into_iter()
                    .collect(),
            )
        });
        assert!(live.ids.is_empty());
        assert_eq!(live.unreadable.len(), 1);
        assert!(live.unreadable[0].contains("process table unreadable"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A record with no `procStart` cannot be checked, and says so.
    ///
    /// Sabotage: defaulting the missing value to 0 makes the comparison
    /// against a real start time fail, which is silently identical to
    /// "not running" -- and the session disappears from the live count
    /// with no explanation anywhere.
    #[test]
    fn a_record_with_no_proc_start_is_reported_rather_than_assumed() {
        let dir = tmp("noprocstart");
        write(&dir, "100.json", r#"{"pid":100,"sessionId":"unguarded"}"#);
        let live = running_ids(&dir, |_| {
            Fake([(100u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert!(live.ids.is_empty());
        assert_eq!(live.unreadable.len(), 1);
        assert!(live.unreadable[0].contains("procStart"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An unparseable record is counted, never skipped.
    ///
    /// Sabotage: a `let Ok(v) = ... else { continue }` with no push is
    /// the natural way to write this loop, and it makes a corrupt record
    /// indistinguishable from a session that is not running.
    #[test]
    fn an_unparseable_record_is_counted_rather_than_skipped() {
        let dir = tmp("corrupt");
        write(&dir, "100.json", "{not json");
        let live = running_ids(&dir, |_| Fake(Default::default()));
        assert!(live.ids.is_empty());
        assert_eq!(live.unreadable.len(), 1, "counted, so the page can say so");
        assert_eq!(live.failure, None, "the DIRECTORY read succeeded");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A missing registry directory is "nothing is running", not a
    /// failure.
    ///
    /// Claude Code creates the directory when a session starts, so its
    /// absence is the settled answer. Reporting it as a failure would
    /// put a "could not tell" banner on every machine with no live
    /// session, which is most machines most of the time -- and a banner
    /// that is always on is a banner nobody reads.
    #[test]
    fn a_missing_registry_is_a_settled_empty_answer() {
        let dir = std::env::temp_dir().join("headstate-live-absent-does-not-exist");
        std::fs::remove_dir_all(&dir).ok();
        let live = running_ids(&dir, |_| Fake(Default::default()));
        assert!(live.ids.is_empty());
        assert_eq!(live.failure, None, "absent is the answer, not an error");
        assert!(live.unreadable.is_empty());
    }

    /// A registry that could not be LISTED is a failure, and yields no
    /// ids.
    ///
    /// The distinction from the test above, and the one the page renders
    /// differently: "we looked and nothing is running" versus "we could
    /// not look". A file where a directory is expected reaches this arm
    /// on every platform.
    #[test]
    fn a_registry_that_could_not_be_listed_is_a_failure() {
        let file = std::env::temp_dir().join(format!("headstate-live-file-{}", std::process::id()));
        std::fs::write(&file, "not a directory").unwrap();
        let live = running_ids(&file, |_| Fake(Default::default()));
        assert!(live.ids.is_empty());
        assert!(
            live.failure.is_some(),
            "an unlistable registry must not read as 'nothing is running'"
        );
        std::fs::remove_file(&file).ok();
    }

    /// Against the real registry on this machine.
    ///
    /// `--ignored` because it needs live `claude` sessions. It prints
    /// rather than asserts the count -- the number changes minute to
    /// minute -- but it DOES assert the two things that must hold: the
    /// directory read succeeded, and every id it returned is one the
    /// registry actually names. A run that silently returned an empty set
    /// because `procStart` failed to parse would show here as zero ids
    /// beside a non-empty `unreadable`, which is the format trap the
    /// module comment describes.
    #[test]
    #[ignore = "needs live claude sessions; run with --ignored"]
    fn real_registry() {
        let dir = registry_dir().expect("a home directory");
        let live = running_ids(&dir, |pids| {
            println!("registry pids        {pids:?}");
            SysinfoProbe::for_pids(pids)
        });
        println!("running session ids  {}", live.ids.len());
        for id in &live.ids {
            println!("  {id}");
        }
        println!("failure              {:?}", live.failure);
        println!("unreadable           {:?}", live.unreadable);
        assert_eq!(
            live.failure, None,
            "the registry directory is readable here"
        );
        // NOT asserted empty any more, and the reason is the measurement
        // in the module header: a machine with a Claudify-launched
        // session running has a `.key` with no `.json`, which is now
        // reported here on purpose. Printed so the format trap is still
        // visible to a reader -- a run with zero ids and a list full of
        // procStart complaints is the failure this test exists to show.
        println!("unreadable: {:#?}", live.unreadable);
    }

    /// The `.key` companion files are not sessions we failed to read.
    #[test]
    fn key_files_are_not_counted_as_unreadable_sessions() {
        let dir = tmp("keys");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"a","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        write(&dir, "100.abc123.key", "opaque");
        let live = running_ids(&dir, |_| {
            Fake([(100u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert_eq!(live.ids.len(), 1);
        assert!(live.unreadable.is_empty(), "a key file is not a session");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole of #1304, in the shape the machine produced it.
    ///
    /// A Claudify launch leaves a `.key` and no `.json` while the process
    /// runs. Before this change the file was skipped and the session was
    /// invisible in every figure; now it is reported as an unreadable
    /// record, which the page renders as "running is at least N".
    ///
    /// Sabotage: restoring the unconditional `continue` for a non-`.json`
    /// file drops `unreadable` to empty and this fails.
    #[test]
    fn a_key_file_with_no_record_is_reported_while_its_process_lives() {
        let dir = tmp("keyonly-live");
        write(
            &dir,
            "4242.deadbeef.key",
            r#"{"peerToken":"x","procStart":"Fri Sep 11 09:43:48 2026","pidDomain":"darwin"}"#,
        );
        let live = running_ids(&dir, |_| {
            Fake([(4242u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert!(
            live.ids.is_empty(),
            "a .key carries no sessionId, so there is no id to report as running"
        );
        assert_eq!(live.unreadable.len(), 1, "{:?}", live.unreadable);
        assert!(
            live.unreadable[0].contains("4242"),
            "the pid is what a reader can act on: {:?}",
            live.unreadable
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A reissued pid must NOT be reported as a live session.
    ///
    /// The honesty constraint #1304 states most sharply: telling a user
    /// "pid N is a leaked Claude session" invites them to kill it, so a
    /// false positive here is worse than a miss. The `.key` path gets the
    /// same `procStart` guard as the `.json` path.
    ///
    /// Sabotage: dropping the start-time comparison from the `.key` arm
    /// reports this unrelated process and the test fails.
    #[test]
    fn a_key_file_whose_pid_was_reissued_is_not_reported() {
        let dir = tmp("keyonly-recycled");
        write(
            &dir,
            "4242.deadbeef.key",
            r#"{"procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| {
            // Days later: a different process wearing the same number.
            Fake([(4242u32, Ok(Some(1789500000)))].into_iter().collect())
        });
        assert!(
            live.unreadable.is_empty(),
            "an unrelated process must not be named as a leaked session: {:?}",
            live.unreadable
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `.key` left behind by a session that ended says nothing.
    ///
    /// The common case once a launched session exits. It is not running,
    /// so it is not hiding from the running count, and reporting it would
    /// turn ordinary cleanup into a standing warning.
    #[test]
    fn a_key_file_whose_process_is_gone_is_not_reported() {
        let dir = tmp("keyonly-gone");
        write(
            &dir,
            "4242.deadbeef.key",
            r#"{"procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| Fake([(4242u32, Ok(None))].into_iter().collect()));
        assert!(live.unreadable.is_empty(), "{:?}", live.unreadable);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `.key` with no readable `procStart` cannot be checked, and says so.
    ///
    /// The `.json` path already refuses to call such a record running,
    /// because pid reuse cannot be ruled out. This is the same refusal:
    /// reported as something we could not check rather than silently
    /// dropped OR asserted as a live session.
    ///
    /// Sabotage: treating a missing `procStart` as a match reports it as
    /// a live session, and the message this asserts changes.
    #[test]
    fn a_key_file_with_no_proc_start_is_reported_as_uncheckable() {
        let dir = tmp("keyonly-noproc");
        write(&dir, "4242.deadbeef.key", r#"{"peerToken":"x"}"#);
        let live = running_ids(&dir, |_| {
            Fake([(4242u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert_eq!(live.unreadable.len(), 1, "{:?}", live.unreadable);
        assert!(
            live.unreadable[0].contains("procStart"),
            "it must say WHY it could not be checked: {:?}",
            live.unreadable
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `.key` beside its own `.json` is still just a companion.
    ///
    /// The regression guard for the change itself: the sessions that work
    /// must not start being reported as problems. Sabotage: removing the
    /// sibling-`.json` check reports pid 100 alongside its own healthy
    /// record, double counting every working session on the machine.
    #[test]
    fn a_key_file_beside_its_record_is_never_reported() {
        let dir = tmp("keyonly-companion");
        write(
            &dir,
            "100.json",
            r#"{"pid":100,"sessionId":"a","procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        write(
            &dir,
            "100.abc.key",
            r#"{"procStart":"Fri Sep 11 09:43:48 2026"}"#,
        );
        let live = running_ids(&dir, |_| {
            Fake([(100u32, Ok(Some(1789119828)))].into_iter().collect())
        });
        assert_eq!(live.ids.len(), 1, "the session is running and named");
        assert!(
            live.unreadable.is_empty(),
            "its record was read fine: {:?}",
            live.unreadable
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
