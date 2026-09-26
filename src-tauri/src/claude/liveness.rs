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
//! # The failure events are NOT a source here, and must not become one
//!
//! Epic #1060 added three events (#1062, #1063, #1064) recording why a
//! turn died, which tools failed and what auto mode denied. None of them
//! belongs in this derivation, and the temptation is real: a turn that
//! died of a rate limit reads like evidence the session is over.
//!
//! It is not. `StopFailure` does not fire on SIGKILL -- the founding
//! measurement of this whole epic -- and a rate-limited session is very
//! much still running, so consulting it would report a live session as
//! dead. A denial is further still from an ending: it is a guardrail
//! doing its job while the session carries on.
//!
//! The separation is structural rather than remembered. Those events live
//! in `claude_hook_event`, this derivation is handed only `&[Run]` rows,
//! and
//! `invariants.rs`'s `liveness_never_reads_the_failure_events` fails on
//! any code in this file that reaches for them.
//!
//! `claude_run` is consulted for sessions the registry does not mention,
//! because #913 will populate it from the hook and a run whose pid the
//! registry has forgotten is still a run we once observed.
//!
//! # A complete registry listing that does not mention a session (#984)
//!
//! MEASURED on the real corpus, after #947 put `claude_poll_live` on the
//! 60-second timer, by `sessions::tests::real_session_list`:
//!
//! ```text
//! rows returned  1491
//! liveness  running 1  dead 0  unknown 1490
//! ```
//!
//! `Dead` matched **no row at all**, and structurally could not: every
//! session here was imported from a transcript, migration 11's
//! `claude_run.pid NOT NULL` forbids an import from fabricating a run,
//! and a session that ran before the hook was installed can never
//! acquire a pid retroactively. So the `runs.is_empty()` arm below WAS
//! the whole list, and it returned `Unknown` -- "could not tell" on
//! 1,490 of 1,491 rows, while `overview::aggregate` called 183 of the
//! same rows resumable on the strength of the same registry read. Two
//! pages, one database, contradictory answers.
//!
//! The overview's reading is the one consistent with this module's own
//! evidence, and the fix is to adopt it here rather than add a third:
//! **once `failure` is `None` and `unreadable` is empty, a registry
//! listing that does not mention a session is positive evidence that it
//! is not running.** `~/.claude/sessions/<pid>.json` exists for every
//! running session and survives a SIGKILL (measured, epic #910), and
//! `read_registry` already treats an absent directory as a settled empty
//! answer rather than a failure -- `live.rs`'s
//! `a_missing_registry_is_a_settled_empty_answer` is the same rule stated
//! as a test, with the comment "absent is the answer, not an error".
//!
//! This does NOT weaken `Unknown`, and the ordering is what guarantees
//! that: `registry.failure` and a non-empty `registry.unreadable` are
//! still checked FIRST and still return `Unknown`, so the inference only
//! ever applies to a listing we read completely. A registry we could not
//! read still poisons every row, which is the direction that matters --
//! a wrong `Dead` offers a confident Resume that starts a second copy.
//!
//! The `why` strings stay distinct, because the evidence is not equally
//! strong. A `Dead` from probing a recorded pid is a fact about a
//! process; a `Dead` from registry absence is an inference from one
//! file's absence, and [`Liveness::Dead`] is rendered with its `why` for
//! exactly that reason.
//!
//! # A running session that publishes only a `.key` (#1315)
//!
//! The #984 inference above rests on "every running session publishes a
//! `<pid>.json`", and that premise is FALSE for a session started from a
//! terminal -- every Claudify or resume launch. Measured (#1304, and
//! again for #1315 with a bare interactive `claude`):
//!
//! ```text
//! ~/.claude/sessions/<pid>.json          ABSENT
//! ~/.claude/sessions/<pid>.<hash>.key    PRESENT, the process alive
//! ```
//!
//! The `.key` is `{"peerToken","procStart","pidDomain"}` and nothing
//! else. It carries **no `sessionId`**, and there is no other file that
//! names one: a bare interactive session has not even written a
//! transcript until its first prompt, so pid -> transcript is not a
//! matching problem we could solve with more cleverness, it is a record
//! that does not exist yet. So such a session CANNOT be named, and this
//! module never invents a name for it -- no `Running` verdict is ever
//! issued from a `.key`.
//!
//! What it does instead is refuse to let the missing record masquerade
//! as a settled `Dead`. A `.key` with no sibling `.json` becomes a
//! [`UnnamedRecord`]; [`unnamed_sessions`] checks it with the same
//! `(pid, procStart)` pairing and [`START_TOLERANCE_SECS`] as every
//! other arm (a false positive would invite someone to kill an unrelated
//! process); and any row whose verdict would be `Dead` while a live
//! unnamed session could be it is returned as `Unknown`, naming the pid.
//! A `.key` whose process is gone, or whose pid now belongs to a
//! different process, says nothing: that is ordinary cleanup.
//!
//! **Narrowed by working directory, and why that is sound.** Without a
//! narrowing, one Claudify launch would put all ~1,400 historical rows
//! back into `Unknown` -- #984's regression, reintroduced by the one
//! event this feature exists to support. A session's process runs in
//! its project directory and Claude Code keeps its shell inside that
//! tree, so a live unnamed process can only be a row whose recorded cwd
//! is the same directory, an ancestor, or a descendant of the process's
//! own. Every uncertainty falls toward `Unknown` rather than `Dead`: a
//! process cwd we could not read, or a row with no recorded cwd,
//! matches everything.
//!
//! This is a fix to `liveness.rs`, not the `live.rs` migration: the
//! overview still counts from `live.rs` (which already reports these as
//! "running is at least N", #1312), and the two now agree that such a
//! session exists without either pretending to name it.
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
    /// `<pid>.<hash>.key` files with no `<pid>.json` beside them (#1315).
    ///
    /// NOT yet a claim that anything is running -- a session that ended
    /// leaves its `.key` behind too. [`unnamed_sessions`] decides that,
    /// against the process table.
    pub unnamed: Vec<UnnamedRecord>,
}

impl Registry {
    /// Every pid a verdict could turn on: the entries' and the unnamed
    /// records'. Callers build the probe from this, so a `.key`'s pid is
    /// in the refreshed process table rather than reading as absent.
    pub fn probe_pids(&self) -> Vec<u32> {
        self.entries
            .values()
            .map(|e| e.pid)
            .chain(self.unnamed.iter().map(|u| u.pid))
            .collect()
    }
}

/// A registry `.key` with no sibling `.json`: a session Claude Code
/// started and published no record for (#1304, #1315).
///
/// Deliberately has no session id -- the file does not contain one, and
/// that absence is what keeps it from ever becoming a `Running` row, an
/// orphan or a resumable entry. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnnamedRecord {
    /// From the filename, `<pid>.<hash>.key`.
    pub pid: u32,
    /// The file's `procStart`, as written. `None` when the file could not
    /// be read or carried none -- which makes the pid uncheckable, not
    /// absent.
    pub proc_start: Option<String>,
    /// For the message, so a reader can go and look at the file.
    pub path: String,
}

/// A `.key`-only record, checked against the process table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnnamedSession {
    pub pid: u32,
    /// The process's working directory. `None` when it could not be read
    /// -- and then it may be ANY session, so it matches every row.
    pub cwd: Option<PathBuf>,
    /// One line for the list and the export: which pid, where, and how
    /// sure. Never a session id -- there is none.
    pub line: String,
}

impl UnnamedSession {
    /// Could this unnamed process be the session whose recorded cwd is
    /// `row_cwd`?
    ///
    /// Same directory, ancestor or descendant -- see the module docs. An
    /// unreadable side matches, because the wrong answer in that
    /// direction is a `Dead` on a live session, which offers a Resume
    /// that starts a second copy.
    pub fn could_be(&self, row_cwd: Option<&str>) -> bool {
        let (Some(proc_cwd), Some(row)) = (self.cwd.as_deref(), row_cwd) else {
            return true;
        };
        let related = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
        let row = Path::new(row);
        if related(proc_cwd, row) {
            return true;
        }
        // A symlinked spelling of the same directory (`/tmp` against the
        // kernel's `/private/tmp` on macOS) must not read as unrelated.
        // Canonicalised only on a raw miss, and only while an unnamed
        // session exists, so the common poll pays nothing.
        match (proc_cwd.canonicalize(), row.canonicalize()) {
            (Ok(p), Ok(r)) => related(&p, &r),
            (Ok(p), Err(_)) => related(&p, row),
            (Err(_), Ok(r)) => related(proc_cwd, &r),
            (Err(_), Err(_)) => false,
        }
    }
}

/// Classify one non-`.json` registry file: `Some` only for a
/// `<pid>.<hash>.key` whose `<pid>.json` does not exist.
///
/// A `.key` beside its own `.json` is a companion and its session is
/// already an entry, so it is skipped rather than double counted. A
/// leading component that is not a number is not one of these files at
/// all.
fn key_only(path: &Path, dir: &Path) -> Option<UnnamedRecord> {
    if path.extension().and_then(|e| e.to_str()) != Some("key") {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let pid: u32 = name.split('.').next()?.parse().ok()?;
    if dir.join(format!("{pid}.json")).exists() {
        return None;
    }
    let proc_start = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("procStart")?.as_str().map(str::to_string));
    Some(UnnamedRecord {
        pid,
        proc_start,
        path: path.display().to_string(),
    })
}

/// Which `.key`-only records could be a running session (#1315).
///
/// | `.key` | process table | result |
/// |---|---|---|
/// | any | pid not there | nothing -- it ended and left the file |
/// | `procStart` parses | there, start times differ | nothing -- the pid was reused |
/// | `procStart` parses | there, start times agree | **running, unnamed** |
/// | no usable `procStart` | there | **may be running** -- a reused pid cannot be ruled out, nor can the session |
/// | any | probe failed | **may be running** |
///
/// The two "may be" rows are included because what they protect is a
/// `Dead` verdict, and a `Dead` on a live session is the expensive error.
pub fn unnamed_sessions<P: ProcessProbe>(probe: &P, registry: &Registry) -> Vec<UnnamedSession> {
    let mut out = Vec::new();
    for rec in &registry.unnamed {
        let actual = match probe.start_time(rec.pid) {
            Ok(None) => continue,
            Ok(Some(t)) => t,
            Err(why) => {
                out.push(UnnamedSession {
                    pid: rec.pid,
                    cwd: None,
                    line: format!("pid {} could not be checked: {why}", rec.pid),
                });
                continue;
            }
        };
        let confirmed = match rec.proc_start.as_deref().and_then(parse_proc_start) {
            Some(recorded) if (actual - recorded).abs() <= START_TOLERANCE_SECS => true,
            Some(_) => continue,
            None => false,
        };
        let cwd = probe.cwd(rec.pid).ok().flatten();
        let place = match &cwd {
            Some(c) => format!("in {}", c.display()),
            None => "in a folder that could not be read".into(),
        };
        let line = if confirmed {
            format!("pid {}, running {place}", rec.pid)
        } else {
            format!(
                "pid {}, {place}, may be a different program reusing the number: its start \
                 time could not be read from {}",
                rec.pid, rec.path
            )
        };
        out.push(UnnamedSession {
            pid: rec.pid,
            cwd,
            line,
        });
    }
    out
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
/// A `<pid>.<hex>.key` beside its own `.json` is a companion file and is
/// skipped. A `.key` with NO `.json` is kept in [`Registry::unnamed`]
/// (#1315): it is the only trace a terminal-launched session leaves.
/// Only a `.json` that fails to parse is reported as unreadable.
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
            // Skipping every non-`.json` here was #1315: a terminal
            // launch publishes ONLY a `.key`, so the session was
            // invisible and the #984 inference below then called its
            // row confidently dead.
            if let Some(rec) = key_only(&path, dir) {
                out.unnamed.push(rec);
            }
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

    /// The process's working directory (#1315), with the same three
    /// answers as [`ProcessProbe::start_time`].
    ///
    /// Only asked about a `.key`-only process, to narrow which rows it
    /// could be. The default says it could not look, which makes that
    /// process match EVERY row -- the safe direction for a probe that
    /// does not read directories.
    fn cwd(&self, _pid: u32) -> Result<Option<PathBuf>, String> {
        Err("this probe does not read working directories".into())
    }
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
            // The cwd for #1315's narrowing. One `proc_pidinfo` per
            // pid on macOS, over the handful of pids asked about.
            sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::OnlyIfNotSet),
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

    fn cwd(&self, pid: u32) -> Result<Option<PathBuf>, String> {
        match self.system.process(sysinfo::Pid::from_u32(pid)) {
            None => Ok(None),
            Some(p) => p
                .cwd()
                .map(|c| Some(c.to_path_buf()))
                .ok_or_else(|| format!("the working directory of pid {pid} could not be read")),
        }
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
    /// How the run ended, as whoever recorded the end wrote it (#965).
    ///
    /// Two disjoint vocabularies, deliberately: the hook writes
    /// `clear|resume|logout|prompt_input_exit|other`, every one of which
    /// means a clean exit RAN, and `crash::CRASHED` (`"crashed"`) is
    /// written by the registry sweep only, for a run we found already
    /// dead. `crash.rs:23-31` is explicit that the column exists to
    /// distinguish "ended, and told us why" from "ended, and we found out
    /// by looking".
    ///
    /// It is NOT a liveness column and this module must not shortcut the
    /// probe with it -- migration 11's deliberate lack of a `status`
    /// column is the epic's central correction, and a stored flag reads
    /// "running" forever for exactly the sessions a user wants back.
    /// What it is used for is WORDING the `Dead` arm: a crashed run did
    /// not "report that it ended", and saying it did is factually wrong
    /// about the one fact this feature exists to surface.
    ///
    /// A crashed run's `ended_at` is also not an end time we witnessed --
    /// `crash.rs:32-46` records that the file's mtime was rejected as too
    /// unreliable to store, so `ended_at` there is the moment we OBSERVED
    /// the crash. Nothing here reinterprets it as anything else.
    pub end_reason: Option<String>,
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
/// 3. **Neither, and the registry listing was COMPLETE** -- `Dead`, on
///    the strength of the absence itself (#984). Every running session
///    publishes a registry file and the file survives a SIGKILL, so a
///    listing we read whole that does not name this session is positive
///    evidence rather than a shrug. This is the ~1,400 transcript-imported
///    historical sessions, i.e. the entire list on any machine that
///    adopted Headstate after using Claude Code, and returning `Unknown`
///    for it made the tri-state carry no information at all while the
///    overview page called the same rows resumable. The `why` says the
///    verdict rests on the absence, because that is weaker evidence than
///    a probed pid and the detail pane renders it.
///
/// The case that is still `Unknown` is a listing we could NOT read
/// completely, and it is checked before any of the three above -- see the
/// module docs. The direction of the remaining error is the safe one: a
/// registry we could not read still puts every row in `Unknown`.
///
/// With no cwd for the row, so any live `.key`-only session turns a
/// `Dead` into `Unknown` (#1315). Callers that know the row's cwd use
/// [`derive_at`], which narrows that to the sessions that could be it.
pub fn derive<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    session_id: &str,
    runs: &[Run],
) -> Liveness {
    derive_at(probe, registry, session_id, None, runs)
}

/// [`derive`], for a row whose recorded working directory is `cwd`.
///
/// Every `Dead` below rests, somewhere, on "the registry would have
/// shown it": the #984 absence inference, a run's pid being gone, even
/// an orphaned entry (a crashed session resumed from a terminal leaves
/// its old `.json` and publishes only a `.key` for the new process). A
/// live `.key`-only session breaks that premise for every row it could
/// be, so those rows become `Unknown`, naming the pid. `Running` and
/// `Unknown` verdicts are never touched -- the unnamed session cannot
/// make a row MORE certain, and it is never promoted to `Running`,
/// because nothing says which session it is.
pub fn derive_at<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    session_id: &str,
    cwd: Option<&str>,
    runs: &[Run],
) -> Liveness {
    let verdict = from_records(probe, registry, session_id, runs);
    if !matches!(verdict, Liveness::Dead { .. }) {
        return verdict;
    }
    let could_be: Vec<String> = unnamed_sessions(probe, registry)
        .into_iter()
        .filter(|u| u.could_be(cwd))
        .map(|u| u.line)
        .collect();
    if could_be.is_empty() {
        return verdict;
    }
    Liveness::Unknown {
        why: format!(
            "{} Claude Code session{} that did not record which session {} could be this one \
             ({}), so it cannot be called stopped",
            could_be.len(),
            if could_be.len() == 1 { "" } else { "s" },
            if could_be.len() == 1 {
                "it is"
            } else {
                "they are"
            },
            could_be.join("; ")
        ),
    }
}

/// The verdict from the named records alone: registry entries and runs.
fn from_records<P: ProcessProbe>(
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
            // #984. The registry listed completely -- both checks above
            // passed -- and it does not name this session. A running
            // session always has a `~/.claude/sessions/<pid>.json`, and
            // that file outlives a SIGKILL, so the absence is the answer.
            //
            // The sentence says what it rests on, and says that nothing
            // watched the process, because the two are different facts and
            // the second is what "Observed runs: none" reports beside it.
            // A reader who sees only "not running" would not know this was
            // inferred from one directory listing rather than from probing
            // a pid we recorded.
            return Liveness::Dead {
                why: "no process was ever recorded for this session, and it is not in the \
                      live session registry -- which lists every running session -- so it \
                      is not running"
                    .into(),
            };
        }
        // Every recorded run ended. HOW it ended is what `end_reason`
        // carries (#965), and the two answers are not interchangeable: a
        // crash is the thing this feature exists to detect, and it was
        // being reported as a self-reported clean exit.
        //
        // `crashed` is checked against the newest run rather than any run:
        // a session that crashed in March and exited cleanly in July is
        // over, cleanly, and the July record is the one that describes its
        // ending. `runs` arrives newest-first from
        // `sessions::runs_by_session` (`ORDER BY started_at DESC`).
        let crashed = runs
            .first()
            .and_then(|r| r.end_reason.as_deref())
            .is_some_and(|reason| reason == super::crash::CRASHED);
        return Liveness::Dead {
            why: if crashed {
                // Deliberately NOT a claim about when. `crash.rs:32-46`
                // stores the moment we observed the orphan, not a
                // witnessed end time, so the sentence says how we found
                // out rather than putting a time on it.
                "this session's process was found already gone, with nothing cleaned up \
                 after it, so it ended without shutting down"
                    .into()
            } else {
                "every recorded run of this session reported that it ended".into()
            },
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

    /// A session nobody ever watched, missing from a registry we read
    /// WHOLE, is `Dead` (#984).
    ///
    /// This is all ~1,400 imported historical sessions, so it is the
    /// COMMON case rather than an edge -- which is precisely why it could
    /// not stay `Unknown`: 1,490 of 1,491 rows read "could not tell" while
    /// the overview page called 183 of them resumable from the same
    /// registry read.
    ///
    /// `Registry::default()` is a COMPLETE read here and not a stub:
    /// `read_registry` returns exactly this for an absent
    /// `~/.claude/sessions`, and `an_absent_registry_directory_is_not_a_failure`
    /// pins that. So the listing succeeded and does not name this session,
    /// and every running session publishes a file in it.
    ///
    /// The `why` must still say what the verdict RESTS on, because the
    /// detail pane renders it and an inference from one directory listing
    /// is weaker evidence than a probed pid.
    #[test]
    fn a_session_with_no_registry_entry_and_no_runs_is_dead_from_the_absence() {
        let got = derive(&gone(), &Registry::default(), "never-seen", &[]);
        match got {
            Liveness::Dead { why } => {
                assert!(
                    why.contains("live session registry"),
                    "the reason must name the registry the verdict rests on: {why}"
                );
                assert!(
                    why.contains("no process was ever recorded"),
                    "and must not imply we watched a process and saw it go: {why}"
                );
            }
            other => panic!(
                "a complete registry listing that omits a session is positive evidence, \
                 not a shrug: got {other:?}"
            ),
        }
    }

    /// The registry-absence `Dead` and the probed-pid `Dead` do not share
    /// a sentence (#984).
    ///
    /// Both are `Dead`, and they rest on different evidence: one on a
    /// directory listing that does not name the session, the other on
    /// asking the process table about a pid we recorded. `SessionDetail`
    /// renders `liveness.why` so the user has the grounds as well as the
    /// verdict, and one shared string would leave the two
    /// indistinguishable there.
    #[test]
    fn the_two_dead_paths_say_different_things() {
        let from_absence = derive(&gone(), &Registry::default(), "never-seen", &[]);
        let runs = [Run {
            pid: 4242,
            pid_start_time: Some(PROC_START.into()),
            ended_at: None,
            ..Default::default()
        }];
        let from_probe = derive(&gone(), &Registry::default(), "s1", &runs);
        match (&from_absence, &from_probe) {
            (Liveness::Dead { why: a }, Liveness::Dead { why: b }) => {
                assert_ne!(a, b, "two kinds of evidence, one sentence");
                assert!(
                    !a.contains("4242") && b.contains("4242"),
                    "only the probed one may name a pid: {a:?} / {b:?}"
                );
            }
            other => panic!("both paths must be Dead: {other:?}"),
        }
    }

    /// A registry we could NOT read still makes an unwatched session
    /// `Unknown` (#984).
    ///
    /// The pair to `a_session_with_no_registry_entry_and_no_runs_is_dead_from_the_absence`,
    /// and the half that must not regress: the new inference rests
    /// entirely on the listing having been complete, so a failure has to
    /// keep poisoning exactly the rows the inference now claims. This is
    /// the direction where being wrong costs something -- a `Dead` on a
    /// live session offers a confident Resume that starts a second copy.
    #[test]
    fn an_unwatched_session_is_unknown_when_the_registry_could_not_be_read() {
        let listing_failed = Registry {
            failure: Some("could not read /Users/acme/.claude/sessions: Permission denied".into()),
            ..Default::default()
        };
        let got = derive(&gone(), &listing_failed, "never-seen", &[]);
        assert!(
            matches!(got, Liveness::Unknown { .. }),
            "an absence in a listing we could not complete proves nothing: {got:?}"
        );

        let record_unreadable = Registry {
            failure: None,
            unreadable: vec!["/Users/acme/.claude/sessions/99.json: expected value".into()],
            ..Default::default()
        };
        let got = derive(&gone(), &record_unreadable, "never-seen", &[]);
        assert!(
            matches!(got, Liveness::Unknown { .. }),
            "the unreadable record might be the one naming this session: {got:?}"
        );
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
            ..Default::default()
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
        // The clean listing is the CONTROL, and since #984 it is `Dead`
        // rather than `Unknown`: this test is about what one unlistable
        // entry changes, so the before state has to be the settled answer
        // it changes away from.
        assert!(
            matches!(derive(&gone(), &r, "s1", &[]), Liveness::Dead { .. }),
            "a complete listing that omits the session is a settled answer"
        );
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        }];
        match derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs) {
            Liveness::Dead { why } => assert!(why.contains("ended"), "{why}"),
            other => panic!("expected Dead, got {other:?}"),
        }
    }

    /// A run recorded as CRASHED does not claim to have reported its own
    /// end (#965).
    ///
    /// `crash.rs` writes `end_reason = "crashed"` for a registry orphan --
    /// a pid we found already gone -- and nothing else writes that value.
    /// The old fall-through arm said "every recorded run of this session
    /// reported that it ended", which for such a run is the one thing that
    /// is definitely false: the run reported nothing, we found out by
    /// looking. Until this test the column had no production reader at
    /// all, so ten tests in `crash.rs` asserted on a classification the
    /// surface could never show.
    ///
    /// This does NOT shortcut the probe: the run is still only consulted
    /// after the registry, and `end_reason` decides the WORDING of an
    /// already-settled `Dead`. A fourth `Liveness` state would break the
    /// tri-state the module argues for.
    #[test]
    fn a_crashed_run_says_it_crashed_rather_than_that_it_reported_ending() {
        let runs = [Run {
            pid: 4242,
            pid_start_time: Some(PROC_START.into()),
            // `crash.rs` sets this to the moment WE observed the orphan,
            // not a witnessed end time, so nothing here may put a time on
            // the ending.
            ended_at: Some("2026-09-11T12:00:00Z".into()),
            end_reason: Some(super::super::crash::CRASHED.into()),
        }];
        match derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs) {
            Liveness::Dead { why } => {
                assert!(
                    why.contains("without shutting down"),
                    "a crash must read as a crash: {why}"
                );
                assert!(
                    !why.contains("reported that it ended"),
                    "the run reported nothing -- we found out by looking: {why}"
                );
                assert!(
                    !why.contains("2026-09-11"),
                    "`ended_at` on a crashed row is our observation time, not an end time: {why}"
                );
            }
            other => panic!("a crashed run is Dead, not {other:?} -- the three states stay three"),
        }
    }

    /// A CLEAN end is not relabelled as a crash (#965).
    ///
    /// The pair to the test above, and the one that keeps the new reader
    /// from adding noise on the happy path. The hook's vocabulary is
    /// `clear|resume|logout|prompt_input_exit|other` and every one of them
    /// means a clean exit ran, so none may fold in with `crashed` --
    /// `crash.rs` keeps the two vocabularies disjoint on purpose. A `NULL`
    /// `end_reason` is the same: it is a run that ended without a recorded
    /// reason, which is not evidence of a crash.
    #[test]
    fn a_clean_or_unlabelled_end_is_not_called_a_crash() {
        for reason in [
            None,
            Some("clear"),
            Some("resume"),
            Some("logout"),
            Some("prompt_input_exit"),
            Some("other"),
        ] {
            let runs = [Run {
                pid: 4242,
                pid_start_time: Some(PROC_START.into()),
                ended_at: Some("2026-09-11T12:00:00Z".into()),
                end_reason: reason.map(str::to_string),
            }];
            match derive(&alive(PROC_START_EPOCH), &Registry::default(), "s1", &runs) {
                Liveness::Dead { why } => assert!(
                    !why.contains("without shutting down"),
                    "end_reason {reason:?} means a clean exit ran: {why}"
                ),
                other => panic!("expected Dead for {reason:?}, got {other:?}"),
            }
        }
    }

    /// The NEWEST run decides, not any run (#965).
    ///
    /// A session that crashed in March and then exited cleanly in July is
    /// over, cleanly, and the July record is the one describing its
    /// ending. `runs` arrives newest-first (`ORDER BY started_at DESC` in
    /// `sessions::runs_by_session`), so this pins that the reader takes
    /// the first element rather than scanning for any `crashed` anywhere
    /// in the history.
    #[test]
    fn the_newest_run_decides_how_the_session_ended() {
        let newest_clean = [
            Run {
                pid: 5000,
                pid_start_time: Some(PROC_START.into()),
                ended_at: Some("2026-07-01T12:00:00Z".into()),
                end_reason: Some("clear".into()),
            },
            Run {
                pid: 4242,
                pid_start_time: Some(PROC_START.into()),
                ended_at: Some("2026-03-01T12:00:00Z".into()),
                end_reason: Some(super::super::crash::CRASHED.into()),
            },
        ];
        match derive(&gone(), &Registry::default(), "s1", &newest_clean) {
            Liveness::Dead { why } => assert!(
                !why.contains("without shutting down"),
                "an older crash does not describe how this session ended: {why}"
            ),
            other => panic!("expected Dead, got {other:?}"),
        }

        // And the other order, so the assertion above is about ORDER
        // rather than about the reader never firing.
        let newest_crashed = [
            Run {
                pid: 5000,
                pid_start_time: Some(PROC_START.into()),
                ended_at: Some("2026-07-01T12:00:00Z".into()),
                end_reason: Some(super::super::crash::CRASHED.into()),
            },
            Run {
                pid: 4242,
                pid_start_time: Some(PROC_START.into()),
                ended_at: Some("2026-03-01T12:00:00Z".into()),
                end_reason: Some("clear".into()),
            },
        ];
        match derive(&gone(), &Registry::default(), "s1", &newest_crashed) {
            Liveness::Dead { why } => assert!(
                why.contains("without shutting down"),
                "the newest run crashed: {why}"
            ),
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

    /// A probe answering per pid, including the working directory
    /// (#1315). The single-answer `Fake` above cannot say "this pid is a
    /// live `.key` session in one folder, that one is gone".
    #[allow(clippy::type_complexity)]
    struct Table(HashMap<u32, (Result<Option<i64>, String>, Result<Option<PathBuf>, String>)>);
    impl ProcessProbe for Table {
        fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
            self.0.get(&pid).map(|a| a.0.clone()).unwrap_or(Ok(None))
        }
        fn cwd(&self, pid: u32) -> Result<Option<PathBuf>, String> {
            self.0.get(&pid).map(|a| a.1.clone()).unwrap_or(Ok(None))
        }
    }

    const WIDGET: &str = "/Users/acme/code/widget";

    /// A live process at `pid`, started at `start`, working in `cwd`.
    fn table(entries: &[(u32, i64, &str)]) -> Table {
        Table(
            entries
                .iter()
                .map(|(pid, start, cwd)| (*pid, (Ok(Some(*start)), Ok(Some(PathBuf::from(cwd))))))
                .collect(),
        )
    }

    /// The registry a terminal launch leaves: a `.key`, no `.json`.
    fn key_only_registry(proc_start: Option<&str>) -> Registry {
        Registry {
            unnamed: vec![UnnamedRecord {
                pid: 4242,
                proc_start: proc_start.map(str::to_string),
                path: "/Users/acme/.claude/sessions/4242.deadbeef.key".into(),
            }],
            ..Default::default()
        }
    }

    /// `read_registry` keeps a `.key` that has no `.json` (#1315).
    ///
    /// Real file shapes: the `.key` body is the measured
    /// `{"peerToken","procStart","pidDomain"}`, and the companion case --
    /// a `.key` beside its own `.json` -- is present too, so the test
    /// shows the two told apart rather than every `.key` collected.
    ///
    /// Sabotage: restoring the unconditional `continue` for non-`.json`
    /// files leaves `unnamed` empty and fails this.
    #[test]
    fn a_key_with_no_json_is_kept_as_an_unnamed_record() {
        let dir =
            std::env::temp_dir().join(format!("headstate-registry-1315-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("4242.0123456789abcdef.key"),
            format!(r#"{{"peerToken":"x","procStart":"{PROC_START}","pidDomain":"darwin"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("14779.fedcba9876543210.key"),
            format!(r#"{{"peerToken":"y","procStart":"{PROC_START}","pidDomain":"darwin"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("14779.json"),
            format!(r#"{{"pid":14779,"sessionId":"s1","procStart":"{PROC_START}"}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("notapid.key"), "{}").unwrap();

        let got = read_registry(&dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(got.failure, None);
        assert!(got.unreadable.is_empty(), "{:?}", got.unreadable);
        assert_eq!(got.entries.len(), 1, "the .key never becomes an entry");
        assert_eq!(
            got.unnamed.len(),
            1,
            "only the .key with no .json: {:?}",
            got.unnamed
        );
        assert_eq!(got.unnamed[0].pid, 4242);
        assert_eq!(got.unnamed[0].proc_start.as_deref(), Some(PROC_START));
        let mut pids = got.probe_pids();
        pids.sort_unstable();
        assert_eq!(pids, [4242, 14779], "the .key's pid must reach the probe");
    }

    /// **The #1315 defect.** A live `.key`-only session in this row's
    /// folder keeps the row from reading as stopped -- and does not make
    /// it `Running` either, because nothing says which session it is.
    ///
    /// Three `Dead` paths, all resting on "the registry would have shown
    /// it": the #984 absence, a session whose runs all ended (a resume of
    /// an old session), and an orphaned entry (a crashed session resumed
    /// from a terminal).
    ///
    /// Sabotage: making `derive_at` return `from_records` unchanged fails
    /// all three.
    #[test]
    fn a_live_key_only_session_keeps_its_rows_from_reading_dead() {
        let probe = table(&[(4242, PROC_START_EPOCH, WIDGET)]);
        let reg = key_only_registry(Some(PROC_START));
        let ended = [Run {
            pid: 5000,
            pid_start_time: Some(PROC_START.into()),
            ended_at: Some("2026-09-11T12:00:00Z".into()),
            ..Default::default()
        }];
        let mut orphaned = reg.clone();
        orphaned.entries = registry_with(None).entries; // pid 14779, gone

        for (what, reg, runs) in [
            ("never observed", &reg, &[][..]),
            ("every run ended", &reg, &ended[..]),
            ("orphaned entry", &orphaned, &[][..]),
        ] {
            match derive_at(&probe, reg, "s1", Some(WIDGET), runs) {
                Liveness::Unknown { why } => {
                    assert!(why.contains("pid 4242"), "{what}: must name the pid: {why}");
                    assert!(why.contains(WIDGET), "{what}: and where it runs: {why}");
                }
                other => panic!("{what}: a live unnamed session here must not read as {other:?}"),
            }
        }
    }

    /// The narrowing: a `.key`-only session in ANOTHER folder does not
    /// hedge this row, and one in an ancestor or descendant folder does.
    ///
    /// Without this one terminal launch would put every historical row
    /// back into `Unknown`, which is #984's 1,490-of-1,491 regression.
    /// Sabotage: making `could_be` return `true` fails the first
    /// assertion; making it compare paths for equality only fails the
    /// descendant and ancestor ones.
    #[test]
    fn a_key_only_session_only_hedges_rows_it_could_be() {
        let probe = table(&[(4242, PROC_START_EPOCH, WIDGET)]);
        let reg = key_only_registry(Some(PROC_START));
        let row = |cwd: Option<&str>| derive_at(&probe, &reg, "s1", cwd, &[]);

        assert!(
            matches!(row(Some("/Users/acme/code/gadget")), Liveness::Dead { .. }),
            "a session in another folder cannot be this one"
        );
        assert!(
            matches!(
                row(Some("/Users/acme/code/widget-old")),
                Liveness::Dead { .. }
            ),
            "a shared string prefix is not a shared folder"
        );
        assert!(
            matches!(row(Some("/Users/acme/code")), Liveness::Unknown { .. }),
            "a row recorded in an ancestor folder of the live process could be it"
        );
        // And the other direction: the row in a folder BELOW the process's.
        let above = table(&[(4242, PROC_START_EPOCH, "/Users/acme")]);
        assert!(
            matches!(
                derive_at(&above, &reg, "s1", Some(WIDGET), &[]),
                Liveness::Unknown { .. }
            ),
            "a row recorded in a descendant folder of the live process could be it"
        );
        assert!(
            matches!(row(None), Liveness::Unknown { .. }),
            "a row with no recorded folder could be any session"
        );
        assert!(
            matches!(derive(&probe, &reg, "s1", &[]), Liveness::Unknown { .. }),
            "`derive` knows no cwd, so it hedges"
        );
    }

    /// A `.key` left by a session that ENDED says nothing, and neither
    /// does one whose pid now belongs to another process.
    ///
    /// Absent is not zero in the other direction: a stale file is
    /// ordinary cleanup, and hedging on it would leave every row in the
    /// folder "could not tell" forever. Sabotage: dropping the
    /// `procStart` comparison fails the reused half.
    #[test]
    fn a_key_whose_pid_is_gone_or_reused_says_nothing() {
        let reg = key_only_registry(Some(PROC_START));
        let gone = table(&[]);
        let reused = table(&[(4242, PROC_START_EPOCH + 86_400, WIDGET)]);
        for (what, probe) in [("gone", &gone), ("reused", &reused)] {
            assert!(unnamed_sessions(probe, &reg).is_empty(), "{what}");
            assert!(
                matches!(
                    derive_at(probe, &reg, "s1", Some(WIDGET), &[]),
                    Liveness::Dead { .. }
                ),
                "{what}: the row keeps its settled answer"
            );
        }
    }

    /// A `.key` with no usable `procStart` whose pid IS there cannot be
    /// confirmed and cannot be ruled out: it hedges, and says it may be a
    /// different program. It never confirms anything.
    #[test]
    fn a_key_with_no_start_time_hedges_without_claiming_a_session() {
        let probe = table(&[(4242, PROC_START_EPOCH, WIDGET)]);
        for proc_start in [None, Some("2026-09-11T09:43:48Z")] {
            let reg = key_only_registry(proc_start);
            let lines = unnamed_sessions(&probe, &reg);
            assert_eq!(lines.len(), 1, "{proc_start:?}");
            assert!(
                lines[0].line.contains("may be a different program"),
                "{proc_start:?}: {}",
                lines[0].line
            );
            assert!(
                matches!(
                    derive_at(&probe, &reg, "s1", Some(WIDGET), &[]),
                    Liveness::Unknown { .. }
                ),
                "{proc_start:?}"
            );
        }
    }

    /// A process we could not look at, or whose folder we could not read,
    /// may be ANY session: every `Dead` row hedges.
    ///
    /// The fail-safe direction, and the one a narrowing is most tempted
    /// to get wrong -- treating "no folder" as "not this folder".
    #[test]
    fn an_unreadable_key_pid_or_folder_hedges_every_row() {
        let reg = key_only_registry(Some(PROC_START));
        let failed = Table(HashMap::from([(
            4242,
            (Err("Operation not permitted".into()), Ok(None)),
        )]));
        let no_cwd = Table(HashMap::from([(
            4242,
            (Ok(Some(PROC_START_EPOCH)), Err("could not read".into())),
        )]));
        for (what, probe) in [("probe failed", &failed), ("cwd unreadable", &no_cwd)] {
            assert!(
                matches!(
                    derive_at(probe, &reg, "s1", Some("/Users/acme/code/gadget"), &[]),
                    Liveness::Unknown { .. }
                ),
                "{what}"
            );
        }
    }

    /// A live `.key`-only session never touches a row that is `Running`
    /// on its own evidence -- it cannot make anything MORE certain.
    #[test]
    fn a_key_only_session_leaves_a_running_row_running() {
        let mut reg = registry_with(Some("busy"));
        reg.unnamed = key_only_registry(Some(PROC_START)).unnamed;
        let probe = table(&[
            (14779, PROC_START_EPOCH, WIDGET),
            (4242, PROC_START_EPOCH, WIDGET),
        ]);
        assert_eq!(
            derive_at(&probe, &reg, "s1", Some(WIDGET), &[]),
            Liveness::Running {
                pid: 14779,
                status: Some("busy".into())
            }
        );
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
        let probe = SysinfoProbe::for_pids(&reg.probe_pids());
        eprintln!(
            "registry {}: {} entries, {} unreadable, {} key-only, failure={:?}",
            dir.display(),
            reg.entries.len(),
            reg.unreadable.len(),
            reg.unnamed.len(),
            reg.failure
        );
        // #1315: the `.key`-only records, and which are live. A terminal
        // `claude` on this machine shows up here as a line with its pid
        // and folder; one that has exited says nothing.
        for u in unnamed_sessions(&probe, &reg) {
            eprintln!("  unnamed: {}", u.line);
            assert!(!u.line.is_empty());
        }
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
