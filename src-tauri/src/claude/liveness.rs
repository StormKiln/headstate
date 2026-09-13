//! Is this session still running? (#917, epic #910.)
//!
//! Three answers, never two. [`Liveness::Running`],
//! [`Liveness::Dead`] and [`Liveness::Unknown`] -- and the third is the
//! reason this module exists rather than being a one-line `ps` check.
//!
//! # Why derived, and never stored
//!
//! Migration 11 has no `status` column on purpose, and its comment says
//! why: `SessionEnd` does not fire on SIGKILL, a closed terminal, a
//! crash or an OS reap. A stored flag would read "running" forever for
//! exactly the sessions a user wants to resurrect, with nothing to
//! correct it. So liveness is asked of the machine every time a row is
//! rendered.
//!
//! # Why `Unknown` is mandatory, not defensive padding
//!
//! `Unknown` is what a check that could not be COMPLETED returns. It is
//! not a shade of `Dead`, and collapsing the two is #841's `is_some_and`
//! fail-open in another costume: the UI offers Resume on a session it
//! believes is not running, so a failed probe rendered as "not running"
//! offers resurrection for a session that may be alive and mid-work.
//! Resuming a session that is already running starts a SECOND copy of
//! it, so the wrong answer here is not cosmetic.
//!
//! `SystemHealthPage`'s `HealthConditions` is the house pattern on the
//! rendering side -- it renders "nothing found", "could not look" and
//! "have not looked recently" as three different things. This is the
//! same rule one layer down, in the type rather than in the component,
//! so a caller cannot accidentally treat the third as the second.
//!
//! `Unknown` carries its REASON as a string, because the remedies
//! differ: a `0700` registry directory is a permission problem, an
//! unparseable `procStart` is a Claude Code format change, and a session
//! we simply never observed a process for is neither.
//!
//! # Two sources, and why both
//!
//! ```text
//! ~/.claude/sessions/<pid>.json   the LIVE registry -- one file per session
//! claude_run (migration 11)       what the hook recorded, if it ran
//! ```
//!
//! The registry is the better source and it needs no hook installed at
//! all: one small file per session carrying `pid`, `sessionId`,
//! `procStart`, `cwd`, `name` and `status`. Measured on the development
//! machine: three registry files, three live `claude` processes, exact
//! correspondence.
//!
//! It is also, measured, NOT reaped on SIGKILL -- a killed session
//! leaves its file behind with `status` frozen at the moment of death
//! (epic #910, the orphan probe). So an entry whose pid is gone is a
//! POSITIVE crash signal, which is strictly better than inferring a
//! crash from a missing `SessionEnd`, since absence is also what a
//! still-running session looks like.
//!
//! `claude_run` is consulted for sessions the registry does not mention,
//! because #913 will populate it from the hook and a run whose pid the
//! registry has forgotten is still a run we once observed.
//!
//! # `(pid, pid_start_time)`, never a pid alone
//!
//! Pids are recycled. A bare "is 14779 alive" is true about whatever
//! process holds that number now, so a long-dead session whose number
//! was reissued would render as Running -- the same fail-open
//! `health/runaway.rs` already pairs `(pid, start_time)` to defeat, and
//! migration 11's comment states as the reason `pid_start_time` exists.
//!
//! A pid we found but whose start time we cannot compare is
//! [`Liveness::Unknown`], never `Running`: it is indistinguishable from
//! a reused pid.
//!
//! # The format trap that would mark every session dead
//!
//! Measured on this machine, the same instant in both places:
//!
//! ```text
//! registry procStart: Fri Sep 11 09:43:48 2026     (month day, UTC)
//! ps      lstart:     Fri 11 Sep 05:43:48 2026     (day month, LOCAL)
//! ```
//!
//! TWO differences -- field order and timezone, four hours apart in
//! `America/New_York`. A string comparison fails. So does a parse that
//! fixes the field order and ignores the zone. Both fail in the same
//! direction, marking every session dead, and the feature still LOOKS
//! correct because dead-but-resumable is the expected state for 84% of
//! rows. `procstart_is_utc_however_it_is_spelled` and
//! `a_string_comparison_of_the_two_formats_would_call_everything_dead`
//! pin it.
//!
//! This module never parses `ps` output. It compares the registry's
//! `procStart` against `sysinfo`'s `start_time()`, which is epoch
//! seconds and has no format at all -- the trap is in the TEXT, so the
//! fix is to leave the text behind. `procStart` is parsed once, as UTC,
//! with an explicit tolerance (see [`START_TOLERANCE_SECS`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How far apart two readings of one process's start time may be and
/// still be the same process.
///
/// # What the two sources actually disagree by: nothing
///
/// MEASURED, against the three live sessions on the development machine,
/// comparing `procStart` parsed as UTC against `sysinfo`'s
/// `start_time()` (documented as "the time where the process was started
/// (in seconds) from epoch"):
///
/// ```text
/// pid 95843  procStart 1789214253  sysinfo 1789214253  delta 0s
/// pid 14779  procStart 1789119828  sysinfo 1789119828  delta 0s
/// pid 29025  procStart 1789135501  sysinfo 1789135501  delta 0s
/// ```
///
/// Exactly zero on all three. `procStart` is the kernel's process start
/// time truncated to the second, not Claude Code's own clock reading, so
/// the two agree by construction rather than by luck.
///
/// (An earlier version of this comment justified the window with a 2.7s
/// disagreement between the two writers. That figure is real but it is
/// the gap between `procStart` and `startedAt` -- the process starting
/// versus the SESSION starting -- and says nothing about the comparison
/// this constant governs. It is corrected here rather than deleted
/// because a tolerance justified by the wrong measurement is the kind of
/// number nobody later dares to change.)
///
/// # So why a tolerance at all
///
/// Because 0s is what was observed, not what is guaranteed. Truncation
/// alone permits 1s; a clock adjustment between the registry write and
/// our read permits more; and a future Claude Code that writes its own
/// clock reading instead would reintroduce the 2.7s class of gap without
/// telling us.
///
/// 120s is generous against all of those and still far tighter than any
/// plausible pid reuse -- a machine would have to exhaust the pid space
/// inside two minutes for a recycled number to also land in the window.
/// An exact match would be the wrong trade in the other direction: it
/// would report live sessions as dead the first time any of the above
/// moved by one second, and "every session reads as dead" is the exact
/// failure this module is written against.
pub const START_TOLERANCE_SECS: i64 = 120;

/// Whether a session's process is running, and what we could not tell.
///
/// Three variants because there are three answers. See the module docs
/// for why `Unknown` cannot be folded into `Dead`.
///
/// Serialised as `{ "state": "running" }` / `{ "state": "dead", "why": … }`
/// / `{ "state": "unknown", "why": … }` so the TypeScript side gets a
/// discriminated union it has to switch on exhaustively, rather than a
/// boolean it can coerce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Liveness {
    /// The process is alive and its start time matches what was
    /// recorded, so it is the same process and not a reused pid.
    Running {
        pid: u32,
        /// `busy` / `idle` as the session last published it, when the
        /// registry had one.
        ///
        /// A REFINEMENT of an answer already derived, never the answer
        /// itself: it is a STORED status that a killed session never
        /// corrects (epic #910 §6). Only ever carried on `Running`, so
        /// there is no way to render "busy" for a process we did not
        /// find.
        status: Option<String>,
    },
    /// The process is not running. `why` distinguishes the two ways of
    /// establishing that, because one of them is a crash.
    Dead { why: String },
    /// The check could not be completed. NOT a shade of `Dead`.
    Unknown { why: String },
}

impl Liveness {
    /// True only for [`Liveness::Running`].
    ///
    /// Deliberately not paired with an `is_dead` that returns `!running`
    /// -- that helper is the bug this module exists to prevent, and a
    /// caller wanting "offer Resume" must match on the variant so the
    /// `Unknown` arm cannot be forgotten.
    pub fn is_running(&self) -> bool {
        matches!(self, Liveness::Running { .. })
    }
}

/// One entry in `~/.claude/sessions/`, as Claude Code writes it.
///
/// Every field optional but `pid` and `session_id`: this is another
/// program's private file and a release that drops a field must degrade
/// to `Unknown` rather than failing the whole read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub pid: u32,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The process start time as Claude Code spells it: `%a %b %e
    /// %H:%M:%S %Y`, in **UTC**. See the module docs' format trap.
    #[serde(rename = "procStart")]
    pub proc_start: Option<String>,
    pub cwd: Option<String>,
    /// The short live handle (`ghstat-c3`), which is NOT the transcript's
    /// `aiTitle`. Different names for different things; the list leads
    /// with `aiTitle` because it survives death.
    pub name: Option<String>,
    pub status: Option<String>,
    pub version: Option<String>,
}

/// What a read of the registry directory found, INCLUDING what it could
/// not read.
///
/// `failure` is the point of the type. A registry we could not list
/// means every session's liveness is `Unknown`, and that is opposite to
/// "no sessions are running" -- the directory is mode `0700`, so an
/// unreadable one is a real case rather than a theoretical one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    /// Live entries keyed by session id.
    pub entries: HashMap<String, RegistryEntry>,
    /// Why the directory could not be listed, when it could not be.
    ///
    /// `None` with an empty `entries` means "read it, nothing is
    /// running". `Some` means "we do not know what is running".
    pub failure: Option<String>,
    /// Files present but unparseable, with why. Counted rather than
    /// skipped: each one hides a session whose liveness we cannot state.
    pub unreadable: Vec<String>,
}

/// `~/.claude/sessions`, or `None` when there is no home directory.
pub fn registry_dir() -> Option<PathBuf> {
    crate::auth::home_dir().map(|h| h.join(".claude").join("sessions"))
}

/// Read the live session registry.
///
/// An ABSENT directory is not a failure: a machine where Claude Code has
/// never run, or has not run since boot, genuinely has no registry, and
/// reporting that as "could not tell" would put every one of 1,400 rows
/// into `Unknown` on a machine where the honest answer is "nothing is
/// running". Any other error IS a failure, because it hides an unknown
/// number of live sessions.
///
/// Non-JSON siblings are ignored silently, and that is not a swallowed
/// error: the directory also holds `<pid>.<hex>.key` files, which are
/// not session records and never were. Only a `.json` that fails to
/// parse is reported.
pub fn read_registry(dir: &Path) -> Registry {
    let mut out = Registry::default();
    let listing = match std::fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.failure = Some(format!("could not read {}: {e}", dir.display()));
            return out;
        }
    };
    for entry in listing {
        // `flatten()` stood here and SILENTLY DISCARDED a per-entry
        // error, which is the absent-is-not-zero mistake in its smallest
        // form: a directory entry we could not stat might be the record
        // proving a session is alive, and dropping it would leave that
        // session reading as merely missing. Counted instead, which
        // `derive` turns into Unknown for every session it cannot
        // positively find.
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.unreadable
                    .push(format!("{}: could not list an entry: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<RegistryEntry>(&text) {
                Ok(e) if !e.session_id.is_empty() => {
                    out.entries.insert(e.session_id.clone(), e);
                }
                Ok(_) => out
                    .unreadable
                    .push(format!("{}: no sessionId", path.display())),
                Err(e) => out.unreadable.push(format!("{}: {e}", path.display())),
            },
            Err(e) => out.unreadable.push(format!("{}: {e}", path.display())),
        }
    }
    out
}

/// Parse a registry `procStart` into epoch seconds, as UTC.
///
/// `%a %b %e %H:%M:%S %Y` -- `Fri Sep 11 09:43:48 2026`. Month name
/// BEFORE the day, and the value is UTC even though `ps -o lstart=`
/// prints the same instant as `Fri 11 Sep 05:43:48 2026` in local time.
/// See the module docs; getting either half wrong marks every session
/// dead.
///
/// Returns `None` rather than a guess when the text does not parse. The
/// caller turns that into [`Liveness::Unknown`], because a session whose
/// start time we cannot read cannot be distinguished from a reused pid.
pub fn parse_proc_start(text: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text.trim(), "%a %b %e %H:%M:%S %Y")
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

/// What the machine says about one pid, independent of any session.
///
/// A trait so [`derive`] is testable without spawning processes. The
/// real implementation is [`SysinfoProbe`]; the tests use a fixture that
/// can also return the failure case, which is the one an integration
/// test against the live machine cannot produce on demand.
pub trait ProcessProbe {
    /// The process's start time in epoch seconds.
    ///
    /// - `Ok(Some(t))` -- the process exists and started at `t`.
    /// - `Ok(None)` -- the process table was read and this pid is NOT in
    ///   it. A positive absence.
    /// - `Err(why)` -- the process table could not be read. NOT an
    ///   absence, and the whole reason this returns a `Result` rather
    ///   than an `Option`.
    fn start_time(&self, pid: u32) -> Result<Option<i64>, String>;
}

/// The real probe, over `sysinfo`.
///
/// `sysinfo` is already a dependency with the `system` feature, and
/// `health/runaway.rs` already keys processes by `(pid, start_time)` --
/// this reuses that rather than adding a `ps` subprocess whose output
/// would land us straight back in the format trap the module docs
/// describe.
pub struct SysinfoProbe {
    system: sysinfo::System,
}

impl SysinfoProbe {
    /// Refresh only the pids asked for.
    ///
    /// A whole-table refresh costs milliseconds per call and this runs
    /// once per poll for a handful of live sessions; refreshing the
    /// specific pids keeps it proportional to the number of sessions
    /// that could possibly be running rather than to the machine's
    /// process count.
    pub fn for_pids(pids: &[u32]) -> Self {
        let mut system = sysinfo::System::new();
        let wanted: Vec<sysinfo::Pid> = pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&wanted),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Self { system }
    }
}

impl ProcessProbe for SysinfoProbe {
    fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
        // `sysinfo` reports absence and failure identically: a pid
        // missing from the map is all we get, and there is no error
        // channel to consult.
        //
        // This comment previously claimed the refresh "cannot partially
        // fail: either the pid is there or it is gone". That is NOT true
        // on macOS, and a review checking the vendored source found the
        // case: `create_new_process` drops a process from the map
        // entirely when it cannot obtain a NAME for it, even though
        // `proc_bsdinfo` -- and therefore the start time we actually want
        // -- was read fine. So `sysinfo` collapses genuine absence and a
        // narrow class of read failure into one answer, and this
        // implementation inherits that.
        //
        // It stays `Ok(None)` rather than guessing, for two reasons. The
        // case needs a process whose name is unreadable, which a
        // same-user `claude` is not; and inventing `Unknown` for every
        // absent pid would make the common, correct "this session has
        // exited" indistinguishable from a failure, which is the same
        // collapse in the opposite direction.
        //
        // The `Err` arm of the trait is what makes `Unknown` reachable
        // and testable rather than theoretical -- the tests drive it
        // directly -- and it is the arm a probe that CAN distinguish the
        // two would use. `derive`'s registry-level checks are what cover
        // the failures this one cannot see.
        Ok(self
            .system
            .process(sysinfo::Pid::from_u32(pid))
            .map(|p| p.start_time() as i64))
    }
}

/// A run of a session as the hook recorded it (`claude_run`).
///
/// `pid_start_time` is nullable in migration 11 and NULL means "could
/// not confirm", which becomes [`Liveness::Unknown`] -- never `Running`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub pid: u32,
    /// `procStart` text as the consumer resolved it, or `None`.
    pub pid_start_time: Option<String>,
    /// `NULL` = no `SessionEnd` arrived. That is NOT a liveness claim --
    /// it is precisely the SIGKILL case -- so it only decides whether
    /// this run is worth probing at all.
    pub ended_at: Option<String>,
}

/// Derive one session's liveness from the registry and its runs.
///
/// Order of authority, and why:
///
/// 1. **A registry entry** -- the session published its own pid and
///    start time, and the file survives a SIGKILL, so this is the only
///    source that can distinguish a crash from a clean exit.
/// 2. **The newest un-ended run** -- what the hook saw, for a session
///    the registry no longer mentions.
/// 3. **Neither** -- `Unknown`, because we never observed a process.
///    Deliberately NOT `Dead`: the transcript importer creates exactly
///    this state for all ~1,400 historical sessions, and claiming to
///    have watched a process we never saw would be a fabricated
///    reading. What the UI does with it is offer Resume with the
///    caveat, which is the honest action for a session that is almost
///    certainly over but was never observed.
pub fn derive<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    session_id: &str,
    runs: &[Run],
) -> Liveness {
    // A registry we could not LIST poisons every answer: the entry that
    // would have proved a session live may be one of the ones we could
    // not see. Checked before the entry lookup, since a miss in a
    // partially-read map is not evidence of anything.
    if let Some(why) = &registry.failure {
        return Liveness::Unknown {
            why: format!("could not read the live session registry: {why}"),
        };
    }

    if let Some(entry) = registry.entries.get(session_id) {
        let Some(text) = entry.proc_start.as_deref() else {
            return Liveness::Unknown {
                why: format!(
                    "the registry lists pid {} for this session but no start time, so a \
                     recycled pid could not be told from the original",
                    entry.pid
                ),
            };
        };
        let Some(recorded) = parse_proc_start(text) else {
            return Liveness::Unknown {
                why: format!(
                    "could not read the recorded start time {text:?} for pid {}",
                    entry.pid
                ),
            };
        };
        return match probe.start_time(entry.pid) {
            Err(why) => Liveness::Unknown {
                why: format!(
                    "could not check whether pid {} is running: {why}",
                    entry.pid
                ),
            },
            Ok(None) => Liveness::Dead {
                // The orphan case, and it is the interesting one: the
                // registry file OUTLIVES a SIGKILL (measured), so a
                // listed pid that is gone means the session did not exit
                // cleanly. Worth saying, because it is the population
                // this feature exists to resurrect.
                why: format!(
                    "pid {} is in the live session registry but is no longer running, so this \
                     session ended without shutting down",
                    entry.pid
                ),
            },
            Ok(Some(actual)) if (actual - recorded).abs() <= START_TOLERANCE_SECS => {
                Liveness::Running {
                    pid: entry.pid,
                    status: entry.status.clone(),
                }
            }
            Ok(Some(actual)) => Liveness::Dead {
                why: format!(
                    "pid {} is running but started {}s from the recorded time, so the number \
                     has been reused by a different process",
                    entry.pid,
                    (actual - recorded).abs()
                ),
            },
        };
    }

    // No registry ENTRY for this session -- but an entry we could not
    // PARSE is not an absent one.
    //
    // The directory listed fine, so `registry.failure` is None and the
    // check above passed; yet a `<pid>.json` that failed to parse may be
    // the very record proving this session is alive. Claude Code owning
    // that format means a release changing it puts us here, which is
    // exactly the case the module docs anticipate.
    //
    // Reported as Unknown rather than falling through, because the
    // fall-through's `Dead` arm below ("every recorded run reported that
    // it ended") would be a confident claim resting on a record we could
    // not read -- and the UI turns "not running" into a primary Resume
    // button. This is the same fail-open as a failed pid probe, one level
    // out, and it was a live bug until a review caught the mismatch
    // between this function and the banner that already told the user
    // these sessions read as "could not tell".
    if !registry.unreadable.is_empty() {
        return Liveness::Unknown {
            why: format!(
                "{} live-session record(s) in the registry could not be read, so this session \
                 not appearing among the rest is not evidence that it is gone",
                registry.unreadable.len()
            ),
        };
    }

    // Fall back to what the hook recorded, newest un-ended run first: a
    // run with an `ended_at` reported its own `SessionEnd`, so there is
    // nothing to probe.
    let Some(run) = runs.iter().find(|r| r.ended_at.is_none()) else {
        if runs.is_empty() {
            return Liveness::Unknown {
                why: "this session's process was never observed, so whether it is running \
                      cannot be told -- only that it is not in the live registry"
                    .into(),
            };
        }
        return Liveness::Dead {
            why: "every recorded run of this session reported that it ended".into(),
        };
    };

    let Some(recorded) = run.pid_start_time.as_deref().and_then(parse_proc_start) else {
        return Liveness::Unknown {
            why: format!(
                "pid {} was recorded for this session without a start time we can compare, so a \
                 recycled pid could not be told from the original",
                run.pid
            ),
        };
    };
    match probe.start_time(run.pid) {
        Err(why) => Liveness::Unknown {
            why: format!("could not check whether pid {} is running: {why}", run.pid),
        },
        Ok(None) => Liveness::Dead {
            why: format!("pid {} is no longer running", run.pid),
        },
        Ok(Some(actual)) if (actual - recorded).abs() <= START_TOLERANCE_SECS => {
            Liveness::Running {
                pid: run.pid,
                // Nothing published a busy/idle status for a run the
                // registry has forgotten, and inventing "busy" from
                // "the process exists" would be a claim about work
                // rather than about existence.
                status: None,
            }
        }
        Ok(Some(actual)) => Liveness::Dead {
            why: format!(
                "pid {} is running but started {}s from the recorded time, so the number has \
                 been reused by a different process",
                run.pid,
                (actual - recorded).abs()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe whose answers -- including its FAILURE -- are chosen by
    /// the test.
    ///
    /// The failure arm is why this exists. A probe over the real process
    /// table cannot be made to fail on demand, so `Unknown` would be
    /// untested against the live machine, which is the one state whose
    /// absence this whole module is written to prevent.
    struct Fake(Result<Option<i64>, String>);
    impl ProcessProbe for Fake {
        fn start_time(&self, _pid: u32) -> Result<Option<i64>, String> {
            self.0.clone()
        }
    }

    fn alive(t: i64) -> Fake {
        Fake(Ok(Some(t)))
    }
    fn gone() -> Fake {
        Fake(Ok(None))
    }
    fn cannot_look() -> Fake {
        Fake(Err("Operation not permitted".into()))
    }

    /// `Fri Sep 11 09:43:48 2026` UTC, from the real registry file.
    const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
    const PROC_START_EPOCH: i64 = 1_789_119_828;

    fn registry_with(status: Option<&str>) -> Registry {
        let mut r = Registry::default();
        r.entries.insert(
            "s1".into(),
            RegistryEntry {
                pid: 14779,
                session_id: "s1".into(),
                proc_start: Some(PROC_START.into()),
                cwd: Some("/Users/acme/code/widget".into()),
                name: Some("widget-c3".into()),
                status: status.map(str::to_string),
                version: Some("2.1.268".into()),
            },
        );
        r
    }

    /// `procStart` is UTC, and the month name comes before the day.
    ///
    /// Measured on the development machine: this exact text, and
    /// `ps -o lstart=` printing the SAME instant as
    /// `Fri 11 Sep 05:43:48 2026` in `America/New_York`. Both halves of
    /// the trap are pinned here -- a parse that read the local zone
    /// would be 4 hours out, and one that read `11` as a month would
    /// not parse at all.
    #[test]
    fn procstart_is_utc_however_it_is_spelled() {
        assert_eq!(parse_proc_start(PROC_START), Some(PROC_START_EPOCH));
        // The same instant as the local-format spelling, converted --
        // asserted as an equation rather than a second parse, because
        // this module deliberately never parses the `ps` spelling.
        assert_eq!(PROC_START_EPOCH % 60, 48, "seconds survive the parse");
        // Single-digit days are space-padded by `%e`, which is the real
        // format: `Sun Sep  6 01:02:03 2026`.
        assert_eq!(
            parse_proc_start("Sun Sep  6 01:02:03 2026"),
            Some(
                chrono::NaiveDate::from_ymd_opt(2026, 9, 6)
                    .unwrap()
                    .and_hms_opt(1, 2, 3)
                    .unwrap()
                    .and_utc()
                    .timestamp()
            )
        );
    }

    /// The `ps` spelling does NOT parse here, and that is deliberate.
    ///
    /// If a later change reached for `ps -o lstart=` as a second source,
    /// its output would arrive in the other field order and the other
    /// zone. This asserts the two formats are genuinely different text,
    /// which is the fact that makes a string comparison -- the obvious
    /// implementation -- mark every session dead.
    #[test]
    fn a_string_comparison_of_the_two_formats_would_call_everything_dead() {
        let ps_spelling = "Fri 11 Sep 05:43:48 2026";
        assert_ne!(
            ps_spelling, PROC_START,
            "the two formats are not the same text"
        );
        assert_eq!(
            parse_proc_start(ps_spelling),
            None,
            "this parser reads the REGISTRY format only; the ps spelling must not \
             silently parse into some other instant"
        );
    }

    /// The happy path: pid found, start time matches.
    #[test]
    fn a_live_pid_whose_start_time_matches_is_running() {
        let got = derive(
            &alive(PROC_START_EPOCH),
            &registry_with(Some("busy")),
            "s1",
            &[],
        );
        assert_eq!(
            got,
            Liveness::Running {
                pid: 14779,
                status: Some("busy".into())
            }
        );
    }

    /// Within the tolerance is still the same process.
    #[test]
    fn a_start_time_inside_the_tolerance_is_the_same_process() {
        let got = derive(
            &alive(PROC_START_EPOCH + START_TOLERANCE_SECS),
            &registry_with(None),
            "s1",
            &[],
        );
        assert!(got.is_running());
    }

    /// A REUSED pid is dead, not running. Without the start-time pairing
    /// this is the fail-open migration 11's comment names.
    #[test]
    fn a_recycled_pid_is_dead_not_running() {
        let got = derive(
            &alive(PROC_START_EPOCH + 86_400),
            &registry_with(None),
            "s1",
            &[],
        );
        match got {
            Liveness::Dead { why } => assert!(why.contains("reused"), "{why}"),
            other => panic!("a recycled pid must not read as {other:?}"),
        }
    }

    /// An orphaned registry entry is a CRASH, and says so.
    ///
    /// Measured: the file survives SIGKILL with `status` frozen at the
    /// moment of death, so a listed pid that is gone is positive
    /// evidence of an unclean end rather than an inference from silence.
    #[test]
    fn a_registry_entry_whose_pid_is_gone_is_a_crash() {
        let got = derive(&gone(), &registry_with(Some("busy")), "s1", &[]);
        match got {
            Liveness::Dead { why } => assert!(
                why.contains("without shutting down"),
                "an orphan is the crash signal, and the reason should say so: {why}"
            ),
            other => panic!("expected Dead, got {other:?}"),
        }
    }

    /// **The sabotage test.** A probe that could not look is `Unknown`,
    /// and `Unknown` is not `Dead`.
    ///
    /// Collapsing the two -- the `is_some_and` shape of #841 -- fails
    /// here, and it must, because "not running" is what enables Resume
    /// in the UI. Resuming a session that is in fact alive starts a
    /// SECOND copy of it.
    #[test]
    fn a_probe_that_could_not_look_is_unknown_and_never_dead() {
        let got = derive(&cannot_look(), &registry_with(None), "s1", &[]);
        match &got {
            Liveness::Unknown { why } => assert!(why.contains("Operation not permitted"), "{why}"),
            other => panic!("a failed probe must not read as {other:?}"),
        }
        assert!(!got.is_running());
        assert!(
            !matches!(got, Liveness::Dead { .. }),
            "Unknown must not be a shade of Dead"
        );
    }

    /// An unreadable registry poisons every answer, including for a
    /// session that happens to have an entry in the partial read.
    ///
    /// The directory is mode `0700`, so this is a real case. Checked
    /// BEFORE the entry lookup: a miss in a partially-read map is not
    /// evidence of absence.
    #[test]
    fn an_unreadable_registry_makes_every_session_unknown() {
        let mut r = registry_with(Some("busy"));
        r.failure = Some("Permission denied".into());
        let got = derive(&alive(PROC_START_EPOCH), &r, "s1", &[]);
        assert!(
            matches!(got, Liveness::Unknown { .. }),
            "a registry we could not list cannot prove anything: {got:?}"
        );
    }

    /// A registry entry with no `procStart` is `Unknown`, not `Running`.
    ///
    /// The pid alone cannot be told from a reused one, so claiming
    /// Running would be the fail-open with an extra step.
    #[test]
    fn a_registry_entry_with_no_start_time_is_unknown() {
        let mut r = registry_with(None);
        r.entries.get_mut("s1").unwrap().proc_start = None;
        let got = derive(&alive(PROC_START_EPOCH), &r, "s1", &[]);
        assert!(matches!(got, Liveness::Unknown { .. }), "{got:?}");
    }

    /// An unparseable `procStart` is `Unknown` -- a Claude Code format
    /// change must degrade to "could not tell", not to "everything is
    /// dead", which is exactly what the format trap produces.
    #[test]
    fn an_unparseable_start_time_is_unknown() {
        let mut r = registry_with(None);
        r.entries.get_mut("s1").unwrap().proc_start = Some("2026-09-11T09:43:48Z".into());
        let got = derive(&alive(PROC_START_EPOCH), &r, "s1", &[]);
        match got {
            Liveness::Unknown { why } => assert!(why.contains("2026-09-11T09:43:48Z"), "{why}"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// A session nobody ever watched is `Unknown`.
    ///
    /// This is all ~1,400 imported historical sessions, so it is the
    /// COMMON case rather than an edge. `Dead` here would be a claim to
    /// have observed a process we never saw.
    #[test]
    fn a_session_with_no_registry_entry_and_no_runs_is_unknown() {
        let got = derive(&gone(), &Registry::default(), "never-seen", &[]);
        match got {
            Liveness::Unknown { why } => assert!(
                why.contains("never observed"),
                "the reason must say we never WATCHED a process, not that one is absent: {why}"
            ),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// **A registry record we could not PARSE is not an absent one.**
    ///
    /// The directory listed fine -- so the `registry.failure` check does
    /// not fire -- but one `<pid>.json` failed to parse, and it might be
    /// the record proving this session is alive. Claude Code owns that
    /// format, so a release changing it puts every session here.
    ///
    /// This was a LIVE BUG until a review caught it: `derive` consulted
    /// only `failure` and `entries`, so this session fell through to the
    /// run fallback and a session whose recorded runs had all ended
    /// reported `Dead` -- "not running", which is what the UI turns into a
    /// primary Resume button -- on the strength of a record we could not
    /// read. The banner above the list already told the user these
    /// sessions read as "could not tell", so the code and the copy
    /// disagreed.
    #[test]
    fn an_unparseable_registry_record_makes_other_sessions_unknown_not_dead() {
        let registry = Registry {
            // Listing succeeded; one record did not parse.
            failure: None,
            unreadable: vec!["/Users/acme/.claude/sessions/99.json: expected value".into()],
            ..Default::default()
        };
        // A session whose every recorded run ENDED -- the arm that would
        // otherwise confidently report Dead.
        let ended = [Run {
            pid: 4242,
            pid_start_time: Some(PROC_START.into()),
            ended_at: Some("2026-09-11T12:00:00Z".into()),
        }];
        let got = derive(&gone(), &registry, "s1", &ended);
        match &got {
            Liveness::Unknown { why } => assert!(
                why.contains("could not be read"),
                "the reason must name the unreadable record: {why}"
            ),
            other => panic!(
                "a session we cannot rule out must not read as {other:?} -- \
                 'not running' is what offers Resume"
            ),
        }
        assert!(
            !matches!(got, Liveness::Dead { .. }),
            "an unreadable record must not licence a Dead verdict"
        );
        // And with a CLEAN registry the same session is legitimately Dead,
        // so the test above is about the unreadable record rather than
        // about this run shape.
        assert!(matches!(
            derive(&gone(), &Registry::default(), "s1", &ended),
            Liveness::Dead { .. }
        ));
    }

    /// A per-entry listing error is counted, not dropped.
    ///
    /// `flatten()` stood in `read_registry` and silently discarded these.
    /// Each one could be the record proving a session alive, so it has to
    /// reach `derive`, which turns a non-empty `unreadable` into Unknown.
    #[test]
    fn the_unreadable_list_is_what_derive_consults() {
        // Asserted as the CONTRACT between the two functions rather than
        // by provoking a real `read_dir` entry error, which needs a race
        // that cannot be staged portably.
        let mut r = Registry::default();
        assert!(matches!(
            derive(&gone(), &r, "s1", &[]),
            Liveness::Unknown { .. }
        ));
        r.unreadable
            .push("whatever: could not list an entry".into());
        let got = derive(&gone(), &r, "s1", &[]);
        match got {
            Liveness::Unknown { why } => assert!(why.contains("could not be read"), "{why}"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// A hook-recorded run is the fallback source when the registry has
    /// forgotten the session.
    #[test]
    fn an_un_ended_run_is_probed_when_the_registry_has_no_entry() {
        let runs = [Run {
            pid: 4242,
            pid_start_time: Some(PROC_START.into()),
            ended_at: None,
        }];
        assert!(derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs).is_running());
        match derive(&gone(), &Registry::default(), "s1", &runs) {
            Liveness::Dead { why } => assert!(why.contains("4242"), "{why}"),
            other => panic!("expected Dead, got {other:?}"),
        }
    }

    /// A run with a NULL `pid_start_time` is `Unknown`.
    ///
    /// Migration 11 makes the column nullable precisely for the pid the
    /// consumer could not confirm, and its comment says NULL means
    /// "cannot confirm". Reporting Running would reinstate the fail-open
    /// the column exists to close.
    #[test]
    fn a_run_without_a_start_time_is_unknown_not_running() {
        let runs = [Run {
            pid: 4242,
            pid_start_time: None,
            ended_at: None,
        }];
        let got = derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs);
        assert!(
            matches!(got, Liveness::Unknown { .. }),
            "NULL means cannot confirm, so it cannot mean running: {got:?}"
        );
    }

    /// Every run reported an end: dead, and nothing to probe.
    #[test]
    fn a_session_whose_runs_all_ended_is_dead() {
        let runs = [Run {
            pid: 4242,
            pid_start_time: Some(PROC_START.into()),
            ended_at: Some("2026-09-11T12:00:00Z".into()),
        }];
        match derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs) {
            Liveness::Dead { why } => assert!(why.contains("ended"), "{why}"),
            other => panic!("expected Dead, got {other:?}"),
        }
    }

    /// An ABSENT registry directory is not a failure.
    ///
    /// A machine where Claude Code has not run since boot has no
    /// registry. Reporting that as "could not tell" would push every
    /// session into `Unknown` on a machine whose honest answer is
    /// "nothing is running" -- the same over-correction in the opposite
    /// direction.
    #[test]
    fn an_absent_registry_directory_is_not_a_failure() {
        let dir = std::env::temp_dir().join("headstate-no-such-registry-917");
        let _ = std::fs::remove_dir_all(&dir);
        let got = read_registry(&dir);
        assert_eq!(got.failure, None);
        assert!(got.entries.is_empty());
    }

    /// The `.key` siblings are not session records and are not reported
    /// as unreadable ones.
    #[test]
    fn key_files_beside_the_records_are_not_parse_failures() {
        let dir = std::env::temp_dir().join("headstate-registry-keys-917");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("14779.abc123.key"), "not json at all").unwrap();
        std::fs::write(
            dir.join("14779.json"),
            format!(
                r#"{{"pid":14779,"sessionId":"s1","procStart":"{PROC_START}","status":"busy"}}"#
            ),
        )
        .unwrap();
        let got = read_registry(&dir);
        assert_eq!(got.failure, None);
        assert_eq!(
            got.unreadable,
            Vec::<String>::new(),
            "a .key is not a record"
        );
        assert_eq!(got.entries.len(), 1);
        assert_eq!(got.entries["s1"].pid, 14779);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed record is COUNTED, not skipped.
    ///
    /// Each one hides a session whose liveness cannot be stated, so it
    /// has to reach the caller.
    #[test]
    fn a_malformed_record_is_reported_rather_than_skipped() {
        let dir = std::env::temp_dir().join("headstate-registry-bad-917");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.json"), "{ not json").unwrap();
        let got = read_registry(&dir);
        assert_eq!(got.failure, None, "one bad file is not a failed listing");
        assert_eq!(got.unreadable.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `sysinfo`'s `start_time()` is EPOCH SECONDS, and this pins it.
    ///
    /// The whole comparison in [`derive`] rests on the two sides sharing
    /// a unit: `procStart` parsed as UTC epoch seconds against
    /// `start_time()`. If a future `sysinfo` returned milliseconds, or
    /// seconds since boot, the subtraction would still compile and every
    /// session would read as a reused pid -- silently, and in the same
    /// direction as the format trap in the module docs.
    ///
    /// Asserted against THIS process, which is the one process guaranteed
    /// to exist while the test runs, and bounded rather than exact: it
    /// must sit between a fixed past date and a little way into the
    /// future. Milliseconds-since-epoch would be ~1000x too large and
    /// seconds-since-boot ~1000x too small, so either fails the range
    /// even though neither could fail an "is it non-zero" check.
    #[test]
    fn sysinfo_reports_a_start_time_in_epoch_seconds() {
        let me = std::process::id();
        let probe = SysinfoProbe::for_pids(&[me]);
        let got = probe
            .start_time(me)
            .expect("the process table is readable")
            .expect("this very process is in it");
        // 2020-01-01 .. 2100-01-01, in epoch SECONDS.
        assert!(
            (1_577_836_800..4_102_444_800).contains(&got),
            "start_time() must be epoch seconds; got {got}, which is the wrong \
             magnitude and would make every liveness check report a reused pid"
        );
        // And it is in the past: a start time in the future would mean the
        // units line up but the epoch does not.
        let now = chrono::Utc::now().timestamp();
        assert!(
            got <= now + 60,
            "a process cannot have started {got} > now {now}"
        );
    }

    /// The real registry on this machine, when there is one.
    ///
    /// Prints rather than asserts the counts -- the number of live
    /// sessions is whatever the developer happens to be running. What it
    /// DOES assert is the invariant that matters: every entry either
    /// parses into a state with a reason, or is reported as unreadable.
    /// Nothing is silently dropped.
    ///
    /// It also prints the DELTA between each registry `procStart` and
    /// `sysinfo`'s reading, which is the measurement
    /// [`START_TOLERANCE_SECS`] is justified by -- 0s on all three live
    /// sessions when that constant's comment was written.
    #[test]
    fn real_registry() {
        let Some(dir) = registry_dir() else {
            eprintln!("no home directory; skipping");
            return;
        };
        let reg = read_registry(&dir);
        let pids: Vec<u32> = reg.entries.values().map(|e| e.pid).collect();
        let probe = SysinfoProbe::for_pids(&pids);
        eprintln!(
            "registry {}: {} entries, {} unreadable, failure={:?}",
            dir.display(),
            reg.entries.len(),
            reg.unreadable.len(),
            reg.failure
        );
        for (id, e) in &reg.entries {
            let state = derive(&probe, &reg, id, &[]);
            // The delta the tolerance is justified by. Printed rather
            // than asserted: an entry whose pid has legitimately exited
            // between the read and here has no delta to report, and that
            // is a correct `Dead` rather than a failure.
            let delta = match (
                e.proc_start.as_deref().and_then(parse_proc_start),
                probe.start_time(e.pid),
            ) {
                (Some(r), Ok(Some(a))) => format!("{}s", (a - r).abs()),
                _ => "n/a".into(),
            };
            eprintln!(
                "  pid {:>7} {:<40} procStart={:?} delta={delta} -> {state:?}",
                e.pid,
                e.name.as_deref().unwrap_or("-"),
                e.proc_start
            );
            // Whatever the answer, it must not be a silent absence: every
            // variant either is Running or carries a reason.
            match state {
                Liveness::Running { .. } => {}
                Liveness::Dead { why } | Liveness::Unknown { why } => {
                    assert!(!why.is_empty(), "every non-running state states a reason")
                }
            }
        }
    }
}
