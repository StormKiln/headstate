//! Consuming `~/.claude/headstate/sessions.jsonl`, the file the hook
//! appends to (#913, writer in #912).
//!
//! The hook runs inside a 1.5 second budget and does exactly one thing:
//! serialise a record and append it. Everything expensive belongs here,
//! because this side has no budget over it -- validating a payload
//! against disk, resolving a start time, writing rows.
//!
//! # Byte offsets, not a filesystem watcher
//!
//! `notify` is not a dependency, and adding one would buy the exact
//! failure mode the epic forbids: a dead FSEvents stream reports "no new
//! sessions" indistinguishably from "the watch died", and on macOS a
//! stream dies silently. Polling's failure mode is legible -- if the tick
//! stops, the log stops with it.
//!
//! So consumption is a stored byte offset. Each pass opens the file,
//! seeks to the offset, reads what is new, and advances. A pass over an
//! unchanged file reads zero bytes and costs one `stat`.
//!
//! # The four things that can happen to a file between passes
//!
//! Each of these is a distinct case with a distinct answer, and getting
//! any of them wrong loses records silently:
//!
//! 1. **It grew.** The normal case: read from the offset to the end.
//! 2. **It SHRANK.** Someone truncated or replaced it -- our own
//!    rotation below, or a user clearing it by hand. The offset now
//!    points past the end, or worse into the middle of a different
//!    record. Detected by comparing the file's length against the stored
//!    offset, and answered by resetting the offset to zero and re-reading
//!    from the start. See [`Offset::advance`] on why re-reading is safe.
//! 3. **It ends mid-line.** A hook's single `write` is atomic, so this
//!    cannot be a torn record -- but a file can still be read at the
//!    moment between two appends, and a reader that assumed the last
//!    byte was a newline would either drop the last record or parse half
//!    of one. Answered by consuming only up to the last newline and
//!    leaving the remainder for the next pass.
//! 4. **It is gone.** Reported as "nothing to read" rather than as an
//!    error, because a missing handoff file is what a machine with the
//!    hook not yet installed looks like. Distinguished from an
//!    unreadable one, which IS an error.
//!
//! # Rotation truncates AFTER the commit, never before
//!
//! The ordering is the whole safety property. Records are read, written
//! to the database inside one transaction, the transaction commits, and
//! only then is the file truncated. If truncation came first, a crash
//! between the two would lose every record in the window; as ordered, a
//! crash means the next pass re-reads records that are already stored,
//! and the upsert makes that a no-op.
//!
//! That is why rotation is the CONSUMER's job and not the hook's. A hook
//! that truncated on a size threshold could drop a record this side had
//! never seen, and it cannot know which ones those are.
//!
//! # Absent is not zero
//!
//! A line that will not parse is COUNTED in [`Consumed::unparseable`],
//! never skipped. A record with an unknown `v` is counted separately, in
//! [`Consumed::unknown_version`], because that is a different problem
//! with a different remedy: the hook on disk is newer than this reader,
//! which happens because the installed hook command line keeps running
//! whatever binary is at that path across an app upgrade. A read failure
//! is an `Err`, never an empty list -- see `caches/mod.rs:550`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// The record version this reader understands.
///
/// A line whose `v` is anything else is counted and skipped rather than
/// guessed at. The hook and the reader are separate binaries on disk and
/// can be different versions at the same time, because #915 installs a
/// command line once and it keeps running whatever lives at that path.
///
/// # The version rule (#1061): a new optional field does NOT bump this
///
/// The gate in [`parse_line`] is **strict equality**, and that is what
/// decides the rule. A bump does not make old readers read new records a
/// little worse; it makes them count **every** record as
/// `unknown_version` and store none of it, including the
/// `SessionStart`/`SessionEnd` records they have always understood.
///
/// So epic #1060's six new events ride at `v: 1` as **optional** fields,
/// following the `source`/`reason` precedent. This struct has no
/// `#[serde(deny_unknown_fields)]`, so a field written by a newer hook
/// that this reader has never heard of is ignored and the fields it does
/// know still arrive -- the degrade-don't-crash path, pinned by
/// [`tests::a_reader_without_the_new_fields_still_reads_a_new_record`].
///
/// Bump only when an existing field changes MEANING, is removed, changes
/// type, or becomes required -- the cases where an old reader would be
/// confidently wrong rather than merely incomplete. See
/// [`super::hook::RECORD_VERSION`] for the writer's half of the same rule.
pub const RECORD_VERSION: u32 = 1;

/// One line of the handoff file, as the hook wrote it (#912).
///
/// Deliberately a SEPARATE type from the hook's own struct rather than a
/// shared one, and that is not duplication for its own sake: the writer's
/// struct describes what it chooses to emit, and this one describes what
/// this reader is willing to accept. They are the same shape today and
/// must be free to diverge -- a newer hook adding a field must not stop
/// an older reader from reading the fields it already understood, which
/// is exactly what a shared struct with a new non-optional field would
/// do.
///
/// Every field but `v`, `event` and `ppid` is optional because the hook
/// writes what the payload gave it, and a payload missing `cwd` still
/// carried a pid and a session id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Record {
    /// Format version. Checked against [`RECORD_VERSION`] before anything
    /// else is trusted.
    pub v: u32,
    /// `SessionStart` or `SessionEnd`, from the payload's
    /// `hook_event_name`, or the hook's literal `"unknown"`.
    pub event: String,
    /// The `claude --resume` handle. `None` makes the record unusable for
    /// a row, which is counted rather than silently dropped.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The Claude Code process id, which the hook took from its own
    /// parent.
    ///
    /// `0` is the hook's documented "could not tell" sentinel (its
    /// Windows arm), and is not a valid pid on any platform. It must
    /// never be checked for liveness or stored as an observed pid.
    pub ppid: u32,
    /// When the hook ran, RFC 3339.
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// `SessionStart`'s `source`: `startup|resume|clear|compact|fork`.
    #[serde(default)]
    pub source: Option<String>,
    /// `SessionEnd`'s `reason`: `clear|resume|logout|prompt_input_exit|other`.
    #[serde(default)]
    pub reason: Option<String>,
    /// `StopFailure`'s `error_type` (#1060 sub-issue 1).
    ///
    /// This and the seven below are the event-specific fields #1061
    /// settles the shape of: flat, `#[serde(default)]`, absent meaning
    /// "this event does not carry it". They are ACCEPTED here before any
    /// event that writes them is installed, which is the point -- a reader
    /// that already understands the shape is what lets each of #1060's
    /// sub-issues add its event without a format change.
    ///
    /// Nothing in this module gives them meaning yet; `store` reads
    /// `source` and `reason` and no more. That belongs to the sub-issues.
    #[serde(default)]
    pub error_type: Option<String>,
    /// `PostToolUseFailure`'s `tool_name` (#1060 sub-issue 2).
    #[serde(default)]
    pub tool_name: Option<String>,
    /// `PostToolUseFailure`'s `error_message`, capped by the writer at
    /// [`super::hook::TEXT_FIELD_CAP`] bytes.
    #[serde(default)]
    pub error_message: Option<String>,
    /// `PermissionDenied`'s `denial_reason` (#1060 sub-issue 3).
    #[serde(default)]
    pub denial_reason: Option<String>,
    /// `PreCompact`/`PostCompact`'s `trigger`: `manual|auto` (#1060
    /// sub-issue 4).
    #[serde(default)]
    pub trigger: Option<String>,
    /// `SubagentStart`/`SubagentStop`'s `agent_id` (#1060 sub-issue 5).
    #[serde(default)]
    pub agent_id: Option<String>,
    /// `SubagentStart`/`SubagentStop`'s `agent_type`.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// `Notification`'s `notification_type` (#1060 sub-issue 6).
    #[serde(default)]
    pub notification_type: Option<String>,
}

impl Record {
    /// Whether this is an end-of-run record.
    ///
    /// Matched on `event` rather than on `reason` being present: the hook
    /// writes `reason` only when the payload carried it, so absence
    /// cannot distinguish the two events. That is the same reasoning the
    /// writer records for keeping `event` non-optional.
    fn is_end(&self) -> bool {
        self.event == "SessionEnd"
    }

    /// Whether this record is a POINT event rather than a run boundary.
    ///
    /// `SessionStart` and `SessionEnd` open and close a `claude_run` row.
    /// The events epic #1060 adds have no pid semantics and no duration,
    /// so they are stored in `claude_hook_event` and must not reach the
    /// run path -- a `Notification` inserted as a run would be a second
    /// run of a session that never restarted, and every liveness read
    /// downstream would then have an extra open run to reason about.
    ///
    /// # Why a NAMED list and not "anything that is not the two"
    ///
    /// Because "not a boundary" is also what an event we have never
    /// heard of looks like, and those two cases need opposite handling.
    /// `an_unrecognised_event_is_not_an_ending` pins the existing rule:
    /// the hook's literal `"unknown"` (a payload with no
    /// `hook_event_name`) still records a run, because a session that
    /// really did start must not be lost over one unreadable field.
    ///
    /// That rule survives here unchanged. Only the events Headstate
    /// INSTALLS and has given meaning to divert, so a newer hook writing
    /// an event this app has never seen keeps degrading the way it
    /// already did rather than being silently discarded by a negative
    /// match nobody revisited.
    fn point_event(&self) -> Option<&str> {
        match self.event.as_str() {
            e @ ("PreCompact" | "SubagentStart" | "Notification") => Some(e),
            _ => None,
        }
    }
}

/// Where the handoff file lives, given a home directory.
///
/// Must agree with the hook's `handoff_path_in` (#912). The home is a
/// PARAMETER rather than read from `$HOME` for `claudemd::expand_home_in`'s
/// reason: `$HOME` is process-global and a test that mutates it races
/// every other test in the binary.
pub fn path_in(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("headstate")
        .join("sessions.jsonl")
}

/// What one pass read, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Consumed {
    /// Records that reached the database.
    ///
    /// Named `runs` from when `SessionStart`/`SessionEnd` were the only
    /// events: since #1065/#1066/#1067 it also counts the point events
    /// that land in `claude_hook_event` rather than in `claude_run`. It
    /// is a diagnostic "how much did this pass store", not a count of
    /// processes -- `claude_run` is what answers that, and
    /// `overview::Counts::orphaned_runs` reads the table rather than
    /// this field.
    ///
    /// The name is kept rather than changed because this struct is
    /// serialised whole onto `ClaudeLiveState::handoff` by
    /// `claude_poll_live`, so a rename is a wire break -- for a field
    /// whose meaning only widened.
    pub runs: usize,
    /// Sessions inserted or touched.
    pub sessions: usize,
    /// Lines that were not valid JSON, or were JSON of the wrong shape.
    /// Counted so a UI can say "3 records could not be read", never
    /// silently skipped.
    pub unparseable: Vec<String>,
    /// Lines whose `v` this reader does not understand -- a NEWER hook
    /// than this app. A different problem from a malformed line, with a
    /// different remedy (reinstall the hooks, #915), so counted apart.
    pub unknown_version: usize,
    /// Records with no `session_id`, which cannot key a row. The session
    /// id is the whole identity, so there is nothing to store.
    pub without_session_id: usize,
    /// Records whose pid was the hook's `0` sentinel. Not stored as runs:
    /// `claude_run.pid` is NOT NULL so that an unobserved process cannot
    /// be recorded as an observed one, and `0` is precisely "we could not
    /// observe it".
    pub without_pid: usize,
    /// Records whose `session_id` had no transcript on disk -- the
    /// stale-payload case (upstream 9188). Stored anyway; see
    /// [`consume`] on why.
    pub unvalidated: usize,
    /// Rows the database refused, with why.
    pub write_failures: Vec<String>,
    /// The byte offset after this pass, which is what the next pass
    /// resumes from. Zero after a rotation.
    pub offset: u64,
    /// Whether the file was truncated at the end of this pass.
    pub rotated: bool,
}

impl Consumed {
    /// Whether anything could not be read or written.
    pub fn is_partial(&self) -> bool {
        !self.unparseable.is_empty()
            || !self.write_failures.is_empty()
            || self.unknown_version > 0
            || self.without_session_id > 0
            || self.without_pid > 0
    }
}

/// Where the last pass stopped.
///
/// A newtype rather than a bare `u64` so the shrink check cannot be
/// forgotten at a call site: the only way to get a new offset is
/// [`Offset::advance`], which performs it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Offset(pub u64);

/// What a pass should do with the file, given where we stopped and how
/// long it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Nothing new. The file is exactly as long as our offset.
    Unchanged,
    /// Read from this byte.
    From(u64),
    /// The file SHRANK. Read from zero -- see [`Offset::advance`].
    Restart,
}

impl Offset {
    /// Decide where to read from, given the file's current length.
    ///
    /// # Why a shorter file means starting over rather than stopping
    ///
    /// A file shorter than our offset was truncated or replaced. The
    /// stored offset is then meaningless: it points past the end, or --
    /// if the file has since been appended to -- into the MIDDLE of a
    /// record that is not the one it used to point after. Reading from
    /// there yields a partial line that parses as nothing, and the
    /// records before it are never read at all.
    ///
    /// So the answer is to re-read from zero. That is safe, and it is
    /// safe for a specific structural reason rather than by luck: every
    /// record is upserted on `(session_id, pid, started_at)`, so
    /// re-reading a record that is already stored writes the same row
    /// again. It is not merely tolerable to re-read, it is a no-op.
    ///
    /// The alternative -- treating a shorter file as an error and
    /// refusing to read -- would mean our own rotation broke consumption
    /// permanently, since rotation is exactly what makes the file
    /// shorter.
    ///
    /// EQUAL length is `Unchanged` rather than `From(len)`: they behave
    /// identically for a well-behaved file, but the distinction lets a
    /// caller skip the open entirely on the common case, and it makes
    /// "nothing happened" visible in a log rather than inferred from a
    /// zero-byte read.
    pub fn advance(self, len: u64) -> Resume {
        match len.cmp(&self.0) {
            std::cmp::Ordering::Equal => Resume::Unchanged,
            std::cmp::Ordering::Greater => Resume::From(self.0),
            std::cmp::Ordering::Less => Resume::Restart,
        }
    }
}

/// Whole lines from a buffer, and the bytes that were NOT a whole line.
///
/// Returns `(lines, consumed_bytes)`. `consumed_bytes` counts up to and
/// including the last newline, so a trailing partial line is left for the
/// next pass to re-read once its newline has arrived.
///
/// # Why a partial last line is left rather than parsed
///
/// A hook's record is one atomic `write`, so a line cannot be torn
/// between two writers -- but a READER can still arrive between the
/// moment a write begins and the moment it lands, and on a file being
/// appended to there is no promise the last byte is a newline. Parsing
/// what is there would produce a malformed record counted as a failure
/// for a record that was perfectly fine, and then the real record would
/// never be read because the offset had moved past it. Two bugs from one
/// assumption.
///
/// A line with no newline at all therefore consumes NOTHING. That is
/// deliberate even though it means a file whose final record never gets
/// its newline is re-read on every pass: re-reading is free and costs a
/// few hundred bytes, whereas consuming it would lose the record if the
/// write was genuinely incomplete.
fn whole_lines(buf: &str) -> (Vec<&str>, u64) {
    match buf.rfind('\n') {
        None => (Vec::new(), 0),
        Some(last) => {
            let complete = &buf[..=last];
            let lines = complete.lines().filter(|l| !l.trim().is_empty()).collect();
            (lines, complete.len() as u64)
        }
    }
}

/// Read the new bytes of the handoff file.
///
/// `Ok(None)` means the file does not exist, which is what a machine
/// with the hook not installed looks like -- not a failure. Any other
/// error IS an error: an unreadable file must not read as an empty one.
fn read_new(path: &Path, offset: Offset) -> Result<Option<(Vec<String>, u64, bool)>, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let len = meta.len();

    let (start, restarted) = match offset.advance(len) {
        Resume::Unchanged => return Ok(Some((Vec::new(), offset.0, false))),
        Resume::From(at) => (at, false),
        Resume::Restart => (0, true),
    };

    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(start))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut raw = Vec::new();
    f.read_to_end(&mut raw)
        .map_err(|e| format!("{}: {e}", path.display()))?;

    // Lossy rather than an error on invalid UTF-8. The hook writes
    // `serde_json` output, which is UTF-8 by construction, so a bad byte
    // here means the file was corrupted by something else -- and in that
    // case the honest outcome is that the affected LINE fails to parse
    // and is counted, rather than the whole pass failing and the other
    // records being lost with it.
    let text = String::from_utf8_lossy(&raw);
    let (lines, used) = whole_lines(&text);
    let lines = lines.into_iter().map(str::to_owned).collect();
    Ok(Some((lines, start + used, restarted)))
}

/// Whether a session id has a transcript on disk, if we can tell.
///
/// `None` means we could not check -- no home directory, or the projects
/// directory could not be read -- which is different from checking and
/// finding nothing.
///
/// # Why this is checked at all (upstream 9188)
///
/// After `/exit` then `--continue`, a hook can receive a `session_id`
/// belonging to the PREVIOUS session. The hook records what it was told
/// and cannot afford to look, because looking is disk work inside a 1.5
/// second budget. This side can.
///
/// # Why a miss does not reject the record
///
/// Because the transcript is written by Claude Code on its own schedule,
/// and a `SessionStart` hook fires BEFORE the first transcript record
/// exists. Rejecting an unvalidated record would therefore throw away
/// every correctly-reported new session -- the common case -- to catch a
/// rare stale one. So the miss is counted in [`Consumed::unvalidated`]
/// and the record is stored; the transcript importer (#914) is what
/// corrects a stale `cwd` later, since it re-reads disk and wins on that
/// field by design.
///
/// # What the directory walk costs, measured
///
/// It probes `<project dir>/<session_id>.jsonl` in each project
/// directory, so the worst case is one `stat` per directory. On the real
/// corpus -- **670 project directories** on the development machine --
/// that is **3.0 ms** for an id that exists nowhere (every directory
/// probed) and 2.2 ms for one that is found, and the caller memoises it
/// per DISTINCT session id, so a start and an end record cost one walk
/// between them.
///
/// Cheap enough that no index is worth keeping, and the reason it is
/// cheap is that nothing is opened or read: the slug directory names are
/// derived from `cwd`, so the right one cannot be computed from the
/// session id alone, but a `stat` per directory is still orders of
/// magnitude under the poll interval this runs on.
fn transcript_exists(session_id: &str) -> Option<bool> {
    let projects = super::transcript::projects_dir()?;
    let dirs = std::fs::read_dir(&projects).ok()?;
    let wanted = format!("{session_id}.jsonl");
    for d in dirs.flatten() {
        if d.path().join(&wanted).exists() {
            return Some(true);
        }
    }
    Some(false)
}

/// Write one record's run, and the session row it belongs to.
///
/// # What this must NOT overwrite
///
/// The transcript importer (#914) owns `cwd`, `git_branch`,
/// `claude_version` and `transcript_path`, because disk is ground truth
/// and a hook-supplied `cwd` can be the previous session's (upstream
/// 9188). So the session upsert here only ever FILLS a NULL -- it takes
/// the existing value first in every `COALESCE`. The one exception is
/// the time range, which widens in both directions.
///
/// That is the mirror image of `store::import`'s rule, and the pairing is
/// the point: the importer overwrites `cwd` from disk, this side never
/// overwrites it at all, so a stale hook value cannot survive a rescan
/// and cannot displace a good one in the meantime.
fn write_record(conn: &Connection, rec: &Record, start_time: Option<i64>) -> Result<(), String> {
    let Some(session_id) = rec.session_id.as_deref() else {
        return Ok(());
    };
    // The hook's timestamp is when the hook ran; without one there is
    // nothing to record a run AT, and `started_at` is part of the primary
    // key. `first_seen_at` is NOT NULL for the same reason.
    let ts = rec
        .ts
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    conn.execute(
        // Existing value FIRST in every COALESCE: the importer owns these
        // fields and a hook must never displace a value read from disk.
        "INSERT INTO claude_session
            (session_id, name, cwd, first_seen_at, last_activity_at)
         VALUES (?1, NULL, ?2, ?3, ?3)
         ON CONFLICT(session_id) DO UPDATE SET
            cwd              = COALESCE(claude_session.cwd, excluded.cwd),
            first_seen_at    = MIN(claude_session.first_seen_at, excluded.first_seen_at),
            last_activity_at = MAX(
                COALESCE(claude_session.last_activity_at, excluded.last_activity_at),
                COALESCE(excluded.last_activity_at, claude_session.last_activity_at)
            )",
        rusqlite::params![session_id, rec.cwd, ts],
    )
    .map_err(|e| format!("{session_id}: could not store the session: {e}"))?;

    // A point event (#1065/#1066/#1067) before any run handling, because
    // it is not a run and must not fall through into one.
    //
    // The session upsert above still happened, and deliberately: a
    // `Notification` for a session we have no row for is still evidence
    // that the session exists, and its `last_activity_at` widening is
    // what makes #1067's staleness rule work -- see `signals::Waiting`.
    if let Some(event) = rec.point_event() {
        conn.execute(
            // OR IGNORE against (session_id, event, at) for the reason
            // the start-record insert below uses it: `consume` can
            // re-read records after a rotation-then-crash, and a
            // re-read must be a no-op rather than a doubled count.
            "INSERT OR IGNORE INTO claude_hook_event
                (session_id, event, at, trigger_kind, agent_id, agent_type,
                 notification_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                session_id,
                event,
                ts,
                rec.trigger,
                rec.agent_id,
                // An EMPTY agent_type is stored as NULL, not as "". The
                // hook passes the payload through verbatim and
                // `SubagentStop` writes `agent_type: b ?? ""` -- so an
                // empty string is Claude Code's way of saying it did not
                // have one. Storing it as a value would render a blank
                // chip that looks like a real agent type; NULL reaches
                // the "not recorded" arm that already exists.
                rec.agent_type.as_deref().filter(|t| !t.is_empty()),
                rec.notification_type,
            ],
        )
        .map_err(|e| format!("{session_id}: could not store the {event}: {e}"))?;
        return Ok(());
    }

    // `pid_start_time` is written as RFC 3339 to match every other
    // timestamp in the schema, and is NULL when the registry could not
    // confirm it -- migration 11's "cannot confirm", which the liveness
    // layer reports as Unknown rather than Running.
    let pid_start = start_time
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|d| d.to_rfc3339());

    if rec.is_end() {
        // An end record closes the run the START record opened, which is
        // keyed on a `started_at` this record does not carry. So it is an
        // UPDATE of the newest open run for this (session, pid) rather
        // than an insert.
        //
        // `ended_at IS NULL` in the predicate so a second end record for
        // the same run does not overwrite the first end time: the first
        // one is when the session actually ended, and a duplicate
        // delivery must not move it later.
        let touched = conn
            .execute(
                "UPDATE claude_run
                    SET ended_at = ?1, end_reason = ?2,
                        pid_start_time = COALESCE(pid_start_time, ?3)
                  WHERE session_id = ?4 AND pid = ?5 AND ended_at IS NULL
                    AND started_at = (
                        SELECT MAX(started_at) FROM claude_run
                         WHERE session_id = ?4 AND pid = ?5 AND ended_at IS NULL
                    )",
                rusqlite::params![ts, rec.reason, pid_start, session_id, rec.ppid],
            )
            .map_err(|e| format!("{session_id}: could not close the run: {e}"))?;
        if touched == 0 {
            // Nothing OPEN to close. Two very different situations reach
            // here and they must not be conflated:
            //
            //   a) this exact end record has already been applied -- a
            //      REDELIVERY, which a rotation-then-crash produces on the
            //      next pass and which `Offset::advance`'s restart relies
            //      on being a no-op;
            //   b) the start record was never seen at all, because the
            //      hook was installed mid-session.
            //
            // Distinguished by asking whether a run for this
            // (session, pid) already carries this end time. Without the
            // check, (a) falls through to the insert below and lands a
            // SECOND row keyed on the end record's own timestamp -- so a
            // re-read after a rotation would double the run history every
            // time, which is exactly the corruption the restart path was
            // argued to be free of. Found by the re-read test, not by
            // foresight.
            let already: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM claude_run
                         WHERE session_id = ?1 AND pid = ?2 AND ended_at = ?3
                     )",
                    rusqlite::params![session_id, rec.ppid, ts],
                    |r| r.get(0),
                )
                .map_err(|e| format!("{session_id}: could not check for the run: {e}"))?;
            if already {
                return Ok(());
            }
            // (b): the session DID end, and that is the fact #921's
            // overview counts, so it is recorded rather than dropped.
            // `started_at = ended_at` is honest about the start being
            // unobserved rather than inventing an earlier one.
            conn.execute(
                "INSERT OR IGNORE INTO claude_run
                    (session_id, pid, pid_start_time, source, end_reason,
                     started_at, ended_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?5)",
                rusqlite::params![session_id, rec.ppid, pid_start, rec.reason, ts],
            )
            .map_err(|e| format!("{session_id}: could not store the end-only run: {e}"))?;
        }
        return Ok(());
    }

    // A start record. `INSERT OR IGNORE` rather than a plain insert so
    // re-reading after a rotation is the no-op that
    // `Offset::advance` relies on: the primary key is
    // (session_id, pid, started_at), all three of which come off the
    // record, so the same record always addresses the same row.
    //
    // OR IGNORE rather than an upsert because there is nothing to merge:
    // a re-read of a start record carries exactly the values already
    // stored, and an upsert would only risk clearing an `ended_at` the
    // matching end record had since written.
    conn.execute(
        "INSERT OR IGNORE INTO claude_run
            (session_id, pid, pid_start_time, source, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![session_id, rec.ppid, pid_start, rec.source, ts],
    )
    .map_err(|e| format!("{session_id}: could not store the run: {e}"))?;
    Ok(())
}

/// Parse one line into a record we are willing to store.
///
/// The version check comes first and is done on a `Value` rather than by
/// deserialising into [`Record`], because a NEWER record may have a shape
/// this reader's struct cannot deserialise at all -- and then the failure
/// would be counted as "malformed" when the truth is "newer than me".
/// Those have different remedies, so they must not collapse into one
/// count.
fn parse_line(line: &str) -> Result<Record, LineProblem> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| LineProblem::Malformed(e.to_string()))?;
    match v.get("v").and_then(serde_json::Value::as_u64) {
        Some(n) if n == u64::from(RECORD_VERSION) => {}
        Some(_) => return Err(LineProblem::UnknownVersion),
        None => return Err(LineProblem::Malformed("it has no `v` field".into())),
    }
    serde_json::from_value(v).map_err(|e| LineProblem::Malformed(e.to_string()))
}

/// Why a line could not become a record.
enum LineProblem {
    Malformed(String),
    UnknownVersion,
}

/// Consume the handoff file into the database.
///
/// `registry` supplies each pid's confirmed start time, so
/// `pid_start_time` is resolved HERE rather than by the hook: the hook
/// would have to `sysctl` its own parent inside a 1.5 second budget for a
/// value this side gets from a file read it is doing anyway.
///
/// A pid the registry does not know leaves `pid_start_time` NULL --
/// migration 11's "cannot confirm", which liveness must report as Unknown
/// and never as Running. That happens routinely and is not a failure: a
/// session that has already ended has no registry file, so its runs are
/// recorded without a start time, and the run's own `ended_at` is what
/// says it is over.
///
/// # Ordering
///
/// Read, write in one transaction, commit, THEN truncate. A crash between
/// commit and truncation re-reads stored records on the next pass, which
/// the upserts make a no-op. The reverse order would lose the window.
pub fn consume(
    conn: &mut Connection,
    path: &Path,
    offset: Offset,
    registry: &std::collections::HashMap<u32, i64>,
) -> Result<Consumed, String> {
    let Some((lines, new_offset, restarted)) = read_new(path, offset)? else {
        // No file: the hook is not installed, or has not fired yet. Not a
        // failure, and the offset stays where it was so an installation
        // later does not look like a rotation.
        return Ok(Consumed {
            offset: offset.0,
            ..Default::default()
        });
    };

    let mut out = Consumed {
        offset: new_offset,
        ..Default::default()
    };
    if restarted {
        log::info!(
            "claude handoff: {} is shorter than our offset ({} bytes), \
             so it was truncated or replaced -- re-reading from the start",
            path.display(),
            offset.0
        );
    }
    if lines.is_empty() {
        return Ok(out);
    }

    let mut records = Vec::with_capacity(lines.len());
    for line in &lines {
        match parse_line(line) {
            Ok(rec) => records.push(rec),
            Err(LineProblem::UnknownVersion) => out.unknown_version += 1,
            Err(LineProblem::Malformed(why)) => {
                out.unparseable.push(format!("{}: {why}", path.display()))
            }
        }
    }

    // Validated once per distinct session id rather than per record, so a
    // session with a start and an end record costs one directory walk
    // rather than two.
    let mut checked: std::collections::HashMap<&str, Option<bool>> =
        std::collections::HashMap::new();

    let tx = conn
        .transaction()
        .map_err(|e| format!("could not begin a transaction: {e}"))?;
    let mut sessions = std::collections::HashSet::new();
    for rec in &records {
        let Some(sid) = rec.session_id.as_deref() else {
            // No id, no identity, no row. Counted rather than dropped
            // silently: it means a hook fired for a session we cannot
            // name, which is worth someone seeing.
            out.without_session_id += 1;
            continue;
        };
        if rec.ppid == 0 {
            // The hook's "could not tell" sentinel. `claude_run.pid` is
            // NOT NULL precisely so an unobserved process cannot be
            // stored as an observed one, and 0 is not a pid on any
            // platform -- storing it would put a row in the table whose
            // liveness check would ask the OS about process zero.
            out.without_pid += 1;
            continue;
        }
        let seen = *checked.entry(sid).or_insert_with(|| transcript_exists(sid));
        if seen == Some(false) {
            // Upstream 9188: the payload may carry the PREVIOUS session's
            // id after `/exit` then `--continue`. Counted, and stored
            // anyway -- see `transcript_exists` on why rejecting would
            // throw away every correctly-reported new session.
            out.unvalidated += 1;
        }
        match write_record(&tx, rec, registry.get(&rec.ppid).copied()) {
            Ok(()) => {
                out.runs += 1;
                sessions.insert(sid.to_owned());
            }
            Err(e) => out.write_failures.push(e),
        }
    }
    out.sessions = sessions.len();
    tx.commit()
        .map_err(|e| format!("could not commit the handoff records: {e}"))?;

    // AFTER the commit. See the module docs: the reverse order loses the
    // window on a crash, this order re-reads a stored record and the
    // upserts make that a no-op.
    //
    // Truncated rather than deleted so the hook's own `create_dir_all`
    // and append keep working without a race against our unlink, and so
    // the offset reset is the one already-handled `Restart` case rather
    // than a new one.
    if out.write_failures.is_empty() && !records.is_empty() {
        match std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
        {
            Ok(_) => {
                out.rotated = true;
                out.offset = 0;
            }
            Err(e) => {
                // Not a failure of the pass: the records are committed.
                // The file keeps growing and the offset keeps working, so
                // the only cost is disk. Said out loud rather than
                // swallowed.
                log::warn!(
                    "claude handoff: stored {} records but could not truncate {}: {e}",
                    out.runs,
                    path.display()
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn start(sid: &str, pid: u32, ts: &str) -> String {
        format!(
            r#"{{"v":1,"event":"SessionStart","session_id":"{sid}","ppid":{pid},"ts":"{ts}","cwd":"/Users/acme/code/widget","source":"startup"}}"#
        )
    }

    fn end(sid: &str, pid: u32, ts: &str) -> String {
        format!(
            r#"{{"v":1,"event":"SessionEnd","session_id":"{sid}","ppid":{pid},"ts":"{ts}","cwd":"/Users/acme/code/widget","reason":"other"}}"#
        )
    }

    fn write_file(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut body = String::new();
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        std::fs::write(path, body).unwrap();
    }

    fn no_registry() -> std::collections::HashMap<u32, i64> {
        std::collections::HashMap::new()
    }

    /// The path agrees with the hook's (#912).
    #[test]
    fn the_path_is_the_one_the_hook_appends_to() {
        let p = path_in(Path::new("/Users/acme"));
        assert_eq!(
            p,
            Path::new("/Users/acme/.claude/headstate/sessions.jsonl"),
            "if this drifts from the hook's own path, the consumer reads \
             an empty directory forever and reports no sessions"
        );
    }

    // ---- offsets, shrink detection, partial lines ----

    /// The three answers, and the boundary between them.
    #[test]
    fn an_offset_decides_where_to_read_from() {
        assert_eq!(Offset(100).advance(100), Resume::Unchanged);
        assert_eq!(Offset(100).advance(250), Resume::From(100));
        assert_eq!(Offset(100).advance(50), Resume::Restart);
        assert_eq!(Offset(100).advance(0), Resume::Restart);
        assert_eq!(Offset(0).advance(0), Resume::Unchanged);
        assert_eq!(Offset(0).advance(1), Resume::From(0));
    }

    /// SHRINK DETECTION against a real file.
    ///
    /// The offset after a first pass points past the end of a truncated
    /// file. Reading from there yields nothing, so the records written
    /// after the truncation would never be seen.
    ///
    /// Sabotage: replacing the `Ordering::Less` arm with
    /// `Resume::From(self.0)` makes this read 0 runs instead of 1, and
    /// `a_rotation_resets_the_offset` fails too.
    #[test]
    fn a_shorter_file_is_re_read_from_the_start() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);
        let long = std::fs::metadata(&p).unwrap().len();

        // Someone clears the file and a new session starts. The new file
        // is SHORTER than our offset, because the old one had a longer
        // line in it.
        write_file(&p, &[r#"{"v":1,"event":"SessionStart","session_id":"s2","ppid":7,"ts":"2026-09-13T11:00:00Z"}"#.to_string()]);
        let short = std::fs::metadata(&p).unwrap().len();
        assert!(
            short < long,
            "the fixture must really shrink: {short} < {long}"
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(long), &no_registry()).unwrap();
        assert_eq!(
            got.runs, 1,
            "a truncated file must be re-read from zero, or every record \
             written after the truncation is lost forever"
        );
        let sid: String = conn
            .query_row("SELECT session_id FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sid, "s2");
    }

    /// A PARTIAL last line is left for the next pass.
    ///
    /// A hook's write is atomic, but a reader can still land between two
    /// appends. Consuming a line without its newline would count a good
    /// record as malformed AND move the offset past it, so the real
    /// record would never be read. Two bugs from one assumption.
    ///
    /// Sabotage: using `buf.lines()` over the whole buffer and consuming
    /// `buf.len()` makes this report 1 unparseable and then MISS the
    /// record entirely on the second pass.
    #[test]
    fn a_line_without_its_newline_waits_for_the_next_pass() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        let whole = start("s1", 100, "2026-09-13T10:00:00Z");
        let partial = start("s2", 200, "2026-09-13T10:00:01Z");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // The second line is mid-write: no trailing newline.
        std::fs::write(&p, format!("{whole}\n{}", &partial[..40])).unwrap();

        let mut conn = db();
        let first = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(first.runs, 1, "only the complete line");
        assert!(
            first.unparseable.is_empty(),
            "a half-written line is not a malformed one: {:?}",
            first.unparseable
        );
        // Rotation truncated the file after committing the one record, so
        // the half line is gone -- which is the honest outcome, because
        // the hook will rewrite it. Assert the more important half: the
        // partial line was never counted as a record.
        assert!(first.rotated);

        // Now the same thing WITHOUT rotation in play: a file whose last
        // line is partial and whose complete records were already
        // consumed must consume nothing and keep its offset.
        std::fs::write(&p, format!("{whole}\n{}", &partial[..40])).unwrap();
        let at = (whole.len() + 1) as u64;
        let second = consume(&mut conn, &p, Offset(at), &no_registry()).unwrap();
        assert_eq!(second.runs, 0);
        assert_eq!(
            second.offset, at,
            "the offset must NOT advance past a line whose newline has \
             not arrived"
        );
    }

    /// `whole_lines` directly: nothing is consumed without a newline.
    #[test]
    fn a_buffer_with_no_newline_consumes_nothing() {
        let (lines, used) = whole_lines(r#"{"v":1,"event":"Sessio"#);
        assert!(lines.is_empty());
        assert_eq!(used, 0, "not one byte, or the record is lost");
    }

    /// Blank lines are not records and are not failures.
    #[test]
    fn blank_lines_are_neither_records_nor_failures() {
        let (lines, used) = whole_lines("a\n\n\nb\n");
        assert_eq!(lines, vec!["a", "b"]);
        // All six bytes, blanks included: the blank lines ARE consumed,
        // they are just not records. Leaving them would re-read them on
        // every pass forever.
        assert_eq!(used, 6);
    }

    /// ROTATION happens after the commit, and resets the offset.
    #[test]
    fn a_rotation_resets_the_offset() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.runs, 1);
        assert!(got.rotated);
        assert_eq!(got.offset, 0, "a truncated file is read from zero");
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            0,
            "the file is truncated, not deleted -- the hook's append must \
             keep working without racing an unlink"
        );
        // The records are still in the database: truncation came after
        // the commit.
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 1);
    }

    /// A pass that could not WRITE does not truncate.
    ///
    /// This is the ordering rule from the other side: truncating after a
    /// failed write would discard records that never reached the
    /// database. Provoked by a real write failure -- a run row whose
    /// session row cannot exist because the session insert is sabotaged
    /// by a conflicting NOT NULL -- rather than by mocking.
    #[test]
    fn a_pass_that_could_not_write_keeps_the_file() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        // A trigger that refuses every run insert, standing in for a
        // disk-full or locked database. The records must survive in the
        // file so the next pass can retry them.
        conn.execute_batch(
            "CREATE TRIGGER refuse_runs BEFORE INSERT ON claude_run
             BEGIN SELECT RAISE(ABORT, 'no runs today'); END",
        )
        .unwrap();

        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.runs, 0);
        assert_eq!(got.write_failures.len(), 1);
        assert!(got.is_partial());
        assert!(!got.rotated);
        assert!(
            std::fs::metadata(&p).unwrap().len() > 0,
            "records that did not reach the database must stay in the \
             file, or a transient write failure loses them permanently"
        );
    }

    /// Re-reading an already-consumed record changes nothing.
    ///
    /// This is the property `Offset::advance`'s restart relies on. If a
    /// re-read doubled the runs, a rotation would corrupt the history
    /// every time it happened.
    #[test]
    fn re_reading_the_same_records_is_a_no_op() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        let lines = vec![
            start("s1", 100, "2026-09-13T10:00:00Z"),
            end("s1", 100, "2026-09-13T10:05:00Z"),
        ];
        write_file(&p, &lines);

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        // Same file again, from zero -- what a rotation-then-crash looks
        // like on the next pass.
        write_file(&p, &lines);
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 1, "one session, one pid, one started_at, one row");
        let ended: Option<String> = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            ended.as_deref(),
            Some("2026-09-13T10:05:00Z"),
            "and the end time is not cleared by re-reading the start"
        );
    }

    // ---- pid_start_time ----

    /// `pid_start_time` comes from the registry, resolved HERE.
    #[test]
    fn the_registry_supplies_the_start_time() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 14779, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        let reg = std::collections::HashMap::from([(14779u32, 1_789_119_828i64)]);
        consume(&mut conn, &p, Offset(0), &reg).unwrap();

        let got: Option<String> = conn
            .query_row("SELECT pid_start_time FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("2026-09-11T09:43:48+00:00"),
            "the registry's UTC procStart, stored as RFC 3339 like every \
             other timestamp in the schema"
        );
    }

    /// A pid the registry does not know leaves `pid_start_time` NULL.
    ///
    /// Migration 11: NULL means "cannot confirm", which liveness reports
    /// as Unknown and never as Running. A zero or a guess here would make
    /// a crashed session look alive and hide its Resume button.
    ///
    /// Sabotage: defaulting to `chrono::Utc::now()` makes this fail with
    /// a present timestamp, and the value would be a lie about when the
    /// process started.
    #[test]
    fn an_unknown_pid_leaves_the_start_time_null() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let got: Option<String> = conn
            .query_row("SELECT pid_start_time FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            got, None,
            "cannot-confirm is NULL, not a substituted time -- absent is \
             not zero"
        );
    }

    // ---- absent is not zero ----

    /// A malformed line is COUNTED and the good ones still store.
    #[test]
    fn a_malformed_line_is_counted_and_the_rest_still_store() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                start("s1", 100, "2026-09-13T10:00:00Z"),
                "{ not json at all".to_string(),
                start("s2", 200, "2026-09-13T10:00:01Z"),
            ],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.runs, 2);
        assert_eq!(got.unparseable.len(), 1);
        assert!(got.is_partial());
    }

    /// A NEWER record version is counted apart from a malformed one.
    ///
    /// Different problem, different remedy: the hook on disk is newer
    /// than this reader, which happens because an installed hook command
    /// line keeps running whatever binary is at that path across an app
    /// upgrade. The remedy is reinstalling the hooks (#915), not fixing
    /// a corrupt file.
    ///
    /// Sabotage: deserialising into `Record` before checking `v` folds
    /// this into `unparseable`, because the newer record's extra
    /// non-optional field fails the struct.
    #[test]
    fn a_newer_record_version_is_its_own_count() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                // A v2 record with a field shape this reader's struct
                // cannot deserialise.
                r#"{"v":2,"event":{"kind":"SessionStart"},"ppid":100}"#.to_string(),
                start("s1", 100, "2026-09-13T10:00:00Z"),
            ],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.unknown_version, 1);
        assert!(
            got.unparseable.is_empty(),
            "a newer hook is not a corrupt file: {:?}",
            got.unparseable
        );
        assert_eq!(got.runs, 1, "and the records we DO understand still store");
    }

    /// A line with no `v` at all is malformed, not "version 0".
    #[test]
    fn a_record_with_no_version_is_malformed() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[r#"{"event":"SessionStart","session_id":"s1","ppid":100}"#.to_string()],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.unparseable.len(), 1);
        assert_eq!(got.unknown_version, 0);
        assert_eq!(got.runs, 0);
    }

    /// A record with no `session_id` cannot key a row, and is counted.
    #[test]
    fn a_record_with_no_session_id_is_counted() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                r#"{"v":1,"event":"SessionStart","ppid":100,"ts":"2026-09-13T10:00:00Z"}"#
                    .to_string(),
            ],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.without_session_id, 1);
        assert_eq!(got.runs, 0);
        assert!(got.is_partial());
    }

    /// The hook's `ppid: 0` sentinel is NOT stored as a pid.
    ///
    /// `claude_run.pid` is NOT NULL so an unobserved process cannot be
    /// recorded as an observed one, and 0 is not a valid pid on any
    /// platform. Storing it would put a row in the table whose liveness
    /// check asks the OS about process zero.
    ///
    /// Sabotage: dropping the `rec.ppid == 0` guard stores the row, and
    /// this fails with 1 run.
    #[test]
    fn the_cannot_say_pid_sentinel_is_not_stored_as_a_pid() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[r#"{"v":1,"event":"SessionStart","session_id":"s1","ppid":0,"ts":"2026-09-13T10:00:00Z"}"#.to_string()],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.without_pid, 1);
        assert_eq!(got.runs, 0, "0 is `we could not tell`, not a process");
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    /// An ABSENT handoff file is not an error and not a rotation.
    ///
    /// It is what a machine with the hook not installed looks like. The
    /// offset must stay where it was, so installing the hook later does
    /// not look like a truncation.
    #[test]
    fn an_absent_file_is_nothing_to_read_rather_than_a_failure() {
        let t = tempfile::TempDir::new().unwrap();
        let mut conn = db();
        let got = consume(&mut conn, &path_in(t.path()), Offset(42), &no_registry()).unwrap();
        assert_eq!(got.runs, 0);
        assert!(!got.is_partial());
        assert_eq!(got.offset, 42, "the offset is preserved, not reset");
        assert!(!got.rotated);
    }

    /// An UNREADABLE file is an error, not an empty list.
    ///
    /// `caches/mod.rs:550`: something we could not read is not evidence
    /// of absence. A `0600`-owned-by-root handoff file and a machine with
    /// no sessions have opposite remedies.
    ///
    /// Sabotage: returning `Ok(None)` for every `Err` from `metadata`
    /// makes this pass as "nothing to read" and the failure becomes
    /// invisible.
    ///
    /// `#[cfg(unix)]` because the only honest way to make a file
    /// unreadable-but-present is a mode, and Windows has no equivalent we
    /// can set from a test. An earlier version used a DIRECTORY at the
    /// file's path as a proxy and failed on Windows for a reason worth
    /// recording: a directory's `metadata.len()` is 0 there, so
    /// `Offset::advance` returns `Resume::Unchanged` and `read_new`
    /// returns `Ok` before it ever calls `File::open`. The production
    /// code was right -- that early return is load-bearing for the common
    /// case of a file that has not grown -- and the test was encoding one
    /// platform's `len()` as universal. A real file with real bytes
    /// reaches `File::open` on every platform, so what is gated here is
    /// only the ability to set the mode. Do not "tidy" the gate away.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        // A real file, with real records in it, that we then make
        // unreadable -- the `0600`-owned-by-root case from the doc
        // comment above. Non-empty so `advance` sees growth and the read
        // path is actually entered, rather than the NotFound arm, which
        // is a different guarantee.
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry());

        // Restore before any assertion can panic and leak a file the
        // temp dir cannot clean up.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        let err = got.expect_err("an unreadable file is not an empty read");
        assert!(
            err.contains("sessions.jsonl"),
            "the error must name the path: {err}"
        );
    }

    /// An UNCHANGED file reads nothing and keeps its offset.
    #[test]
    fn an_unchanged_file_reads_nothing() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);
        let len = std::fs::metadata(&p).unwrap().len();

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(len), &no_registry()).unwrap();
        assert_eq!(got.runs, 0);
        assert_eq!(got.offset, len);
        assert!(
            !got.rotated,
            "nothing was read, so there is nothing to rotate"
        );
    }

    // ---- runs and end records ----

    /// A start and an end become ONE run with both times.
    #[test]
    fn a_start_and_an_end_are_one_run() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                start("s1", 100, "2026-09-13T10:00:00Z"),
                end("s1", 100, "2026-09-13T10:05:00Z"),
            ],
        );

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let (started, ended, source, reason): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT started_at, ended_at, source, end_reason FROM claude_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(started, "2026-09-13T10:00:00Z");
        assert_eq!(ended.as_deref(), Some("2026-09-13T10:05:00Z"));
        assert_eq!(source.as_deref(), Some("startup"));
        assert_eq!(reason.as_deref(), Some("other"));
    }

    /// A resume is a SECOND run of the SAME session.
    ///
    /// Migration 11's whole reason for two tables: `session_id` survives a
    /// resume and the pid does not, so the history of revivals lives in
    /// the runs.
    #[test]
    fn a_resume_is_a_second_run_of_the_same_session() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                start("s1", 100, "2026-09-13T10:00:00Z"),
                end("s1", 100, "2026-09-13T10:05:00Z"),
                // Same id, new pid: `claude --resume`.
                start("s1", 200, "2026-09-13T11:00:00Z"),
            ],
        );

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 2);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1, "one session, two runs");
    }

    /// An END with no matching start still records that the session
    /// ended.
    ///
    /// The hook installed mid-session, or the handoff file cleared
    /// between the two records. Dropping it would lose the one fact the
    /// overview counts -- that the session finished cleanly -- and would
    /// leave the row looking crashed.
    #[test]
    fn an_end_with_no_start_still_records_the_ending() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[end("s1", 100, "2026-09-13T10:05:00Z")]);

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.runs, 1);

        let (started, ended, reason): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT started_at, ended_at, end_reason FROM claude_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            started, "2026-09-13T10:05:00Z",
            "the start was never observed, so it is not invented earlier"
        );
        assert_eq!(ended.as_deref(), Some("2026-09-13T10:05:00Z"));
        assert_eq!(reason.as_deref(), Some("other"));
    }

    /// A DUPLICATE end record does not move the end time later.
    ///
    /// The first end is when the session actually ended. A redelivery --
    /// from a rotation crash, or a hook that fired twice -- must not
    /// rewrite it.
    ///
    /// Sabotage: dropping `AND ended_at IS NULL` from the UPDATE lets the
    /// second record overwrite the first, and this fails.
    #[test]
    fn a_duplicate_end_does_not_move_the_end_time() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                start("s1", 100, "2026-09-13T10:00:00Z"),
                end("s1", 100, "2026-09-13T10:05:00Z"),
                end("s1", 100, "2026-09-13T23:59:00Z"),
            ],
        );

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let ends: Vec<Option<String>> = conn
            .prepare("SELECT ended_at FROM claude_run ORDER BY started_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            ends.contains(&Some("2026-09-13T10:05:00Z".to_string())),
            "the FIRST end time is the real one: {ends:?}"
        );
    }

    /// The session row is created, and the hook NEVER overwrites a cwd
    /// the transcript importer wrote.
    ///
    /// The mirror image of `store::import`'s rule. The importer wins on
    /// `cwd` because disk is ground truth and a hook payload can carry the
    /// PREVIOUS session's directory (upstream 9188); this side therefore
    /// only ever fills a NULL.
    ///
    /// Sabotage: reversing the COALESCE to `excluded.cwd` first makes the
    /// hook's stale value displace the good one, and this fails.
    #[test]
    fn the_hook_never_overwrites_a_cwd_read_from_disk() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        // What the importer stored, from disk.
        conn.execute(
            "INSERT INTO claude_session (session_id, cwd, first_seen_at)
             VALUES ('s1', '/Users/acme/code/the-real-one', '2026-09-01T00:00:00Z')",
            [],
        )
        .unwrap();

        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let cwd: Option<String> = conn
            .query_row(
                "SELECT cwd FROM claude_session WHERE session_id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cwd.as_deref(),
            Some("/Users/acme/code/the-real-one"),
            "disk wins on cwd; a hook payload can be the previous \
             session's (upstream 9188)"
        );
    }

    /// A hook-only session still gets a row, with the cwd it reported.
    #[test]
    fn a_hook_only_session_fills_a_null_cwd() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let cwd: Option<String> = conn
            .query_row("SELECT cwd FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cwd.as_deref(), Some("/Users/acme/code/widget"));
    }

    /// The session's time range widens and never narrows.
    #[test]
    fn the_session_range_widens_and_never_narrows() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[start("s1", 100, "2026-09-13T10:00:00Z")]);

        let mut conn = db();
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at, last_activity_at)
             VALUES ('s1', '2026-01-01T00:00:00Z', '2026-12-01T00:00:00Z')",
            [],
        )
        .unwrap();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let (first, last): (String, Option<String>) = conn
            .query_row(
                "SELECT first_seen_at, last_activity_at FROM claude_session",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(first, "2026-01-01T00:00:00Z", "earliest wins");
        assert_eq!(last.as_deref(), Some("2026-12-01T00:00:00Z"), "latest wins");
    }

    /// Session counting is DISTINCT sessions, not records.
    #[test]
    fn the_session_count_is_distinct_sessions() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[
                start("s1", 100, "2026-09-13T10:00:00Z"),
                end("s1", 100, "2026-09-13T10:05:00Z"),
                start("s2", 200, "2026-09-13T10:06:00Z"),
            ],
        );

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();
        assert_eq!(got.runs, 3, "three records reached the database");
        assert_eq!(got.sessions, 2, "two distinct sessions");
    }

    /// An `event` we do not recognise is treated as a start, not an end.
    ///
    /// The hook writes the literal `"unknown"` when the payload had no
    /// `hook_event_name`. Treating that as an END would mark a session
    /// that is still running as finished, and hide the fact that it is
    /// alive -- the fail-open direction. Treating it as a start records a
    /// run with no end, which derived liveness answers correctly from the
    /// pid.
    #[test]
    fn an_unrecognised_event_is_not_an_ending() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(
            &p,
            &[r#"{"v":1,"event":"unknown","session_id":"s1","ppid":100,"ts":"2026-09-13T10:00:00Z"}"#.to_string()],
        );

        let mut conn = db();
        consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        let ended: Option<String> = conn
            .query_row("SELECT ended_at FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            ended, None,
            "an event we cannot name must not be read as an ending -- \
             that would mark a live session finished"
        );
    }

    // -----------------------------------------------------------------
    // #1061: the version rule, proven rather than asserted.
    // -----------------------------------------------------------------

    /// [`Record`] exactly as it was BEFORE #1061 added the event fields.
    ///
    /// A frozen copy, not a reference to the live type -- that is the
    /// whole point. An assertion about "an old reader" written against
    /// today's struct tests nothing, because today's struct has the new
    /// fields and would go on having them as more are added. This one
    /// cannot: it is a snapshot of the shape a Headstate shipped before
    /// this change has compiled into it, and it stays that shape forever.
    ///
    /// It carries the same `#[serde(default)]` attributes and, crucially,
    /// the same ABSENCE of `#[serde(deny_unknown_fields)]` -- which is the
    /// property the test below is really about.
    #[derive(Debug, serde::Deserialize)]
    struct OldRecord {
        v: u32,
        event: String,
        #[serde(default)]
        session_id: Option<String>,
        ppid: u32,
        #[serde(default)]
        ts: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    }

    /// One line as a NEW hook writes it: every event-specific field #1061
    /// adds, present at once, at `v: 1`.
    ///
    /// Produced by the real writer rather than hand-written, so it cannot
    /// drift from what a hook actually emits.
    fn a_new_format_line() -> String {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUseFailure",
            "session_id": "s-new",
            "cwd": "/Users/someone/code/proj",
            "source": "startup",
            "reason": "other",
            "error_type": "rate_limit",
            "tool_name": "Bash",
            "error_message": "the command exited 1",
            "denial_reason": "auto mode refuses writes outside the worktree",
            "trigger": "auto",
            "agent_id": "a-1",
            "agent_type": "Explore",
            "notification_type": "idle_prompt",
        });
        serde_json::to_string(&super::super::hook::record_from(&payload, 4242, TS_NEW)).unwrap()
    }

    const TS_NEW: &str = "2026-09-15T10:00:00+00:00";

    /// **An old reader meets a new record: it degrades, it does not
    /// crash.** The headline test of #1061.
    ///
    /// Proven against [`OldRecord`], a struct that genuinely does not have
    /// the new fields, rather than asserted about one. Three things are
    /// checked, and only the three together mean anything:
    ///
    /// 1. The parse SUCCEEDS. Serde ignores unknown fields by default, and
    ///    this is the test that keeps that true -- adding
    ///    `#[serde(deny_unknown_fields)]` to the live `Record` would be a
    ///    one-word change that silently made every new record unreadable
    ///    to every old Headstate.
    /// 2. The fields the old reader DOES know still arrive, with the right
    ///    values. A parse that succeeded but zeroed the session id would
    ///    be a crash by another name.
    /// 3. The version is still `1`, so the old reader's strict-equality
    ///    gate in [`parse_line`] lets the line through at all. This is the
    ///    assertion that would fail if someone bumped [`RECORD_VERSION`]
    ///    to carry the new fields -- which is the mistake the constant's
    ///    docs exist to prevent.
    ///
    /// PROVEN BY SABOTAGE, three ways, because the three assertions guard
    /// different mistakes:
    ///
    /// - Adding `#[serde(deny_unknown_fields)]` to `OldRecord` (standing
    ///   in for a live `Record` that had it) fails (1):
    ///   `unknown field 'error_type', expected one of 'v', 'event', ...`.
    /// - Changing `hook::record_from` to drop `session_id` fails (2).
    /// - Bumping `hook::RECORD_VERSION` to 2 fails (3) -- and, tellingly,
    ///   fails `the_gate_lets_a_new_format_record_through` below with
    ///   `unknown_version: 1`, which is the whole record rejected rather
    ///   than just its new fields.
    #[test]
    fn a_reader_without_the_new_fields_still_reads_a_new_record() {
        let line = a_new_format_line();
        // The fixture is really a new-format line, or this test is a
        // round trip of the old format against itself.
        assert!(
            line.contains("\"error_type\":\"rate_limit\""),
            "the fixture must carry a field the old reader has never heard \
             of, or it proves nothing: {line}"
        );

        let old: OldRecord = serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!(
                "an old reader must DEGRADE on a new record, not fail to \
                 parse it: {e}\nline: {line}"
            )
        });

        assert_eq!(old.v, 1, "a new optional field must not bump the version");
        assert_eq!(old.event, "PostToolUseFailure");
        assert_eq!(old.session_id.as_deref(), Some("s-new"));
        assert_eq!(old.ppid, 4242);
        assert_eq!(old.ts.as_deref(), Some(TS_NEW));
        assert_eq!(old.cwd.as_deref(), Some("/Users/someone/code/proj"));
        assert_eq!(old.source.as_deref(), Some("startup"));
        assert_eq!(old.reason.as_deref(), Some("other"));
    }

    /// The same new-format line, through the REAL gate and all the way
    /// into the database.
    ///
    /// The test above proves the struct shape degrades; this proves the
    /// pipeline does. They are different failures: a record that
    /// deserialises fine can still be counted as `unknown_version` by
    /// [`parse_line`], which reads `v` off a `Value` before the struct is
    /// ever built.
    ///
    /// The event here is deliberately one that is NOT installed, which is
    /// the state #1061 ships in: the format accepts it before any hook
    /// writes it. It is stored as a run with no `ended_at`, per
    /// `an_unrecognised_event_is_not_an_ending`.
    #[test]
    fn the_gate_lets_a_new_format_record_through() {
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[a_new_format_line()]);

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        assert_eq!(
            got.unknown_version, 0,
            "a record carrying new optional fields at v:1 must NOT be \
             counted as a newer version -- the gate is strict equality, so \
             a bump would reject the whole record rather than just the \
             fields"
        );
        assert!(got.unparseable.is_empty(), "got {:?}", got.unparseable);
        assert_eq!(got.runs, 1);
        assert_eq!(got.sessions, 1);
    }

    /// The new fields are accepted by the live [`Record`] too, so a later
    /// sub-issue has somewhere to read them from.
    ///
    /// Without this, `Record` could quietly drop the fields it declares
    /// and both tests above would still pass -- they are about what an
    /// OLD reader does. This is the other direction: a current reader
    /// really receives what a current writer sends.
    #[test]
    fn the_current_reader_receives_every_new_field() {
        let rec: Record = serde_json::from_str(&a_new_format_line()).unwrap();

        assert_eq!(rec.error_type.as_deref(), Some("rate_limit"));
        assert_eq!(rec.tool_name.as_deref(), Some("Bash"));
        assert_eq!(rec.error_message.as_deref(), Some("the command exited 1"));
        assert_eq!(
            rec.denial_reason.as_deref(),
            Some("auto mode refuses writes outside the worktree")
        );
        assert_eq!(rec.trigger.as_deref(), Some("auto"));
        assert_eq!(rec.agent_id.as_deref(), Some("a-1"));
        assert_eq!(rec.agent_type.as_deref(), Some("Explore"));
        assert_eq!(rec.notification_type.as_deref(), Some("idle_prompt"));
    }

    /// A record from a genuinely NEWER format version is still rejected.
    ///
    /// The three tests above are all about what must keep working; this is
    /// the one about what must still be refused. If the rule "new optional
    /// fields do not bump" were implemented by loosening the gate instead,
    /// every test above would pass and the version number would mean
    /// nothing -- a `v: 2` record whose fields had CHANGED MEANING would
    /// be read and believed.
    ///
    /// `a_newer_record_version_is_its_own_count` above covers the bare
    /// case; this covers it with the new fields present, which is the
    /// shape someone would actually produce while making this mistake.
    #[test]
    fn a_newer_version_carrying_the_new_fields_is_still_refused() {
        let mut v: serde_json::Value = serde_json::from_str(&a_new_format_line()).unwrap();
        v["v"] = serde_json::json!(2);
        let t = tempfile::TempDir::new().unwrap();
        let p = path_in(t.path());
        write_file(&p, &[v.to_string()]);

        let mut conn = db();
        let got = consume(&mut conn, &p, Offset(0), &no_registry()).unwrap();

        assert_eq!(
            got.unknown_version, 1,
            "the version gate must still refuse a record from a format this \
             reader does not understand -- the additive rule is about not \
             BUMPING, not about ignoring a bump"
        );
        assert_eq!(got.runs, 0);
    }
}
