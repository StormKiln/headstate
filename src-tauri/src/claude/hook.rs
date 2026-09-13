//! The `claude-hook` subcommand: what Claude Code runs on `SessionStart`
//! and `SessionEnd` (#912).
//!
//! Reads the hook payload on stdin, adds the one fact the payload does not
//! carry -- the Claude Code process's pid -- and makes exactly ONE append to
//! `~/.claude/headstate/sessions.jsonl`. Then it exits. Nothing else happens
//! here: no subprocess, no async runtime, no network, no database, and no
//! call back into the running Headstate app.
//!
//! # The 1.5 second budget is the whole design
//!
//! `SessionEnd` hooks share a **1.5 second** budget, and async work started
//! inside one is killed before it completes
//! (anthropics/claude-code#41577). That is not a performance guideline; it
//! is a correctness boundary. A hook that does real work either delays the
//! user's `/exit` or silently loses whatever it started.
//!
//! A probe hook measured **27.7ms** -- about 50x headroom -- and that
//! margin is the entire reason the "hook appends, Headstate reads" shape in
//! #910 is safe. Everything expensive (validating the payload against disk,
//! scanning transcripts, writing rows) belongs to the consumer in #913,
//! which runs in Headstate's own poll loop with no budget over it.
//!
//! This implementation was then measured as shipped, end to end -- fork,
//! exec, dyld load of the 30MB release binary, read stdin, append, exit:
//! **5.5ms median over 20 runs, 6.3ms worst**, which is 274x and 238x
//! headroom. It comes in *under* the probe because the probe was a script
//! and this is not; the 30MB binary costs almost nothing because dyld maps
//! lazily and the dispatch in [`super::cli`] returns before any of the
//! webview or TLS stack is constructed.
//!
//! So the rule for this module is stated as a rule rather than as advice:
//! **if a change here would make this do more than read stdin and append
//! one line, it belongs in #913 instead.**
//!
//! # Why the pid is the parent, and is not searched for
//!
//! The documented hook payload carries `session_id`, `transcript_path`,
//! `cwd`, `permission_mode`, `hook_event_name`, plus `source` on start and
//! `reason` on end. It does **not** carry a pid, and the requester asked
//! for one (#910).
//!
//! Verified empirically: the hook process's parent **is** the Claude Code
//! process, with no shell wrapper in between. So the pid is the parent's,
//! taken directly -- see [`super::cli::parent_pid`].
//!
//! It is deliberately **not** found by walking up the process tree looking
//! for something named `claude`. That was tried and it finds the **wrong
//! process** whenever an outer `claude` exists further up -- which is the
//! common case, because Claude Code sessions are routinely started from
//! inside other Claude Code sessions. A tree walk would then attribute a
//! session to its grandparent, and the failure is invisible: both pids are
//! live `claude` processes, so nothing downstream looks wrong until a
//! resume targets the wrong session.
//!
//! `std::os::unix::process::parent_id()` rather than `libc::getppid()`: it
//! is stable, infallible, and needs no `unsafe` block. It is **not**
//! portable -- there is no `std::process::parent_id`, which was measured by
//! writing it and having the compiler reject it -- so the call is
//! `cfg`-gated in [`super::cli::parent_pid`], which documents what the
//! Windows arm records and why.
//!
//! The pid is a PARAMETER to everything in this module for the usual
//! reason: a test cannot choose its own parent, so a function that read the
//! ppid itself could only ever be tested against whatever ran the test
//! binary.
//!
//! # Why the record is written in one `write` call
//!
//! Concurrent appends are safe here **without a lock**, and that is a
//! measured claim rather than a hopeful one: 60 concurrent invocations and
//! 3 concurrent real sessions produced no torn or interleaved lines. The
//! mechanism is that a single small `write(2)` to a file descriptor opened
//! `O_APPEND` is atomic -- the kernel does the seek-to-end and the write
//! under one lock.
//!
//! The mechanism was confirmed by breaking it. Replacing the one
//! `write_all` below with `serde_json::to_writer` (several small writes per
//! value) and re-running 60 concurrent *processes* tore **48 of 60 lines**,
//! interleaved character by character into text that parses as no record at
//! all. So the property is the single call, not the `O_APPEND` flag on its
//! own, and the flag alone buys nothing.
//!
//! That guarantee is conditional on two things this module must therefore
//! guarantee in turn:
//!
//! 1. **One `write` call.** [`append_line`] serialises the whole record
//!    plus its newline into a single buffer and calls `write_all` once.
//!    `write_all` can in principle loop on a short write, so the buffer is
//!    kept small (see below) to stay inside the single-write regime that
//!    makes the atomicity argument hold.
//! 2. **A small record.** The probe's records were sub-512 bytes, and
//!    512 bytes is comfortably inside `PIPE_BUF`-class atomicity on every
//!    platform this ships to. [`tests::a_realistic_record_stays_small`]
//!    pins that, so a later field addition that pushes a realistic record
//!    over the line fails rather than quietly moving the design outside
//!    the regime it was measured in.
//!
//!    Measured: a worst-case record is **282 bytes**, and real records
//!    from a live run are 192 (start) and 200 (end). So there are ~230
//!    bytes of margin -- which sounds ample and is not, because it is
//!    roughly ONE path. Adding `transcript_path` alone was measured at
//!    464 bytes, consuming 80% of the remaining margin for a field that
//!    is derivable from `session_id`. That is why the omissions listed on
//!    [`Record`] are argued rather than assumed.
//!
//! If a future change cannot keep to one call, the honest move is to say so
//! and add a lock -- not to keep the comment and lose the property.
//!
//! # Payloads can be stale, and that is recorded rather than corrected
//!
//! After `/exit` followed by `--continue`, a hook can receive a
//! `session_id` and `transcript_path` belonging to the **previous** session
//! (anthropics/claude-code#9188). This module records what it was given and
//! does not try to correct it, for two reasons: correcting would mean
//! reading the transcript directory, which is exactly the disk work the
//! budget forbids; and the consumer (#913) validates against disk anyway,
//! where being wrong is recoverable.
//!
//! So a line in this file is "what the hook was told", not "what is true".
//! #913 owns the difference.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The record format version, written as `v` on every line.
///
/// #913 reads this file and must be able to tell a record it understands
/// from one written by a newer Headstate that the user has since installed
/// -- the hook and the reader are separate binaries on disk and can be
/// different versions at the same time, because a hook command line
/// installed once keeps running whatever is at that path.
///
/// A reader that finds an unknown `v` should skip that line and say it
/// skipped it, rather than guessing at fields. `v` is the first field in
/// the serialised order for exactly that reason: it can be read without
/// committing to the rest of the shape.
pub const RECORD_VERSION: u32 = 1;

/// Everything that goes on one line of `sessions.jsonl`.
///
/// # What is carried, and why each field earns its bytes
///
/// - `v` -- the format version, per [`RECORD_VERSION`].
/// - `event` -- `SessionStart` or `SessionEnd`, from the payload's
///   `hook_event_name`. Not inferred from which fields are present:
///   `source` and `reason` are both optional in practice, so absence
///   cannot distinguish the two events.
/// - `session_id` -- the `claude --resume` handle, which is the whole
///   point of the feature (#910), and the join key for every other source.
/// - `ppid` -- the Claude Code pid, per the module docs.
/// - `ts` -- when the hook ran.
/// - `cwd` -- the session's working directory, needed because
///   `claude --resume` adopts the *invoking* cwd rather than the recorded
///   one, so the resume command has to restate it.
/// - `source` / `reason` -- the event-specific field. These exist in **no
///   other source**: the live registry and the transcripts do not record
///   why a session started or how it ended, so losing them here loses them
///   permanently. That is the main thing the hooks contribute over the two
///   sources that need no install at all.
///
/// # What is deliberately omitted
///
/// - **`transcript_path`.** Derivable: it is
///   `~/.claude/projects/<slug>/<session_id>.jsonl`, and #913 scans that
///   directory regardless, so the path is a lookup rather than a fact only
///   the hook knows. It is also the field most likely to be *stale* per
///   anthropics/claude-code#9188, so storing it would mean storing a
///   second copy of the same wrong answer. Omitting it is also the single
///   largest byte saving available, which matters against the 512-byte
///   budget above.
/// - **`permission_mode`.** A property of the moment, not of the session,
///   and nothing in #910's four deliverables reads it.
/// - **`prompt_id`.** Per-turn. `SessionStart`/`SessionEnd` are not turn
///   events, so it carries no information here.
/// - **`pid_start_time`.** `claude_run.pid_start_time` wants it as the
///   pid-reuse guard, but reading it means `sysctl`/`/proc` work on the
///   parent, and the column is nullable precisely so a run recorded
///   without one reads as "cannot confirm" rather than as running.
///   #913 can pair the pid against the live registry's `procStart`, which
///   is a file read it is already doing. Cheaper there, and out of the
///   1.5s budget.
/// - **`name`.** Not in the payload at all. It is in the live registry
///   (`~/.claude/sessions/<pid>.json`) and as `aiTitle` in the transcript,
///   both of which #913 reads.
///
/// # Why every field but `v`, `event` and `ppid` is optional
///
/// The payload is another process's JSON and this runs inside a 1.5s
/// budget with no way to report a problem to anyone -- a hook's stderr is
/// not shown to the user in the normal case. A missing field must
/// therefore degrade to a recorded `null` rather than to a lost line: a
/// record with a pid and no `cwd` still tells #913 that a session started,
/// which is strictly better than the alternative of writing nothing and
/// leaving the feature silent about a session that really did start.
///
/// `v`, `event` and `ppid` are not optional because they come from us, not
/// from the payload -- `ppid` from the OS, the other two constant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    /// Format version. First in the serialised order so a reader can
    /// dispatch on it without parsing the rest -- see [`RECORD_VERSION`].
    pub v: u32,
    /// The payload's `hook_event_name`, verbatim.
    pub event: String,
    /// The payload's `session_id`: the `claude --resume` handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The Claude Code process id, from [`super::cli::parent_pid`].
    ///
    /// `0` means the writer could not determine it (the Windows arm), and
    /// is never a real pid on any platform -- a reader must treat it as
    /// "cannot say" rather than checking it for liveness.
    pub ppid: u32,
    /// When the hook ran, RFC 3339 in UTC.
    ///
    /// Named `ts` rather than `timestamp` for bytes. UTC with an explicit
    /// offset rather than a local time, because the two other sources this
    /// joins against disagree on zone -- the live registry writes
    /// `procStart` in UTC while `ps` reports local, four hours apart on
    /// the development machine, and a comparison that ignores the zone
    /// marks every session dead. A record whose zone is written down
    /// cannot contribute to that.
    pub ts: String,
    /// The payload's `cwd`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `SessionStart`'s `source`: `startup|resume|clear|compact|fork`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `SessionEnd`'s `reason`: `clear|resume|logout|prompt_input_exit|other`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A string field of the payload, when it is present AND a string.
///
/// `None` for absent, for `null`, and for a value of the wrong type. The
/// wrong-type case is folded in with absent deliberately: there is nothing
/// useful to do with a `cwd` that arrived as a number, and the alternative
/// -- refusing the whole record -- would lose the pid and the session id
/// over one bad field. See [`Record`] on why a degraded line beats no
/// line.
fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload.get(key)?.as_str().map(str::to_owned)
}

/// Turn a parsed payload plus the two facts we supply ourselves into a
/// record.
///
/// `ppid` and `now` are parameters rather than read in here so this is
/// testable without a real parent process or a real clock. The production
/// caller is [`run`], which passes `std::process::parent_id()` and the
/// current time.
///
/// # `event` when the payload does not say
///
/// `hook_event_name` is documented as always present, but a record whose
/// event is unknown is still worth writing -- it carries the pid, the
/// session id and the timestamp. So the fallback is the explicit string
/// `"unknown"` rather than an empty string or a guess from which optional
/// fields are present. `""` would be indistinguishable from a payload that
/// really sent an empty name, and guessing is wrong because both `source`
/// and `reason` can be absent.
pub fn record_from(payload: &serde_json::Value, ppid: u32, now: &str) -> Record {
    Record {
        v: RECORD_VERSION,
        event: str_field(payload, "hook_event_name").unwrap_or_else(|| "unknown".to_owned()),
        session_id: str_field(payload, "session_id"),
        ppid,
        ts: now.to_owned(),
        cwd: str_field(payload, "cwd"),
        source: str_field(payload, "source"),
        reason: str_field(payload, "reason"),
    }
}

/// Where the handoff file lives, given a home directory.
///
/// The home is a PARAMETER, following `claudemd::expand_home_in`'s reason
/// for the same shape: `$HOME` is process-global state, and a test that
/// mutates it races every other test in the binary.
pub fn handoff_path_in(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("headstate")
        .join("sessions.jsonl")
}

/// Append one record to `path`, creating the directory if needed.
///
/// # The single-write property
///
/// The JSON and its newline are built into one `String` and handed to
/// `write_all` **once**, on a descriptor opened with `.append(true)`. That
/// is the whole basis of the no-lock claim in the module docs: the kernel
/// performs the implied seek-to-end and the write atomically for an
/// `O_APPEND` descriptor, so two concurrent hooks cannot interleave.
///
/// Building the string first is load-bearing, not stylistic. Serialising
/// straight into the file (`serde_json::to_writer`) issues *several* small
/// writes per value, and two of those interleaved would produce a line
/// that is neither record and parses as neither -- the exact corruption
/// the design claims cannot happen.
///
/// # Why the file is not opened `truncate` or rotated here
///
/// Rotation is the consumer's business (#913): it is the only side that
/// knows which records it has already ingested, and a hook that truncated
/// on some size threshold could drop a record the reader had not seen
/// yet. A hook inside a 1.5s budget also cannot safely `stat`-and-rewrite
/// under concurrency without the lock this design exists to avoid.
pub fn append_line(path: &Path, record: &Record) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Serialise BEFORE opening the file. A serialisation failure then
    // cannot leave a half-written line behind, and an empty `Record` is
    // not a shape `serde_json` can fail on anyway -- the `?` is here for
    // the type, not because a failure mode is expected.
    let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    line.push('\n');

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    // ONE call. See the docs above -- this is the property, not an
    // implementation detail.
    f.write_all(line.as_bytes())
}

/// The subcommand's whole body: read stdin, append one line, report.
///
/// Returns the record it wrote so the caller can print it under a verbose
/// flag and so tests can assert on it. Errors are returned rather than
/// printed here: [`main`]'s arm decides what a hook should say, and the
/// answer is "as little as possible on stdout, because Claude Code reads
/// a hook's stdout".
pub fn run(stdin: &str, home: &Path, ppid: u32, now: &str) -> std::io::Result<Record> {
    // An unparseable payload still produces a record. `Value::Null` has no
    // fields, so every optional field degrades to `None` and the line
    // still carries `v`, `event: "unknown"`, the pid and the timestamp.
    //
    // This is the absent-is-not-zero rule applied to our own input: the
    // hook cannot report a parse failure to anyone (a hook's stderr is not
    // surfaced in the normal case), so the only way the failure becomes
    // visible at all is as a line in the file that #913 can see is
    // degraded. Writing nothing would make a real session's start
    // indistinguishable from no session at all.
    let payload: serde_json::Value = serde_json::from_str(stdin).unwrap_or(serde_json::Value::Null);
    let record = record_from(&payload, ppid, now);
    append_line(&handoff_path_in(home), &record)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload shaped like the real thing, as documented for
    /// `SessionStart` and as observed on this machine's existing hooks.
    fn start_payload() -> serde_json::Value {
        serde_json::json!({
            "session_id": "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
            "transcript_path": "/Users/someone/.claude/projects/-Users-someone-code-proj/e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2.jsonl",
            "cwd": "/Users/someone/code/proj",
            "permission_mode": "acceptEdits",
            "hook_event_name": "SessionStart",
            "source": "startup"
        })
    }

    fn end_payload() -> serde_json::Value {
        serde_json::json!({
            "session_id": "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
            "transcript_path": "/Users/someone/.claude/projects/-Users-someone-code-proj/e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2.jsonl",
            "cwd": "/Users/someone/code/proj",
            "permission_mode": "acceptEdits",
            "hook_event_name": "SessionEnd",
            "reason": "prompt_input_exit"
        })
    }

    const TS: &str = "2026-09-13T10:31:10.123456+00:00";

    #[test]
    fn a_start_payload_becomes_a_start_record() {
        let r = record_from(&start_payload(), 4242, TS);

        assert_eq!(r.v, RECORD_VERSION);
        assert_eq!(r.event, "SessionStart");
        assert_eq!(
            r.session_id.as_deref(),
            Some("e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2")
        );
        assert_eq!(r.ppid, 4242);
        assert_eq!(r.ts, TS);
        assert_eq!(r.cwd.as_deref(), Some("/Users/someone/code/proj"));
        assert_eq!(r.source.as_deref(), Some("startup"));
        // `reason` belongs to the other event and must not be invented.
        assert_eq!(r.reason, None);
    }

    #[test]
    fn an_end_payload_carries_the_reason_and_no_source() {
        let r = record_from(&end_payload(), 4242, TS);

        assert_eq!(r.event, "SessionEnd");
        assert_eq!(r.reason.as_deref(), Some("prompt_input_exit"));
        assert_eq!(r.source, None);
    }

    /// The version field is the contract with #913, which reads this file
    /// from a possibly-different Headstate version. Asserted as a literal
    /// rather than against the constant, so bumping the constant fails
    /// here and the bump has to be a deliberate, reviewed change with
    /// #913's reader updated alongside it.
    #[test]
    fn the_record_version_is_one() {
        assert_eq!(RECORD_VERSION, 1);
        let json = serde_json::to_string(&record_from(&start_payload(), 1, TS)).unwrap();
        assert!(json.contains("\"v\":1"), "got {json}");
    }

    /// `v` must be readable without parsing the rest, so #913 can skip a
    /// record from a newer writer instead of guessing at its fields.
    /// `serde` emits struct fields in declaration order, so this pins the
    /// declaration order rather than trusting it.
    #[test]
    fn the_version_is_the_first_field_on_the_line() {
        let json = serde_json::to_string(&record_from(&end_payload(), 1, TS)).unwrap();
        assert!(json.starts_with("{\"v\":1,"), "got {json}");
    }

    /// The 512-byte ceiling is not decoration: the no-lock claim rests on
    /// one small `write(2)` to an `O_APPEND` descriptor, and the records
    /// the concurrency probe measured were sub-512-byte. A field added
    /// later that pushes a realistic record past it moves the design
    /// outside the regime it was measured in, and that must fail here
    /// rather than be discovered as a torn line in someone's file.
    ///
    /// Built from EVERY documented hook payload field, with the longest
    /// realistic value for each: a deep macOS path for `cwd`, the
    /// transcript path that path implies, a UUID session id, `u32::MAX` as
    /// the pid, and both event-specific fields present at once even though
    /// no real payload carries both.
    ///
    /// The completeness is the point, and it was found by SABOTAGE. An
    /// earlier version listed only the fields [`Record`] reads today, and
    /// adding a `transcript_path` field to `Record` left this test GREEN --
    /// because the test's own payload had no `transcript_path` key, so the
    /// new field serialised as absent and cost nothing. The guard was
    /// measuring the test author's memory rather than the record.
    ///
    /// So the payload below is the payload, not a subset of it: any field a
    /// future `Record` starts reading is already populated here and will
    /// show up in the byte count.
    #[test]
    fn a_realistic_record_stays_small() {
        // Synthetic per CONTRIBUTING.md's privacy rule, and deliberately
        // long: the byte count IS the assertion, so the fixture has to be
        // as deep as a real monorepo checkout without naming a real one.
        let cwd =
            "/Users/someone/code/acme/acme-monorepo/packages/some-deeply-nested-workspace/src";
        let worst = serde_json::json!({
            "session_id": "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
            "prompt_id": "f1e2d3c4-b5a6-4798-8a9b-0c1d2e3f4a5b",
            "transcript_path": format!(
                "/Users/someone/.claude/projects/{}/e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2.jsonl",
                cwd.replace('/', "-")
            ),
            "cwd": cwd,
            "permission_mode": "bypassPermissions",
            "hook_event_name": "SessionEnd",
            "reason": "prompt_input_exit",
            "source": "compact",
        });
        let line = serde_json::to_string(&record_from(&worst, u32::MAX, TS)).unwrap();

        assert!(
            line.len() + 1 < 512,
            "a record plus its newline is {} bytes, which is outside the \
             single-write regime the no-lock design was measured in",
            line.len() + 1
        );
    }

    /// An absent optional field is omitted from the line rather than
    /// written as `null`. Bytes, against the ceiling above -- and it means
    /// a `SessionStart` record carries no `reason` key at all, so #913
    /// cannot mistake a present-but-null field for a real value.
    #[test]
    fn absent_optional_fields_are_omitted_not_nulled() {
        let json = serde_json::to_string(&record_from(&start_payload(), 1, TS)).unwrap();
        assert!(!json.contains("reason"), "got {json}");
        assert!(!json.contains("null"), "got {json}");
    }

    /// A payload that is not JSON at all still produces a line. The pid
    /// and the timestamp come from us, so the record is degraded rather
    /// than worthless -- and a hook cannot report a parse failure to
    /// anyone, so a line in the file is the only way the failure becomes
    /// visible. Writing nothing would make a real session's start
    /// indistinguishable from no session at all.
    #[test]
    fn an_unparseable_payload_still_records_the_pid() {
        let home = tempfile::TempDir::new().unwrap();
        let r = run("this is not JSON {{{", home.path(), 777, TS).unwrap();

        assert_eq!(r.event, "unknown");
        assert_eq!(r.ppid, 777);
        assert_eq!(r.session_id, None);

        let body = std::fs::read_to_string(handoff_path_in(home.path())).unwrap();
        assert_eq!(body.lines().count(), 1, "got {body:?}");
    }

    /// Empty stdin is the same case as unparseable and must not be a
    /// special one: a hook whose payload never arrived still ran, and the
    /// pid it recorded is still the Claude Code process.
    #[test]
    fn empty_stdin_still_records_the_pid() {
        let home = tempfile::TempDir::new().unwrap();
        let r = run("", home.path(), 778, TS).unwrap();

        assert_eq!(r.event, "unknown");
        assert_eq!(r.ppid, 778);
    }

    /// A field of the wrong type degrades to absent rather than losing the
    /// whole record. See [`str_field`] on why the two are folded together.
    #[test]
    fn a_wrongly_typed_field_does_not_lose_the_record() {
        let payload = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "abc",
            "cwd": 12345,
        });
        let r = record_from(&payload, 1, TS);

        assert_eq!(r.cwd, None);
        assert_eq!(r.session_id.as_deref(), Some("abc"));
        assert_eq!(r.event, "SessionStart");
    }

    /// A stale `session_id` is recorded verbatim. After `/exit` then
    /// `--continue`, hooks can receive the PREVIOUS session's id
    /// (anthropics/claude-code#9188), and this module must not try to
    /// correct it -- correcting means reading the transcript directory,
    /// which is the disk work the 1.5s budget forbids, and #913 validates
    /// against disk instead. This test exists to stop a well-meaning later
    /// change from adding that correction here.
    #[test]
    fn a_stale_session_id_is_recorded_as_given() {
        let stale = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "the-previous-sessions-id",
            "source": "resume",
        });
        let r = record_from(&stale, 1, TS);

        assert_eq!(r.session_id.as_deref(), Some("the-previous-sessions-id"));
    }

    #[test]
    fn the_handoff_path_is_under_dot_claude_headstate() {
        let p = handoff_path_in(Path::new("/home/someone"));
        assert_eq!(
            p,
            PathBuf::from("/home/someone/.claude/headstate/sessions.jsonl")
        );
    }

    /// The directory is created on first use, because a machine can have
    /// `~/.claude` without `~/.claude/headstate` -- verified on the
    /// development machine, where the directory did not exist.
    #[test]
    fn the_first_append_creates_the_directory() {
        let home = tempfile::TempDir::new().unwrap();
        assert!(!home.path().join(".claude").exists());

        run(&start_payload().to_string(), home.path(), 1, TS).unwrap();

        assert!(handoff_path_in(home.path()).is_file());
    }

    /// Appends accumulate. A second run must not truncate the first, which
    /// is what an `OpenOptions` missing `.append(true)` would do -- and
    /// #913's whole design assumes the file is a log.
    #[test]
    fn a_second_append_does_not_truncate_the_first() {
        let home = tempfile::TempDir::new().unwrap();

        run(&start_payload().to_string(), home.path(), 1, TS).unwrap();
        run(&end_payload().to_string(), home.path(), 2, TS).unwrap();

        let body = std::fs::read_to_string(handoff_path_in(home.path())).unwrap();
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 2, "got {body:?}");

        let first: Record = serde_json::from_str(lines[0]).unwrap();
        let second: Record = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first.event, "SessionStart");
        assert_eq!(second.event, "SessionEnd");
        assert_eq!(first.ppid, 1);
        assert_eq!(second.ppid, 2);
    }

    /// Every record is exactly one line, so a reader can split on newlines.
    /// A `cwd` containing a newline is legal on macOS and would tear the
    /// file if the record were not JSON-escaped, so the assertion is on
    /// the FILE rather than on the record.
    #[test]
    fn each_record_is_exactly_one_line() {
        let home = tempfile::TempDir::new().unwrap();
        let nasty = serde_json::json!({
            "hook_event_name": "SessionStart",
            "cwd": "/tmp/a\nb",
            "session_id": "x",
        });
        append_line(&handoff_path_in(home.path()), &record_from(&nasty, 1, TS)).unwrap();

        let body = std::fs::read_to_string(handoff_path_in(home.path())).unwrap();
        assert_eq!(body.lines().count(), 1, "got {body:?}");
        assert!(body.ends_with('\n'));

        let back: Record = serde_json::from_str(body.trim_end()).unwrap();
        assert_eq!(back.cwd.as_deref(), Some("/tmp/a\nb"));
    }

    /// The concurrency claim, in-process: many appenders at once to one
    /// `O_APPEND` file produce lines that all parse and none of which is
    /// torn.
    ///
    /// This is the unit-test half. The real proof is the same thing with
    /// separate PROCESSES, which is recorded in the pull request for #912
    /// -- each `append_line` opens its own descriptor, so threads here
    /// share nothing that would make this easier than the process case,
    /// but a reviewer should not have to take that on trust.
    ///
    /// Asserts on CONTENT rather than on time: every line must parse, and
    /// every appender's ppid must be accounted for exactly once.
    #[test]
    fn concurrent_appends_produce_whole_lines() {
        const APPENDERS: u32 = 60;

        let home = tempfile::TempDir::new().unwrap();
        let path = handoff_path_in(home.path());

        std::thread::scope(|s| {
            for i in 0..APPENDERS {
                let path = path.clone();
                s.spawn(move || {
                    append_line(&path, &record_from(&start_payload(), i, TS)).unwrap();
                });
            }
        });

        let body = std::fs::read_to_string(&path).unwrap();
        let mut seen = Vec::new();
        for (n, line) in body.lines().enumerate() {
            let r: Record = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {n} is torn: {e}\n{line}"));
            seen.push(r.ppid);
        }
        seen.sort_unstable();

        assert_eq!(
            seen,
            (0..APPENDERS).collect::<Vec<_>>(),
            "every append must land exactly once"
        );
    }
}
