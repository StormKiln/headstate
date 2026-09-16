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
//!    #1061 spent some of that margin and found the floor. The eight
//!    event-specific fields epic #1060 needs are additive and cost
//!    nothing when absent -- but a record carrying ALL of them at once,
//!    with both free-text fields capped at 160 bytes, measures **818
//!    bytes**, and **497** with those two fields empty. That is outside
//!    the regime, and the answer is not a smaller cap: no event carries
//!    all eight. The fields partition by event, so the ceiling that
//!    matters is the largest single partition (the subagent pair, 316
//!    bytes before text), and
//!    [`tests::every_events_worst_case_record_stays_small`] measures each
//!    event's own worst case rather than a shape nothing emits.
//!
//!    The consequence for a future field: there is room for one more
//!    event's worth, not for another eight. A field that has to coexist
//!    with a capped free-text one needs its own row in that test.
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
///
/// # The version rule (#1061), and why adding a field does NOT bump it
///
/// Epic #1060 adds six hook events, each carrying its own payload fields.
/// The obvious move -- bump `v` when the format grows -- is the wrong one
/// here, and the reason is mechanical rather than a matter of taste.
///
/// [`super::handoff::parse_line`] gates on **strict equality**: a record
/// whose `v` is anything but the reader's own `RECORD_VERSION` is counted
/// as `unknown_version` and stored nowhere. So bumping this constant does
/// not make old readers read new records *slightly worse* -- it makes them
/// reject **every** record, including the `SessionStart`/`SessionEnd` ones
/// they have always understood. And old readers are the normal case, not
/// an edge one: the installed hook command line keeps running whatever
/// binary is at that path across an app upgrade, so a new hook writing for
/// an old app is exactly what a mid-upgrade machine looks like.
///
/// Adding an **optional** field is forward-compatible without a bump.
/// [`Record`] on this side skips absent fields when serialising, and the
/// reader's `Record` carries no `#[serde(deny_unknown_fields)]`, so a
/// field an old reader has never heard of is silently ignored and the
/// fields it does know still arrive. That is the `source`/`reason`
/// precedent, applied to every event-specific field #1060 adds.
///
/// So, stated as the rule the next person needs:
///
/// - **Adding a new optional field** -- `#[serde(default)]` on the reader,
///   `skip_serializing_if` on the writer, absent meaning "this event does
///   not carry it": **does not bump.** An old reader degrades by ignoring
///   it, which is the outcome we want.
/// - **Adding a new event name** to `event`, or a new value to an existing
///   field: **does not bump.** `event` is a string the reader already
///   matches rather than exhaustively parses, and an event it does not
///   recognise is not an ending -- which
///   `handoff::tests::an_unrecognised_event_is_not_an_ending` already pins.
/// - **Changing the MEANING of an existing field**, removing one, making
///   an optional field required, or changing a field's type: **bumps.**
///   An old reader would read these and be confidently wrong, which is the
///   one failure mode a version number exists to prevent. Reading nothing
///   beats reading a wrong answer.
///
/// A bump is therefore a rare, deliberate act that requires updating the
/// reader in the same change and accepting that every record written by an
/// older hook stops being read.
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
/// - `error_type`, `tool_name`, `error_message`, `denial_reason`,
///   `trigger`, `agent_id`, `agent_type`, `notification_type` -- the same
///   shape, for the events epic #1060 adds. See "Event-specific fields"
///   below.
///
/// # Event-specific fields are FLAT and OPTIONAL, not a nested payload
///
/// #1061 settles the shape all of #1060's events use, because six events
/// each inventing their own would make this file six formats.
///
/// Every event-specific field is a **flat `Option<String>`**, absent when
/// the event does not carry it -- exactly what `source` and `reason`
/// already are. The flatness is the load-bearing part, not a style
/// preference:
///
/// - A **nested blob** (`"payload": { ... }`) would make an old reader
///   choose between deserialising a shape it has never seen and refusing
///   the record. Flat optional fields it has never heard of are simply
///   ignored by serde, so the fields it DOES understand -- the session id,
///   the pid, the timestamp -- still arrive. Degrading beats crashing.
/// - It keeps [`RECORD_VERSION`]'s rule true: a new field is additive at
///   `v: 1`, so no bump, so no old reader is cut off. See that constant's
///   docs for why a bump would reject every record rather than just the
///   new fields.
/// - It costs nothing when absent. `skip_serializing_if` means a
///   `SessionStart` record carries none of these keys at all, so the
///   512-byte single-write budget is unaffected for the two events that
///   ship today. [`tests::the_new_event_fields_cost_nothing_when_absent`]
///   pins that, and
///   [`tests::every_events_worst_case_record_stays_small`] pins that
///   even a record carrying every one of them at once stays inside it.
///
/// These fields are a FORMAT, not an install: #1061 adds no event to
/// `install::EVENTS`. Each of #1060's six sub-issues adds its own event
/// and gives its own field meaning; until then these are written only when
/// a payload happens to carry them, which for the two installed events is
/// never.
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
    /// `StopFailure`'s `error_type`: `rate_limit|overloaded|
    /// authentication_failed|…`. Why the turn died, which the transcript
    /// does not record (#1060 sub-issue 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    /// `PostToolUseFailure`'s `tool_name`: which tool failed (#1060
    /// sub-issue 2). Shared with any other event naming a tool rather than
    /// given a per-event name, because the field means the same thing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// `PostToolUseFailure`'s `error_message`.
    ///
    /// The one field here that is NOT bounded by a small vocabulary: it is
    /// free text from a failing tool, and a long one would push the line
    /// past the 512-byte single-write regime. [`TEXT_FIELD_CAP`] truncates
    /// it for that reason -- see there for why truncating beats dropping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// `PermissionDenied`'s `denial_reason` (#1060 sub-issue 3). Free text
    /// like `error_message`, and capped the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denial_reason: Option<String>,
    /// `PreCompact`/`PostCompact`'s `trigger`: `manual|auto` (#1060
    /// sub-issue 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// `SubagentStart`/`SubagentStop`'s `agent_id` (#1060 sub-issue 5) --
    /// the subagent's identity **at the source**, rather than #914's
    /// structural inference over paths and cwds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// `SubagentStart`/`SubagentStop`'s `agent_type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// `Notification`'s `notification_type`, of which `idle_prompt` is the
    /// "waiting for you" state the session list cannot show today (#1060
    /// sub-issue 6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
}

/// The longest a free-text field may be on a line, in bytes.
///
/// Every other field here comes from a small documented vocabulary
/// (`rate_limit`, `manual`, `idle_prompt`) or is a UUID, so its length is
/// bounded by the payload's own schema. `error_message` and
/// `denial_reason` are not: they are prose from a failing tool, and
/// nothing stops one being a multi-kilobyte stack trace.
///
/// That matters because the no-lock concurrency claim in this module's
/// docs rests on ONE small `write(2)` to an `O_APPEND` descriptor, and the
/// records it was measured against were sub-512 bytes. An uncapped field
/// would let a single unlucky tool failure move the whole design outside
/// the regime it was measured in -- and the symptom would be a torn line
/// in someone else's file, not a test failure here.
///
/// 160 bytes: enough for a first line of a real error message, and small
/// enough that both capped fields at once still leave the worst-case
/// record comfortably inside 512 (measured by
/// [`tests::every_events_worst_case_record_stays_small`]).
///
/// # Truncated, not dropped
///
/// Absent is not zero, and a dropped message would say "this failure had
/// no message" when the truth is "it had a long one". A truncated one says
/// what it can and marks that it was cut, with the ellipsis INSIDE the
/// cap so the cap is a real ceiling on the bytes written.
pub const TEXT_FIELD_CAP: usize = 160;

/// Why `PreCompact` is installed and `PostCompact` is not (#1065).
///
/// The docs offer both, and #1065 asks for the choice to be settled here
/// rather than taken for completeness -- epic #1060's own rule rejects
/// the second event of a pair that answers the same question at twice
/// the volume.
///
/// # What each one would carry, MEASURED
///
/// Read out of the shipped Claude Code binary (2.1.273) rather than from
/// the docs, which describe the matcher but print no payload for either
/// event:
///
/// ```text
/// PreCompact   { ..common, hook_event_name, trigger, custom_instructions }
/// PostCompact  { ..common, hook_event_name, trigger, compact_summary }
/// ```
///
/// Two findings came out of that read, and both point the same way.
///
/// **The field is `trigger`, not `compact_trigger`.** #1065's text says
/// `compact_trigger`; the binary says `trigger`, on both events. #1061
/// named the [`Record`] field `trigger`, so it happens to be right --
/// but it was right by luck until this was measured, and a field read
/// from the wrong key is a column that is empty forever with nothing to
/// say why. That is the reason this is checked against the binary rather
/// than believed from an issue.
///
/// **`PostCompact` does not fire inside a subagent.** Its first
/// statement is `if (Us(r.agentContext)) return {}` -- an early return on
/// the agent context that `PreCompact` does not have. So the two events
/// do not even cover the same population: a subagent that compacts emits
/// a `PreCompact` and no `PostCompact` at all. Installing `PostCompact`
/// as the "completed compactions" counter would silently undercount
/// exactly the sessions #1002 says are a quarter of the corpus.
///
/// # The choice
///
/// `PreCompact`, alone.
///
/// - It fires on **every** compaction, including a subagent's and
///   including one that starts and does not finish. `PostCompact` fires
///   on a strict subset, and the subset is not the interesting one.
/// - Both carry the same `trigger`, which is the whole of what #1065
///   records. A second event would add volume and no field.
/// - The question is "how hard is this session pushing context", and a
///   compaction that *began* is evidence of pressure whether or not it
///   completed. Counting only completions would report a session that
///   died mid-compaction as having compacted less than it did.
///
/// The cost is stated rather than hidden: a `PreCompact` count is
/// compactions ATTEMPTED, not compactions completed, and a compaction
/// blocked by another tool's `PreCompact` hook (the binary has
/// "Compaction blocked by PreCompact hook" as a real path) still counts
/// here. Nothing in Headstate renders it as "completed", and
/// [`super::store`] names it "compactions" for that reason.
pub mod compaction {}

/// Why `SubagentStart` is installed and `SubagentStop` is not (#1066).
///
/// The same one-of-a-pair question as [`compaction`], with a different
/// answer available and the same rule deciding it. MEASURED out of the
/// same binary:
///
/// ```text
/// SubagentStart { ..common, hook_event_name, agent_id, agent_type }
/// SubagentStop  { ..common, hook_event_name, stop_hook_active, agent_id,
///                 agent_transcript_path, agent_type, last_assistant_message,
///                 background_tasks, session_crons }
/// ```
///
/// # The choice
///
/// `SubagentStart`, alone.
///
/// - **It carries exactly the two fields #1066 wants and nothing else.**
///   `SubagentStop`'s payload is six fields wider, and one of them is
///   `last_assistant_message` -- unbounded model output, which #1066
///   explicitly forbids recording. [`record_from`] does not read it, so
///   installing `SubagentStop` would not put it in the file; but the
///   safest place for a field we must never write is an event we never
///   install.
/// - **Start is the one that always fires.** A subagent killed with
///   SIGKILL, or one whose parent dies under it, emits no `SubagentStop`
///   -- the same asymmetry `install.rs` records for `SessionEnd`. An
///   inventory keyed on stop would miss precisely the subagents that
///   went wrong.
/// - **`SubagentStop` writes `agent_type` as `b ?? ""`** -- an empty
///   string rather than an absent field when the type is unknown. An
///   empty string is a value, and a value is what
///   [`super::signals`] would have to special-case to avoid rendering
///   a blank agent type as a real one. `SubagentStart` passes the type
///   through as given.
///
/// The cost: Headstate learns that a subagent STARTED and never that it
/// finished. That is acceptable because nothing here counts durations --
/// #1066 is about `agent_type` being stated rather than inferred, and
/// the type is known at the start.
///
/// # `session_id` on these payloads is the PARENT's
///
/// The common fields are built as `{ session_id: e.id, ..., agent_id }`
/// from the session the hook fires for, and `SubagentStart` passes the
/// *parent* session. So a `SubagentStart` row is keyed on the session
/// that SPAWNED the subagent, and `agent_id` names the subagent within
/// it.
///
/// That is why [`super::signals::Events::agent_types`] reads these rows as "what
/// this session spawned" and never as "what this session IS", and why
/// the disagreement check in [`super::signals::AgentTypes`] compares a
/// parent's stated spawns against the inference over its children rather
/// than against the row's own id. Getting this backwards would attribute
/// every subagent's type to its parent.
pub mod subagents {}

/// A free-text payload field, capped at [`TEXT_FIELD_CAP`] bytes.
///
/// Truncation is on a **char boundary**, because `String` is UTF-8 and
/// slicing mid-codepoint panics -- a panic inside a hook is the worst
/// available outcome, since it would take the record with it and exit
/// non-zero into the user's session.
fn capped_str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    let s = str_field(payload, key)?;
    if s.len() <= TEXT_FIELD_CAP {
        return Some(s);
    }
    const ELLIPSIS: &str = "…";
    // The ellipsis is 3 bytes in UTF-8 and lives INSIDE the cap, so the
    // result is never longer than the cap however long the input was.
    let room = TEXT_FIELD_CAP - ELLIPSIS.len();
    let mut end = room;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}{ELLIPSIS}", &s[..end]))
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
        // Read unconditionally rather than switched on `event`. A payload
        // that does not carry a key yields `None` and costs no bytes, and
        // switching would mean this function knowing every event name --
        // which is exactly the coupling #1061 exists to avoid, since each
        // of #1060's six sub-issues would then have to edit this match.
        //
        // It also keeps the hook O(1): this is eight `get` calls on an
        // already-parsed `Value`, with no disk touched.
        error_type: str_field(payload, "error_type"),
        tool_name: str_field(payload, "tool_name"),
        error_message: capped_str_field(payload, "error_message"),
        denial_reason: capped_str_field(payload, "denial_reason"),
        trigger: str_field(payload, "trigger"),
        agent_id: str_field(payload, "agent_id"),
        agent_type: str_field(payload, "agent_type"),
        notification_type: str_field(payload, "notification_type"),
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

    // -----------------------------------------------------------------
    // #1061: the event-specific fields the epic's six events will use.
    // -----------------------------------------------------------------

    /// A payload carrying every event-specific field #1061 adds, for the
    /// tests that need a worst case.
    ///
    /// Not realistic as a single payload -- no event carries all of them
    /// at once -- and deliberately so. The byte assertion below has to
    /// hold for the worst line the format can produce, not the likely one.
    fn every_field_payload() -> serde_json::Value {
        serde_json::json!({
            "session_id": "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
            "cwd": "/Users/someone/code/acme/acme-monorepo/packages/some-workspace/src",
            "hook_event_name": "PostToolUseFailure",
            "source": "compact",
            "reason": "prompt_input_exit",
            "error_type": "authentication_failed",
            "tool_name": "Bash",
            "error_message": "the tool exited 1",
            "denial_reason": "the write is outside the allowed directory",
            "trigger": "manual",
            "agent_id": "f1e2d3c4-b5a6-4798-8a9b-0c1d2e3f4a5b",
            "agent_type": "general-purpose",
            "notification_type": "idle_prompt",
        })
    }

    /// Every field the epic's six events carry is mapped, and mapped from
    /// the payload key the event documents.
    ///
    /// A field declared on [`Record`] but never read from the payload
    /// would serialise as absent forever, and the sub-issue that came to
    /// use it would find an always-empty column with nothing to say why.
    #[test]
    fn every_event_field_is_read_from_the_payload() {
        let r = record_from(&every_field_payload(), 1, TS);

        assert_eq!(r.error_type.as_deref(), Some("authentication_failed"));
        assert_eq!(r.tool_name.as_deref(), Some("Bash"));
        assert_eq!(r.error_message.as_deref(), Some("the tool exited 1"));
        assert_eq!(
            r.denial_reason.as_deref(),
            Some("the write is outside the allowed directory")
        );
        assert_eq!(r.trigger.as_deref(), Some("manual"));
        assert_eq!(
            r.agent_id.as_deref(),
            Some("f1e2d3c4-b5a6-4798-8a9b-0c1d2e3f4a5b")
        );
        assert_eq!(r.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(r.notification_type.as_deref(), Some("idle_prompt"));
    }

    /// The new fields cost ZERO bytes on the two events shipping today.
    ///
    /// This is what makes the format additive rather than expensive: a
    /// `SessionStart` line after #1061 is byte-identical to one before it,
    /// because `skip_serializing_if` omits a field the payload did not
    /// carry. A `null` for each of the eight would be ~140 wasted bytes on
    /// every line of a file that grows without bound.
    ///
    /// PROVEN BY SABOTAGE: removing `skip_serializing_if` from
    /// `error_type` fails here with `"error_type":null` on the line.
    #[test]
    fn the_new_event_fields_cost_nothing_when_absent() {
        for payload in [start_payload(), end_payload()] {
            let json = serde_json::to_string(&record_from(&payload, 1, TS)).unwrap();
            for field in [
                "error_type",
                "tool_name",
                "error_message",
                "denial_reason",
                "trigger",
                "agent_id",
                "agent_type",
                "notification_type",
            ] {
                assert!(
                    !json.contains(field),
                    "an event that does not carry {field} must not write the \
                     key at all: {json}"
                );
            }
            assert!(!json.contains("null"), "got {json}");
        }
    }

    /// Every event's own worst-case record stays inside the 512-byte
    /// single-write regime.
    ///
    /// `a_realistic_record_stays_small` guards the worst case of the two
    /// events installed today. This guards the worst case for each event
    /// #1060 adds, which is the number that matters once they are
    /// installed.
    ///
    /// # Why this is PER EVENT and not one all-fields-at-once record
    ///
    /// MEASURED while writing this test, and it changed the design: a
    /// record carrying all eight new fields at once, with both free-text
    /// fields at a 160-byte cap, is **818 bytes** -- outside the regime.
    /// Even with both text fields EMPTY it is 497, leaving 14 bytes.
    ///
    /// The honest response is not to shrink the cap until that number
    /// fits, because that would be tuning a guard to pass on a shape
    /// nothing produces. No event carries all eight: `error_type` is
    /// `StopFailure`'s, `agent_id`/`agent_type` are the subagent events',
    /// `trigger` is the compaction events'. The fields PARTITION by event,
    /// and the worst line the writer can actually emit is the largest
    /// single partition.
    ///
    /// So each event's real field set is measured, with the free-text
    /// fields at their cap. The widest is the subagent pair at 316 bytes
    /// before any text, and `PostToolUseFailure` at 273 plus a capped
    /// message. That is the ceiling [`TEXT_FIELD_CAP`] is sized against.
    ///
    /// The cost of this choice is stated rather than hidden: if a future
    /// event carries BOTH a subagent identity and a capped message, this
    /// test will not have covered it -- so that event must add its own row
    /// to the table below, which is why the table is a table.
    ///
    /// # That combination, MEASURED (#1066)
    ///
    /// #1065/#1066/#1067 took #1061 up on that and measured it, because
    /// #1066 installs a subagent event and the question is whether the
    /// next one has room:
    ///
    /// ```text
    /// PreCompact                                 250 bytes
    /// Notification (longest documented value)    282
    /// SubagentStart (plugin-scoped agent_type)   358
    /// subagent identity + a capped free text     496   <- the uncovered shape
    /// ```
    ///
    /// **It fits, with 16 bytes to spare.** That is a pass and it is not
    /// a margin anyone should spend: one more field of any kind on such
    /// an event puts it over. So the answer to #1061's open question is
    /// "yes, but only just, and nothing further" -- recorded here rather
    /// than left for the next person to re-derive.
    ///
    /// None of the three events installed by #1065/#1066/#1067 is that
    /// shape. `SubagentStart` carries no free-text field at all (see
    /// [`super::subagents`] for why the pair's other half, which does,
    /// is not installed), so the worst line these three can emit is 358.
    ///
    /// PROVEN BY SABOTAGE: raising `TEXT_FIELD_CAP` to 400 fails here at
    /// `PostToolUseFailure` (673 bytes), and removing the cap entirely
    /// fails at 4000-odd.
    #[test]
    fn every_events_worst_case_record_stays_small() {
        // The fields each of #1060's events actually carries. A deep cwd
        // and a UUID session id in every row, because those are the
        // bytes that are always there.
        let cwd =
            "/Users/someone/code/acme/acme-monorepo/packages/some-deeply-nested-workspace/src";
        let long = "x".repeat(4000);
        let cases: &[(&str, serde_json::Value)] = &[
            ("SessionStart", serde_json::json!({ "source": "compact" })),
            (
                "SessionEnd",
                serde_json::json!({ "reason": "prompt_input_exit" }),
            ),
            (
                "StopFailure",
                serde_json::json!({ "error_type": "authentication_failed" }),
            ),
            (
                "PostToolUseFailure",
                serde_json::json!({ "tool_name": "Bash", "error_message": long }),
            ),
            (
                "PermissionDenied",
                serde_json::json!({ "tool_name": "Bash", "denial_reason": long }),
            ),
            ("PreCompact", serde_json::json!({ "trigger": "manual" })),
            (
                "SubagentStop",
                serde_json::json!({
                    "agent_id": "f1e2d3c4-b5a6-4798-8a9b-0c1d2e3f4a5b",
                    "agent_type": "general-purpose",
                }),
            ),
            // #1066 installs `SubagentStart`, so its own worst case is
            // measured rather than assumed to match `SubagentStop`'s.
            // The agent TYPE is the free-ish part: it is a user-chosen
            // agent name, and a plugin-scoped one is the longest shape
            // the docs describe. It is NOT capped -- it comes from a
            // vocabulary the user controls rather than from model output
            // -- so this row is what establishes that an uncapped
            // agent_type still fits.
            (
                "SubagentStart",
                serde_json::json!({
                    "agent_id": "f1e2d3c4-b5a6-4798-8a9b-0c1d2e3f4a5b",
                    "agent_type":
                        "some-long-plugin-name:some-long-custom-agent-reviewer-name",
                }),
            ),
            (
                "Notification",
                serde_json::json!({ "notification_type": "idle_prompt" }),
            ),
        ];

        for (event, extra) in cases {
            let mut payload = serde_json::json!({
                "hook_event_name": event,
                "session_id": "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
                "cwd": cwd,
            });
            for (k, v) in extra.as_object().unwrap() {
                payload[k] = v.clone();
            }
            let line = serde_json::to_string(&record_from(&payload, u32::MAX, TS)).unwrap();

            assert!(
                line.len() + 1 < 512,
                "a worst-case {event} record is {} bytes, which is outside \
                 the single-write regime the no-lock design was measured in \
                 -- see TEXT_FIELD_CAP",
                line.len() + 1
            );
        }
    }

    /// A free-text field longer than the cap is TRUNCATED, not dropped and
    /// not written whole.
    ///
    /// All three outcomes are asserted because each of the other two is a
    /// real failure: writing it whole moves the line outside the
    /// single-write regime, and dropping it says "this failure had no
    /// message" when the truth is "it had a long one".
    ///
    /// PROVEN BY SABOTAGE: making `capped_str_field` delegate to
    /// `str_field` fails here at the length assertion (4000 > 160), and
    /// fails `every_events_worst_case_record_stays_small` at 8000-odd
    /// bytes -- which is the failure that matters, since that is a torn
    /// line in a user's file.
    #[test]
    fn a_long_free_text_field_is_truncated_rather_than_dropped() {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUseFailure",
            "error_message": format!("the tool failed: {}", "x".repeat(4000)),
        });

        let r = record_from(&payload, 1, TS);

        let msg = r.error_message.expect(
            "a long message must be truncated, not dropped -- absent \
                     would say the failure had no message",
        );
        assert!(
            msg.len() <= TEXT_FIELD_CAP,
            "the cap must be a real ceiling on the BYTES written, ellipsis \
             included: {} bytes",
            msg.len()
        );
        assert!(
            msg.starts_with("the tool failed: xxx"),
            "truncation must keep the START of the message, which is where a \
             tool puts what went wrong: {msg}"
        );
        assert!(
            msg.ends_with('…'),
            "a cut message must say it was cut: {msg}"
        );
    }

    /// A message exactly at the cap is left alone -- the boundary is
    /// `<=`, not `<`.
    ///
    /// Without this the test above passes for a cap that truncates
    /// everything, including messages that fit.
    #[test]
    fn a_message_that_fits_is_not_truncated() {
        let exact = "x".repeat(TEXT_FIELD_CAP);
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUseFailure",
            "error_message": exact.clone(),
        });

        assert_eq!(record_from(&payload, 1, TS).error_message, Some(exact));
    }

    /// Truncation lands on a char boundary rather than mid-codepoint.
    ///
    /// `String` is UTF-8 and slicing mid-codepoint PANICS. A panic inside
    /// a hook is the worst outcome available: it takes the record with it
    /// and exits non-zero into the user's session, which is the one thing
    /// this module promises never to do.
    ///
    /// The fixture is multi-byte characters only, so the naive cut lands
    /// inside one -- `TEXT_FIELD_CAP - 3` is not a multiple of 3.
    #[test]
    fn truncation_does_not_split_a_multi_byte_character() {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUseFailure",
            // 3 bytes each, so the byte cap falls inside a character.
            "denial_reason": "→".repeat(500),
        });

        let r = record_from(&payload, 1, TS);

        let msg = r.denial_reason.unwrap();
        assert!(msg.len() <= TEXT_FIELD_CAP, "{} bytes", msg.len());
        assert!(
            msg.trim_end_matches('…').chars().all(|c| c == '→'),
            "got {msg}"
        );
    }

    /// The record still serialises `v` first with the new fields present.
    ///
    /// `the_version_is_the_first_field_on_the_line` covers today's two
    /// events; this covers the shape #1060 will write. Serde emits fields
    /// in declaration order, so a new field declared above `v` would move
    /// it -- and a reader that cannot find `v` without parsing the whole
    /// line loses the cheap dispatch the version exists for.
    #[test]
    fn the_version_is_still_first_with_every_field_present() {
        let json = serde_json::to_string(&record_from(&every_field_payload(), 1, TS)).unwrap();
        assert!(json.starts_with("{\"v\":1,"), "got {json}");
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
