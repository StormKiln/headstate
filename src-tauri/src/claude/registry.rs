//! The live session registry: `~/.claude/sessions/<pid>.json` (#913).
//!
//! Claude Code writes one JSON file per session into
//! `~/.claude/sessions/`, named for the session's pid. It carries
//! everything the hook payload does not: the pid itself, `procStart`,
//! `sessionId`, `cwd`, `name` and `version`.
//!
//! This module reads that directory, pairs each file's pid against the
//! live process table, and classifies the session. It never writes to
//! `~/.claude` -- the directory belongs to Claude Code, and one of these
//! files is rewritten by its owner every few seconds.
//!
//! # An orphan file is a positive crash signal, and that is the point
//!
//! The epic's original design inferred a crash from a MISSING
//! `SessionEnd` record. That is the weak form, because absence is also
//! what a still-running session looks like: a row with no end record is
//! either running, or crashed, and nothing distinguishes them.
//!
//! MEASURED instead (epic #910, third comment): a SIGKILLed session
//! **leaves its registry file behind**. Started a real session, confirmed
//! the file, SIGKILLed the pid, and the file was still present twenty
//! seconds later -- not a late reap. It carried `sessionId`, `procStart`,
//! `cwd`, and a `status: "busy"` frozen at the moment of death.
//!
//! So a file whose pid is dead is **direct, positive** evidence of a
//! crash, and it covers the case the handoff file cannot: a session that
//! died before ever appending an end record, which is every crash.
//!
//! # `status` is in the file and is deliberately distrusted
//!
//! The record carries `"status":"busy"|"idle"`. It is a STORED status that
//! a killed session never corrects -- the epic's own argument against a
//! `status` column, and the `is_some_and` fail-open of #841 in another
//! costume. The orphan above proves it: `busy` on a process that no longer
//! exists. So it is read, kept as [`Live::status`] for display, and never
//! consulted to decide whether anything is running. Liveness is derived
//! from the process table, every time.
//!
//! # The format trap that marks every session dead
//!
//! `procStart` and `ps` disagree in **two** ways at once. Measured on the
//! development machine, same process:
//!
//! ```text
//! registry procStart: Fri Sep 11 09:43:48 2026
//! ps lstart:          Fri 11 Sep 05:43:48 2026
//! ```
//!
//! 1. **Field order.** `Sep 11` against `11 Sep`. A `%a %b %e` format
//!    parses the first and REFUSES the second outright.
//! 2. **Timezone.** The registry writes UTC; `ps` reports local. Four
//!    hours apart here (`America/Toronto`, EDT).
//!
//! Both differences fail in the same direction: a string comparison
//! mismatches, and a parse that fixes the order but resolves the naive
//! time in the LOCAL zone lands 14,400 seconds away -- measured exactly,
//! not estimated. Either way every session reads as dead, and it would
//! look correct, because dead-but-resumable is the expected state for
//! almost every row. Nothing in the UI would flag it.
//!
//! The fix is not to parse `ps` at all. `sysinfo::Process::start_time()`
//! returns **epoch seconds**, and parsing `procStart` as UTC lands on it
//! exactly. Measured against all three live sessions on this machine:
//!
//! ```text
//! Fri Sep 11 09:43:48 2026 -> 1789119828   sysinfo 1789119828   delta 0
//! Fri Sep 11 14:05:01 2026 -> 1789135501   sysinfo 1789135501   delta 0
//! Sat Sep 12 11:57:33 2026 -> 1789214253   sysinfo 1789214253   delta 0
//! ```
//!
//! [`tests::the_two_clock_formats_do_not_both_parse_as_utc`] pins the trap
//! with a fixture in both spellings, so a later "simplification" to a
//! local-zone parse or a string compare fails rather than silently
//! condemning the whole list.
//!
//! # Absent is not zero
//!
//! A directory that cannot be read is an error, not an empty registry
//! (`caches/mod.rs:550`: "an idle time we could not read is not evidence
//! that anything is disposable"). A file that cannot be parsed is COUNTED
//! in [`Sweep::unreadable`] rather than skipped. A `procStart` we cannot
//! parse leaves `pid_start_time` NULL, which migration 11 documents as
//! "cannot confirm" and which the liveness layer must report as Unknown
//! -- never as Running, and never as Dead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One registry file, as read.
///
/// Every field but `pid` is optional because this is another program's
/// JSON: a record missing `sessionId` still tells us a pid was live, and
/// refusing the whole file over one absent field would lose that. `pid`
/// is not optional because the FILENAME carries it, so a file we can
/// name is a file whose pid we know.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Live {
    /// The session's pid, from the record and cross-checked against the
    /// filename -- see [`read_file`].
    pub pid: u32,
    /// `claude --resume`'s handle. `None` for a record that omitted it,
    /// which makes the row unresumable and is worth surfacing rather
    /// than dropping.
    pub session_id: Option<String>,
    /// `procStart`, verbatim, for the error message when it will not
    /// parse. The parsed form is [`Live::pid_start_epoch`].
    pub proc_start_raw: Option<String>,
    /// `procStart` as epoch seconds, UTC. `None` when it was absent or
    /// unparseable, which becomes a NULL `pid_start_time` and therefore
    /// Unknown liveness.
    pub pid_start_epoch: Option<i64>,
    pub cwd: Option<String>,
    /// The session's name, which exists in NO other source -- the hook
    /// payload has none and the transcript has `aiTitle` instead.
    pub name: Option<String>,
    pub claude_version: Option<String>,
    /// The record's own `status`, kept for DISPLAY only. Never consulted
    /// for liveness -- see the module docs on why a stored status lies
    /// about exactly the sessions this feature exists to resurrect.
    pub status: Option<String>,
    /// Where this was read from, so an error can name the file.
    pub path: PathBuf,
}

/// What a sweep of the registry directory found.
///
/// `Err` from [`sweep`] means the DIRECTORY could not be read. This
/// struct is the success case, and its `unreadable` is the per-file
/// half: a sweep that read four files and failed on a fifth reports four
/// entries and one failure, never five entries or an error.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sweep {
    /// Files whose pid is ALIVE, with a matching start time. A running
    /// session.
    pub running: Vec<Live>,
    /// Files whose pid is GONE, or alive under a different start time
    /// (a recycled pid). Each is a crashed session: the file survived
    /// the process, which only happens when no clean exit ran.
    pub orphaned: Vec<Live>,
    /// Files we read but whose liveness we could not determine, because
    /// `procStart` would not parse AND the pid is currently in use. A
    /// pid without a start time is a fail-open (migration 11), so this
    /// is neither running nor crashed.
    pub unknown: Vec<Live>,
    /// Per-file failures, with the path and the reason. Counted, never
    /// skipped.
    pub unreadable: Vec<String>,
}

impl Sweep {
    /// Every record read, regardless of how it was classified.
    pub fn all(&self) -> impl Iterator<Item = &Live> {
        self.running
            .iter()
            .chain(self.orphaned.iter())
            .chain(self.unknown.iter())
    }

    /// Whether anything could not be read. The caller that renders the
    /// list is the one that has to say "this may be incomplete".
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }
}

/// Where the registry lives, given a home directory.
///
/// The home is a PARAMETER for `claudemd::expand_home_in`'s reason:
/// `$HOME` is process-global state, and a test that mutates it races
/// every other test in the binary.
pub fn dir_in(home: &Path) -> PathBuf {
    home.join(".claude").join("sessions")
}

/// `procStart` as epoch seconds.
///
/// # Why UTC is hardcoded rather than taken from the system
///
/// Because it is a property of the WRITER, not of the reader. Claude Code
/// writes `procStart` in UTC; `ps` prints local. The two were measured
/// four hours apart on the development machine, and the match against
/// `sysinfo`'s epoch seconds is exact when this parses as UTC and
/// 14,400 seconds wrong when it parses as local. See the module docs.
///
/// `%e` rather than `%d` for the day because `%e` is the specifier that
/// DOCUMENTS a space-padded day, which is how a single-digit day arrives:
/// `Sep  1`, with two spaces. Measured, and worth writing down because it
/// contradicts the obvious guess: `chrono`'s `%d` accepts the padded form
/// too, so swapping the two does NOT break
/// [`tests::a_space_padded_day_parses`]. The test therefore pins the
/// BEHAVIOUR -- that a padded day parses -- rather than the specifier,
/// and the specifier is chosen for what it says to a reader. A stricter
/// parser is what this guards against, not today's one.
///
/// A value that will not parse returns `None`, which becomes a NULL
/// `pid_start_time` -- "cannot confirm", per migration 11 -- rather than
/// a zero or a guess.
pub fn parse_proc_start(raw: &str) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(raw.trim(), "%a %b %e %H:%M:%S %Y").ok()?;
    Some(naive.and_utc().timestamp())
}

/// The pid a registry filename claims, from `<pid>.json`.
///
/// The directory also holds `<pid>.<hex>.key` files, which are not
/// records and must not be reported as unreadable ones -- they are
/// Claude Code's own key material, and counting them as parse failures
/// would put a permanent "3 files could not be read" on a healthy
/// registry. Only `.json` names yield a pid.
fn pid_from_filename(name: &str) -> Option<u32> {
    name.strip_suffix(".json")?.parse().ok()
}

/// Read one registry file.
///
/// # Why the filename's pid wins
///
/// The filename is how Claude Code addresses the file, so it is the more
/// authoritative of the two. They agreed on every real file measured, but
/// if they ever disagree the body is the copy that could have been
/// written by a process that has since been replaced. The disagreement is
/// reported rather than silently resolved, because it would mean the
/// registry's own invariant had broken and a silent pick would hide that.
fn read_file(path: &Path) -> Result<Live, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: its name is not valid UTF-8", path.display()))?;
    let named_pid = pid_from_filename(name)
        .ok_or_else(|| format!("{}: the name is not <pid>.json", path.display()))?;

    let body = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("{}: it is not valid JSON: {e}", path.display()))?;

    if let Some(body_pid) = v.get("pid").and_then(serde_json::Value::as_u64) {
        if body_pid != u64::from(named_pid) {
            return Err(format!(
                "{}: the name says pid {named_pid} and the record says {body_pid}",
                path.display()
            ));
        }
    }

    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let proc_start_raw = s("procStart");

    Ok(Live {
        pid: named_pid,
        session_id: s("sessionId"),
        pid_start_epoch: proc_start_raw.as_deref().and_then(parse_proc_start),
        proc_start_raw,
        cwd: s("cwd"),
        name: s("name"),
        claude_version: s("version"),
        status: s("status"),
        path: path.to_path_buf(),
    })
}

/// Read every record in a registry directory, without classifying them.
///
/// Split from [`classify`] so the classification -- which needs a live
/// process table -- is testable against fixtures, and so a caller that
/// only wants the metadata does not pay for a process walk.
///
/// # An unreadable directory is an error
///
/// `Err` rather than an empty `Sweep`. A `0700` or absent
/// `~/.claude/sessions` and a machine with no sessions running are
/// different answers with opposite remedies, and the second is
/// reassuring when it is false. The one exception is a directory that
/// does not exist: Claude Code creates it on first run, so its absence
/// means "no session has ever started here", which IS an empty registry
/// and is reported as one.
pub fn read_dir(dir: &Path) -> Result<(Vec<Live>, Vec<String>), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // Claude Code creates this on first run, so absence is a real
        // empty registry rather than a failure to look.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };

    let mut records = Vec::new();
    let mut unreadable = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                unreadable.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        // Not a record. Skipped WITHOUT counting: the `.key` files
        // beside every record are Claude Code's own key material, and
        // counting them as failures would show a permanent "could not
        // read" on a healthy registry.
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if pid_from_filename(name).is_none() {
            continue;
        }
        match read_file(&path) {
            Ok(live) => records.push(live),
            Err(e) => unreadable.push(e),
        }
    }
    // Sorted by pid so a sweep is reproducible: `read_dir` order is the
    // filesystem's and differs between machines and between runs, which
    // would make a test that asserts on order flake rather than fail.
    records.sort_by_key(|r| r.pid);
    unreadable.sort();
    Ok((records, unreadable))
}

/// A live process's start time, epoch seconds, or `None` if it is gone.
///
/// This is the trait the classification is written against, so
/// [`classify`] can be tested with a fixture table instead of a real
/// process table -- a test cannot start a process with a chosen pid, and
/// one that used its own pid could only ever assert the Running arm.
pub trait ProcessTable {
    /// `None` when no process holds that pid. `Some(0)` when the process
    /// exists but its start time could not be read, which `sysinfo`
    /// reports as a literal zero on platforms where the accounting
    /// failed -- and which must NOT be compared as a real timestamp.
    fn start_time(&self, pid: u32) -> Option<u64>;
}

/// The real process table, via `sysinfo`.
///
/// One refresh over the pids we actually care about rather than the whole
/// table: a registry has single digits of entries, and
/// `ProcessesToUpdate::Some` is what `worktrees/scan.rs:801` already uses
/// for the same question.
pub struct SystemTable(HashMap<u32, u64>);

impl SystemTable {
    /// Snapshot the start times of the given pids.
    ///
    /// Taken as a snapshot rather than queried per pid so the whole
    /// sweep sees ONE moment: a process that exits midway through
    /// classification would otherwise be running for one row and gone
    /// for the next, and the two rows would contradict each other.
    pub fn snapshot(pids: &[u32]) -> Self {
        let mut map = HashMap::with_capacity(pids.len());
        if pids.is_empty() {
            return Self(map);
        }
        let wanted: Vec<sysinfo::Pid> = pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&wanted), true);
        for pid in pids {
            if let Some(proc) = sys.process(sysinfo::Pid::from_u32(*pid)) {
                map.insert(*pid, proc.start_time());
            }
        }
        Self(map)
    }
}

impl ProcessTable for SystemTable {
    fn start_time(&self, pid: u32) -> Option<u64> {
        self.0.get(&pid).copied()
    }
}

/// How far apart a claimed and an observed start time may be.
///
/// `procStart` and `sysinfo::start_time` agreed EXACTLY on all three live
/// sessions measured (delta 0, see the module docs), so this is not
/// absorbing a known skew -- it is absorbing the one-second rounding two
/// independent formatters of the same instant can disagree by. Kept
/// tight on purpose: the whole job of the start time is to catch a
/// recycled pid, and a wide window is how a pid-reuse guard stops
/// guarding. `worktrees/scan.rs` uses 300s for a ctime with no timezone,
/// which is a looser input than this one.
const START_TOLERANCE_SECS: i64 = 2;

/// Classify each record against a process table.
///
/// # The three answers, and why there are three
///
/// - **Running** -- the pid is live and its start time matches. Both
///   halves required: a pid alone is a fail-open, because pids are
///   recycled and a reissued number would report a long-dead session as
///   running. Same defence as `health/runaway.rs`'s `(pid, start_time)`
///   identity.
/// - **Orphaned** -- the pid is gone, or it is live under a DIFFERENT
///   start time. Either way the process that wrote this file is not
///   running and never cleaned up after itself, which is a crash. The
///   recycled-pid case belongs here rather than in Unknown: we have
///   positive evidence the original process ended.
/// - **Unknown** -- `procStart` would not parse (or was absent) AND
///   something holds the pid. We cannot tell whether that something is
///   our session. Reported as Unknown rather than Running, per migration
///   11: a NULL `pid_start_time` means "cannot confirm", and the
///   fail-open direction here would claim a crashed session is alive and
///   hide the Resume button the feature exists to offer.
///
/// A missing `procStart` with a DEAD pid is still Orphaned, not Unknown:
/// nothing holds the pid, so there is nothing the missing start time
/// could have disambiguated.
///
/// `Some(0)` from the table is treated as "cannot confirm" rather than as
/// the epoch, following `worktrees/scan.rs:801`'s handling of the same
/// `sysinfo` quirk. It lands in Unknown, because the process does exist.
pub fn classify(
    records: Vec<Live>,
    table: &impl ProcessTable,
) -> (Vec<Live>, Vec<Live>, Vec<Live>) {
    let mut running = Vec::new();
    let mut orphaned = Vec::new();
    let mut unknown = Vec::new();

    for rec in records {
        match (table.start_time(rec.pid), rec.pid_start_epoch) {
            // Nothing holds the pid. The file outlived its process, which
            // is the crash signal -- and it is positive evidence, unlike
            // a missing SessionEnd.
            (None, _) => orphaned.push(rec),
            // The process exists but its start time is unreadable, so the
            // pid-reuse guard cannot run. Not Running: an unconfirmed
            // liveness must never read as alive.
            (Some(0), _) => unknown.push(rec),
            // We know when the process started but not when the session
            // did, so we cannot tell whether they are the same one.
            (Some(_), None) => unknown.push(rec),
            (Some(actual), Some(claimed)) => {
                let actual = i64::try_from(actual).unwrap_or(i64::MAX);
                if (actual - claimed).abs() <= START_TOLERANCE_SECS {
                    running.push(rec);
                } else {
                    // A live process under a RECYCLED pid. The session
                    // that wrote this file is provably gone.
                    orphaned.push(rec);
                }
            }
        }
    }
    (running, orphaned, unknown)
}

/// Read and classify a registry directory in one pass.
pub fn sweep(dir: &Path) -> Result<Sweep, String> {
    let (records, unreadable) = read_dir(dir)?;
    let pids: Vec<u32> = records.iter().map(|r| r.pid).collect();
    let table = SystemTable::snapshot(&pids);
    let (running, orphaned, unknown) = classify(records, &table);
    Ok(Sweep {
        running,
        orphaned,
        unknown,
        unreadable,
    })
}

/// Sweep the real `~/.claude/sessions`.
///
/// `Err` when there is no home directory, because then there is no
/// registry to read and an empty list would be a claim we cannot make.
pub fn sweep_default() -> Result<Sweep, String> {
    let home = crate::auth::home_dir()
        .ok_or_else(|| "no home directory, so ~/.claude/sessions cannot be read".to_string())?;
    sweep(&dir_in(&home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A process table built from a table of `(pid, start_time)`.
    fn table(rows: &[(u32, u64)]) -> impl ProcessTable + '_ {
        struct Fake<'a>(&'a [(u32, u64)]);
        impl ProcessTable for Fake<'_> {
            fn start_time(&self, pid: u32) -> Option<u64> {
                self.0.iter().find(|(p, _)| *p == pid).map(|(_, t)| *t)
            }
        }
        Fake(rows)
    }

    fn rec(pid: u32, proc_start: Option<&str>) -> Live {
        let raw = proc_start.map(str::to_owned);
        Live {
            pid,
            session_id: Some(format!("sid-{pid}")),
            pid_start_epoch: raw.as_deref().and_then(parse_proc_start),
            proc_start_raw: raw,
            cwd: Some("/Users/acme/code/widget".into()),
            name: Some(format!("widget-{pid}")),
            claude_version: Some("2.1.268".into()),
            status: Some("busy".into()),
            path: PathBuf::from(format!("/Users/acme/.claude/sessions/{pid}.json")),
        }
    }

    /// THE FORMAT TRAP, in both spellings (#913, epic #910).
    ///
    /// The registry writes `Sep 11` in UTC; `ps` prints `11 Sep` in local
    /// time. Two differences at once, and both fail toward "every session
    /// is dead" -- which looks correct, because dead-but-resumable is
    /// what most rows genuinely are.
    ///
    /// The real pairing this pins is MEASURED: the registry's
    /// `Fri Sep 11 09:43:48 2026` and `sysinfo`'s `1789119828` are the
    /// same instant, delta 0, on all three live sessions on the
    /// development machine.
    ///
    /// Proven by sabotage. Replacing `naive.and_utc()` with
    /// `naive.and_local_timezone(chrono::Local).unwrap()` fails this with
    /// `1789134228 != 1789119828` -- exactly 14,400 seconds, the
    /// measured four-hour offset.
    #[test]
    fn the_two_clock_formats_do_not_both_parse_as_utc() {
        // The registry's spelling, which is the one we consume.
        assert_eq!(
            parse_proc_start("Fri Sep 11 09:43:48 2026"),
            Some(1_789_119_828),
            "the registry writes UTC in `Mon DD` order, and this is the \
             exact epoch sysinfo reports for that same process"
        );

        // `ps -o lstart=`'s spelling of the SAME instant. It must not
        // parse: the day and month are transposed, so accepting it would
        // mean the format is loose enough to also mis-read something.
        assert_eq!(
            parse_proc_start("Fri 11 Sep 05:43:48 2026"),
            None,
            "`ps` order is a different format and must be refused, not \
             coerced -- silently reading it would put the day in the \
             month's place"
        );

        // And the zone is not incidental: the two strings above are the
        // same moment, so if the parse ignored the zone they would have
        // to be 14,400 seconds apart. That gap is the whole trap.
        let ps_as_if_utc = parse_proc_start("Fri Sep 11 05:43:48 2026").unwrap();
        assert_eq!(
            1_789_119_828 - ps_as_if_utc,
            14_400,
            "the registry is UTC and ps is local -- four hours on the \
             machine this was measured on"
        );
    }

    /// A single-digit day arrives space-padded, and must parse.
    ///
    /// Nine days a month, so this is not an edge case -- it is 30% of the
    /// calendar.
    ///
    /// Sabotage, and the result was NEGATIVE: `%d` in place of `%e` keeps
    /// this GREEN, because `chrono`'s `%d` accepts a space-padded day as
    /// well. Recorded rather than quietly dropped, because the honest
    /// reading is that this test pins the behaviour and not the format
    /// specifier -- a future parser that is stricter about padding is what
    /// it would catch, and today's `%d`/`%e` choice is a statement to a
    /// reader rather than a thing under test.
    #[test]
    fn a_space_padded_day_parses() {
        let padded = parse_proc_start("Tue Sep  1 09:43:48 2026");
        assert!(
            padded.is_some(),
            "`Sep  1` has two spaces; a format that refuses it is broken \
             for nine days of every month"
        );
        assert_eq!(padded, parse_proc_start("Tue Sep 1 09:43:48 2026"));
    }

    /// Garbage is `None`, not a zero.
    ///
    /// `None` becomes a NULL `pid_start_time`, which migration 11
    /// documents as "cannot confirm". A zero would be a timestamp in
    /// 1970 that the tolerance check would then compare against a real
    /// one and call a recycled pid.
    #[test]
    fn an_unparseable_start_time_is_absent_rather_than_zero() {
        assert_eq!(parse_proc_start(""), None);
        assert_eq!(parse_proc_start("not a date"), None);
        assert_eq!(parse_proc_start("1789119828"), None);
    }

    /// A live pid with a matching start time is running.
    #[test]
    fn a_matching_pid_and_start_time_is_running() {
        let (running, orphaned, unknown) = classify(
            vec![rec(14779, Some("Fri Sep 11 09:43:48 2026"))],
            &table(&[(14779, 1_789_119_828)]),
        );
        assert_eq!(running.len(), 1);
        assert!(orphaned.is_empty() && unknown.is_empty());
    }

    /// THE CRASH SIGNAL: a file whose pid is gone.
    ///
    /// This is the whole addition to #913. A SIGKILLed session leaves its
    /// file behind -- measured, epic #910 -- so a record with no live pid
    /// is positive evidence of a crash, unlike the absence of a
    /// `SessionEnd` which also describes a running session.
    #[test]
    fn a_file_whose_pid_is_gone_is_a_crash() {
        let (running, orphaned, unknown) = classify(
            vec![rec(80043, Some("Sun Sep 13 09:57:19 2026"))],
            &table(&[]),
        );
        assert!(running.is_empty());
        assert_eq!(orphaned.len(), 1, "the file outlived its process");
        assert!(unknown.is_empty());
    }

    /// A RECYCLED pid is a crash, not a running session.
    ///
    /// The fail-open this guards is the reason `pid_start_time` exists at
    /// all (migration 11). Without the start-time half, a long-dead
    /// session whose number the kernel reissued would report as alive and
    /// its Resume button would never appear.
    ///
    /// Sabotage: dropping the tolerance comparison and pushing every live
    /// pid into `running` fails this.
    #[test]
    fn a_recycled_pid_is_not_the_session_that_claimed_it() {
        let (running, orphaned, unknown) = classify(
            // Claimed Sep 11; the process holding that pid now started
            // Sep 13, two days later.
            vec![rec(14779, Some("Fri Sep 11 09:43:48 2026"))],
            &table(&[(14779, 1_789_293_439)]),
        );
        assert!(
            running.is_empty(),
            "a pid alone is a fail-open; the start time is what makes \
             derived liveness sound"
        );
        assert_eq!(orphaned.len(), 1);
        assert!(unknown.is_empty());
    }

    /// A live pid with NO parseable start time is Unknown, never Running.
    ///
    /// Migration 11: NULL `pid_start_time` means "cannot confirm", and
    /// the liveness layer must report Unknown. Reporting Running would
    /// hide the Resume button on a session that had actually crashed.
    ///
    /// Sabotage: folding the `(Some(_), None)` arm into `running` fails
    /// this.
    #[test]
    fn a_live_pid_with_no_confirmable_start_time_is_unknown() {
        let (running, orphaned, unknown) =
            classify(vec![rec(14779, None)], &table(&[(14779, 1_789_119_828)]));
        assert!(
            running.is_empty(),
            "cannot-confirm must not read as Running"
        );
        assert!(orphaned.is_empty(), "and must not read as crashed either");
        assert_eq!(unknown.len(), 1);
        assert_eq!(
            unknown[0].pid_start_epoch, None,
            "which is the NULL that reaches pid_start_time"
        );
    }

    /// `sysinfo` reporting a literal zero start time is Unknown too.
    ///
    /// A zero is the platform's "accounting failed", not the epoch, and
    /// comparing it as a timestamp would call every session a recycled
    /// pid. Same handling as `worktrees/scan.rs:801`.
    #[test]
    fn a_zero_start_time_from_the_platform_is_unknown() {
        let (running, orphaned, unknown) = classify(
            vec![rec(14779, Some("Fri Sep 11 09:43:48 2026"))],
            &table(&[(14779, 0)]),
        );
        assert!(running.is_empty() && orphaned.is_empty());
        assert_eq!(unknown.len(), 1);
    }

    /// A DEAD pid with no start time is still a crash.
    ///
    /// Nothing holds the pid, so there is nothing the missing start time
    /// could have disambiguated. Pushing this into Unknown would lose a
    /// known-crashed session to a field that could not have changed the
    /// answer.
    #[test]
    fn a_dead_pid_needs_no_start_time_to_be_a_crash() {
        let (running, orphaned, unknown) = classify(vec![rec(80043, None)], &table(&[]));
        assert!(running.is_empty() && unknown.is_empty());
        assert_eq!(orphaned.len(), 1);
    }

    /// A one-second formatting difference is not a recycled pid.
    #[test]
    fn a_second_of_rounding_is_still_the_same_process() {
        let (running, _, _) = classify(
            vec![rec(14779, Some("Fri Sep 11 09:43:48 2026"))],
            &table(&[(14779, 1_789_119_829)]),
        );
        assert_eq!(running.len(), 1);
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// A real-shaped record reads every field we use.
    #[test]
    fn a_real_record_reads() {
        let t = tempfile::TempDir::new().unwrap();
        // The shape of a real file from the development machine, with
        // the paths and ids replaced by this repo's `acme` placeholder.
        write(
            t.path(),
            "14779.json",
            r#"{"pid":14779,"sessionId":"e5dff3bd-1111-2222-3333-444455556666",
                "cwd":"/Users/acme/code/widget","startedAt":1789119830687,
                "procStart":"Fri Sep 11 09:43:48 2026","version":"2.1.268",
                "kind":"interactive","entrypoint":"cli","name":"widget-c3",
                "nameSource":"derived","status":"busy","updatedAt":1789299111704}"#,
        );

        let (records, unreadable) = read_dir(t.path()).unwrap();
        assert!(unreadable.is_empty(), "{unreadable:?}");
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.pid, 14779);
        assert_eq!(
            r.session_id.as_deref(),
            Some("e5dff3bd-1111-2222-3333-444455556666")
        );
        assert_eq!(r.pid_start_epoch, Some(1_789_119_828));
        assert_eq!(r.cwd.as_deref(), Some("/Users/acme/code/widget"));
        assert_eq!(r.name.as_deref(), Some("widget-c3"));
        assert_eq!(r.claude_version.as_deref(), Some("2.1.268"));
        assert_eq!(
            r.status.as_deref(),
            Some("busy"),
            "read for display; never for liveness"
        );
    }

    /// The `.key` files beside every record are not parse failures.
    ///
    /// The real directory holds `<pid>.<hex>.key` next to each record.
    /// Counting them would put a permanent "3 files could not be read" on
    /// a healthy registry, which is a false partial-failure warning --
    /// the #745 defect, where an advisory that could not take itself back
    /// sat over correct data.
    ///
    /// Sabotage: dropping the `pid_from_filename(name).is_none()` skip
    /// fails this with 1 unreadable.
    #[test]
    fn the_key_files_beside_the_records_are_not_failures() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "14779.json", r#"{"pid":14779}"#);
        write(t.path(), "14779.b58125478.key", "not json at all");
        write(t.path(), "README", "nor this");

        let (records, unreadable) = read_dir(t.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            unreadable.is_empty(),
            "key material is not a record that failed to parse: {unreadable:?}"
        );
    }

    /// A malformed record is COUNTED, and the others still read.
    ///
    /// Absent is not zero: a sweep that read three files and failed on a
    /// fourth reports three entries and one failure. Neither four
    /// entries, nor an error, nor three entries and silence.
    #[test]
    fn a_malformed_record_is_counted_and_the_rest_still_read() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "1.json", r#"{"pid":1}"#);
        write(t.path(), "2.json", "{ this is not json");
        write(t.path(), "3.json", r#"{"pid":3}"#);

        let (records, unreadable) = read_dir(t.path()).unwrap();
        assert_eq!(records.len(), 2, "the readable ones still read");
        assert_eq!(unreadable.len(), 1, "and the failure is reported");
        assert!(
            unreadable[0].contains("2.json") && unreadable[0].contains("not valid JSON"),
            "the message must name the file and say what was wrong: {:?}",
            unreadable[0]
        );
    }

    /// A record whose body disagrees with its filename is refused.
    #[test]
    fn a_pid_that_disagrees_with_its_filename_is_refused() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "14779.json", r#"{"pid":99999,"status":"busy"}"#);

        let (records, unreadable) = read_dir(t.path()).unwrap();
        assert!(records.is_empty());
        assert_eq!(unreadable.len(), 1);
        assert!(
            unreadable[0].contains("14779") && unreadable[0].contains("99999"),
            "both numbers belong in the message: {:?}",
            unreadable[0]
        );
    }

    /// A record with only a pid still reads, degraded.
    ///
    /// Every field but the pid is another program's JSON. A record
    /// missing `sessionId` still tells us a pid was live, and refusing
    /// the file over it would lose that.
    #[test]
    fn a_record_with_only_a_pid_still_reads() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "42.json", "{}");

        let (records, unreadable) = read_dir(t.path()).unwrap();
        assert!(unreadable.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pid, 42);
        assert_eq!(records[0].session_id, None);
        assert_eq!(records[0].pid_start_epoch, None);
    }

    /// An ABSENT directory is an empty registry.
    ///
    /// Claude Code creates it on first run, so its absence means no
    /// session has ever started here. That is genuinely nothing, and the
    /// only case where an empty list is the honest answer.
    #[test]
    fn an_absent_directory_is_genuinely_empty() {
        let t = tempfile::TempDir::new().unwrap();
        let (records, unreadable) = read_dir(&t.path().join("never-created")).unwrap();
        assert!(records.is_empty() && unreadable.is_empty());
    }

    /// An UNREADABLE directory is an error, not an empty registry.
    ///
    /// The house rule (`caches/mod.rs:550`): something we could not read
    /// is not evidence of absence. A `0700` registry and a machine with
    /// no sessions have opposite remedies.
    ///
    /// Sabotage: returning `Ok(Default::default())` from the `Err` arm
    /// fails this.
    #[test]
    fn an_unreadable_directory_is_an_error() {
        let t = tempfile::TempDir::new().unwrap();
        // A FILE where a directory should be: `read_dir` fails with
        // NotADirectory, which is a real failure rather than NotFound.
        let blocker = t.path().join("sessions");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let err = read_dir(&blocker).expect_err("a file is not a readable directory");
        assert!(
            err.contains("sessions"),
            "the error must name the path: {err}"
        );
    }

    /// The output order is by pid, not the filesystem's.
    #[test]
    fn records_come_back_in_pid_order() {
        let t = tempfile::TempDir::new().unwrap();
        for pid in [300u32, 100, 200] {
            write(
                t.path(),
                &format!("{pid}.json"),
                &format!(r#"{{"pid":{pid}}}"#),
            );
        }
        let (records, _) = read_dir(t.path()).unwrap();
        assert_eq!(
            records.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
    }

    /// A sweep of a fixture directory classifies against the REAL process
    /// table, and the one thing that can be asserted there is the
    /// direction: pids that cannot exist are crashes.
    ///
    /// `u32::MAX` is above every platform's `pid_max`, so nothing can
    /// hold it. That makes this the one liveness assertion a test can
    /// make about the real table without starting a process.
    #[test]
    fn a_sweep_against_the_real_process_table_calls_an_impossible_pid_dead() {
        let t = tempfile::TempDir::new().unwrap();
        write(
            t.path(),
            &format!("{}.json", u32::MAX),
            &format!(
                r#"{{"pid":{},"sessionId":"gone","procStart":"Fri Sep 11 09:43:48 2026"}}"#,
                u32::MAX
            ),
        );

        let swept = sweep(t.path()).unwrap();
        assert!(swept.running.is_empty() && swept.unknown.is_empty());
        assert_eq!(swept.orphaned.len(), 1);
        assert!(!swept.is_partial());
    }

    /// Our OWN process is running, which is the other half of the real
    /// table: an impossible pid proves the Dead arm, and this proves the
    /// Running arm is reachable at all.
    ///
    /// Without it, `fn classify(..) { (vec![], records, vec![]) }` --
    /// everything is a crash -- would pass every other test here. That
    /// is exactly the failure the format trap produces, so this is the
    /// guard against shipping it.
    #[test]
    fn our_own_process_reads_as_running() {
        let t = tempfile::TempDir::new().unwrap();
        let me = std::process::id();
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(me)]),
            true,
        );
        let started = sys
            .process(sysinfo::Pid::from_u32(me))
            .expect("our own process is in the table")
            .start_time();
        // Written the way the registry writes it: UTC, `Mon DD` order.
        // If this round-trip were wrong the test would fail, which is
        // the format trap caught from the other side.
        let proc_start = chrono::DateTime::from_timestamp(started as i64, 0)
            .expect("a plausible start time")
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();
        write(
            t.path(),
            &format!("{me}.json"),
            &format!(r#"{{"pid":{me},"sessionId":"self","procStart":"{proc_start}"}}"#),
        );

        let swept = sweep(t.path()).unwrap();
        assert_eq!(
            swept.running.len(),
            1,
            "our own pid and start time must classify as running -- \
             orphaned {:?} unknown {:?}",
            swept.orphaned.len(),
            swept.unknown.len()
        );
    }

    /// THE END-TO-END CRASH PROOF: a real process, a real SIGKILL, and
    /// the sweep classifying the survivor as a crash (#913, epic #910).
    ///
    /// Every other test here either fakes the process table or asserts on
    /// a pid that cannot exist. This one does the whole thing for real,
    /// in the order the measured finding describes:
    ///
    /// 1. spawn a process and write a registry file for it, with
    ///    `procStart` spelled the way Claude Code spells it -- UTC, in
    ///    `Mon DD` order -- derived from the process table's own epoch
    ///    seconds, so the round-trip through the format is exercised;
    /// 2. sweep, and it is RUNNING;
    /// 3. `SIGKILL` it -- the signal `SessionEnd` does not fire on;
    /// 4. sweep again, and the same unchanged file is now a CRASH.
    ///
    /// The second sweep is what makes the first one load-bearing: without
    /// it, a `classify` that called everything dead would pass, and that
    /// is precisely what the format trap produces.
    ///
    /// # Why this is `#[ignore]`d
    ///
    /// It spawns a process, signals it, and waits on a real reap. That is
    /// three pieces of OS behaviour whose timing CI cannot promise, and a
    /// flake here would be read as "the crash detection is unreliable"
    /// when the truth would be "the runner was busy". The unit tests
    /// above cover every branch of [`classify`] deterministically; this
    /// exists so the claim can be re-verified by hand on a real machine:
    ///
    /// ```text
    /// cargo test --lib the_whole_crash_path -- --ignored --nocapture
    /// ```
    ///
    /// It writes only into a `tempfile::TempDir`. Nothing under the
    /// user's own `~/.claude` is touched.
    #[test]
    #[ignore = "spawns and kills a real process; run with --ignored"]
    fn the_whole_crash_path_from_a_real_sigkill() {
        let t = tempfile::TempDir::new().unwrap();

        // A real child with a real pid and a real start time.
        let mut child = std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .expect("spawn a child to kill");
        let pid = child.id();

        // Its start time, read from the process table and then WRITTEN
        // BACK OUT in the registry's own spelling. Going through the
        // string is the point: a format bug would surface here as the
        // running assertion failing.
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
        );
        let started = sys
            .process(sysinfo::Pid::from_u32(pid))
            .expect("the child we just spawned")
            .start_time();
        let proc_start = chrono::DateTime::from_timestamp(started as i64, 0)
            .expect("a plausible start time")
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();

        let dir = t.path().join(".claude").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{pid}.json"));
        std::fs::write(
            &file,
            format!(
                r#"{{"pid":{pid},"sessionId":"proof-of-crash","cwd":"/Users/acme/code/widget",
                     "procStart":"{proc_start}","version":"2.1.269","kind":"interactive",
                     "entrypoint":"cli","name":"crash-proof","status":"busy"}}"#
            ),
        )
        .unwrap();

        // 2. Alive.
        let alive = sweep(&dir).unwrap();
        println!(
            "[proof] pid {pid} procStart {proc_start:?} -> running {} orphaned {} unknown {}",
            alive.running.len(),
            alive.orphaned.len(),
            alive.unknown.len()
        );
        assert_eq!(
            alive.running.len(),
            1,
            "a live pid whose procStart round-tripped through the \
             registry format must read as running -- if this fails, the \
             format trap is back"
        );

        // 3. SIGKILL: the signal `SessionEnd` does NOT fire on, which is
        //    the whole reason this source exists.
        child.kill().expect("SIGKILL the child");
        let _ = child.wait();

        // 4. The SAME file, untouched, is now a crash.
        let before = std::fs::read_to_string(&file).unwrap();
        let dead = sweep(&dir).unwrap();
        println!(
            "[proof] after SIGKILL -> running {} orphaned {} unknown {}",
            dead.running.len(),
            dead.orphaned.len(),
            dead.unknown.len()
        );
        assert!(
            dead.running.is_empty(),
            "a killed process must not still read as running"
        );
        assert_eq!(
            dead.orphaned.len(),
            1,
            "the registry file outlived its process, which is the positive \
             crash signal -- unlike a missing SessionEnd, which a running \
             session also looks like"
        );
        assert!(dead.unknown.is_empty());
        assert_eq!(
            dead.orphaned[0].status.as_deref(),
            Some("busy"),
            "and it carries the STALE status frozen at the moment of \
             death, which is why a stored status can never be trusted for \
             liveness"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            before,
            "the sweep is read-only: the file it classified is unchanged"
        );

        // 5. And the crash reaches the database as an ended-by-crash run.
        let mut conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let recorded = super::super::crash::record(&mut conn, &dead).unwrap();
        println!(
            "[proof] recorded: crashed {} running {} unknown {}",
            recorded.crashed, recorded.running, recorded.unknown
        );
        assert_eq!(recorded.crashed, 1);
        let (sid, reason, ended): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT session_id, end_reason, ended_at FROM claude_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        println!("[proof] row: {sid} end_reason={reason:?} ended_at={ended:?}");
        assert_eq!(sid, "proof-of-crash");
        assert_eq!(reason.as_deref(), Some(super::super::crash::CRASHED));
        assert!(ended.is_some());
    }

    /// The unreadable count survives the sweep, not just `read_dir`.
    #[test]
    fn a_sweep_carries_its_failures() {
        let t = tempfile::TempDir::new().unwrap();
        write(t.path(), "7.json", "{ broken");
        let swept = sweep(t.path()).unwrap();
        assert!(swept.is_partial());
        assert_eq!(swept.unreadable.len(), 1);
        assert_eq!(swept.all().count(), 0);
    }
}
