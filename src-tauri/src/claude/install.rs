//! Installing the `claude-hook` command line into `~/.claude/settings.json`
//! (#915, epic #910 §5).
//!
//! This is the one module in the tree that WRITES to a file Headstate does
//! not own. Everything below is shaped by that, and by two measured facts
//! about what happens when it goes wrong.
//!
//! # Fact one: a malformed settings.json is ignored SILENTLY
//!
//! MEASURED (#910 §1.7): `claude` was started with a deliberately corrupt
//! `.claude/settings.json`. It started normally, answered normally, and the
//! hooks **never ran**. No error, no warning, no line on stderr.
//!
//! That is the nastiest finding in the whole investigation, and it decides
//! this module's central rule: **a file we cannot parse is REFUSED, never
//! rewritten.** Three consequences follow, and each of them is a thing this
//! module does rather than a thing it avoids:
//!
//! - An installer that wrote a file it could not re-parse would produce a
//!   feature that is silently inert. So [`install`] parses the result back
//!   before it renames the temp file into place, and a result that does not
//!   re-parse is thrown away rather than shipped.
//! - "Installed" is never inferred from "we wrote the file". [`status`]
//!   READS the file, every time (§5.5). There is no cached flag anywhere,
//!   because a cached "installed" is wrong the moment the user hand-edits
//!   the file -- the same staleness argument the schema makes for liveness.
//! - A user whose file is already broken has EVERY hook dead and no symptom.
//!   The install dialog is the best chance anyone has to tell them, so
//!   [`Refusal`] carries the parse error's own message through to the UI
//!   rather than flattening it to "could not install".
//!
//! Repairing is refused too, not merely deferred. A "fix" would have to
//! discard the settings we failed to parse, which is a strictly worse
//! outcome than the one the user already has.
//!
//! # Fact two: `hooks.<Event>` is a LIST, and lists merge
//!
//! `hooks.SessionStart` is an array of matcher objects, and Claude Code
//! merges those arrays across settings precedence rather than picking one.
//! So an installer that SETS the array destroys whatever was in it.
//!
//! Not hypothetical. The development machine's real `~/.claude/settings.json`
//! has ten hook events pointing at an iTerm2 `cc-status` binary, plus a
//! `codegraph prompt-hook` under `UserPromptSubmit` -- measured today:
//!
//! ```text
//! hook events: 10
//!   Notification      1 matcher   cc-status
//!   PermissionRequest 1 matcher   cc-status
//!   PostToolUse       1 matcher   cc-status
//!   PreToolUse        1 matcher   cc-status
//!   SessionEnd        1 matcher   cc-status
//!   SessionStart      1 matcher   cc-status
//!   Stop              1 matcher   cc-status
//!   StopFailure       1 matcher   cc-status
//!   SubagentStop      1 matcher   cc-status
//!   UserPromptSubmit  2 matchers  codegraph prompt-hook, cc-status
//! ```
//!
//! So [`install`] APPENDS one matcher and touches nothing else, and the
//! headline test ([`tests::install_then_uninstall_leaves_a_foreign_hook_and_the_document_untouched`])
//! is a round-trip against a fixture carrying exactly that foreign entry.
//! Proven by sabotage: replacing the append with an assignment fails it with
//! the foreign command gone.
//!
//! # How uninstall finds only OUR entry: two predicates
//!
//! A matcher is ours if `_headstate` is present **or** its command line
//! points at our subcommand. Two predicates because each covers the other's
//! gap (§5.2):
//!
//! - The **marker** survives a user reformatting or reordering the file, and
//!   survives the app moving -- which changes the path but not the marker.
//! - The **command** catches an entry whose marker a user deleted while
//!   hand-editing, and would have caught an entry written by a
//!   pre-marker version of this code.
//!
//! The marker itself is safe to write: an unknown key on a hook matcher is
//! TOLERATED, measured (#910 §1.6) -- a matcher tagged `"_headstate": 1`
//! still fired.
//!
//! Anything that matches neither predicate is left strictly alone. That is
//! the whole safety property, and [`is_ours`] is the only place it is
//! decided.
//!
//! # Reinstall is the repair operation
//!
//! [`install`] IS a reinstall: it drops every matcher it recognises as ours
//! and appends exactly one fresh matcher per event, in one write. Idempotent
//! by construction, which makes it the fix for the cases that actually
//! happen -- a stale absolute path after the app moves to `/Applications`, a
//! duplicate from an interrupted install, a matcher someone hand-edited.
//!
//! A replaced hand-edit is REPORTED ([`Installed::replaced`]) rather than
//! done quietly: silently reverting someone's edit is its own defect.
//!
//! # Every write is atomic, and that is not decoration
//!
//! Temp file in the same directory, `sync_all`, then `rename`. `rename(2)`
//! within a filesystem is atomic, so a reader sees the old file or the new
//! one and never a half-written one.
//!
//! The reason this matters more here than in most places is fact one above:
//! a crash mid-write is the thing that would PRODUCE the silently-ignored
//! malformed file. Without the temp-and-rename, this module's own failure
//! mode would be the exact state it refuses to touch.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The hook events Headstate installs.
///
/// `SessionStart` and `SessionEnd` bound a session, which is the whole
/// question #910 asks. The bulk hooks are still rejected for the reason
/// they always were: `Stop`, `UserPromptSubmit` and `PostToolUse` are
/// per-turn or per-tool-call, and the transcript already carries that
/// detail at a cost measured in milliseconds, so installing them would add
/// volume without adding an answer.
///
/// # The three FAILURE events, and why they pass the same test (#1062-#1064)
///
/// `StopFailure`, `PostToolUseFailure` and `PermissionDenied` are here
/// because they pass that test rather than because they are cheap to add.
/// Each answers something a windowed transcript reader cannot: the reader
/// is 40 head records plus a 16 KB tail (`transcript.rs:151,163`), so a
/// failure scattered through the body of a 73 MB transcript is outside the
/// window, and the alternative is a full-body scan of a 1.8 GB corpus that
/// grows without bound.
///
/// - **`StopFailure`** (#1062) carries `error_type` -- `rate_limit`,
///   `overloaded`, `authentication_failed` -- and says why a turn died.
///   The paragraph this replaces called it "the one genuine candidate" and
///   excluded it, and the EXCLUSION REASONING STILL BINDS: it does not
///   fire on SIGKILL, so it adds error context and **not liveness**.
///   Nothing deciding "is this session running" may consult it. That is
///   enforced structurally rather than remembered -- these events are
///   stored in `claude_event`, and `liveness::derive` reads `claude_run`,
///   which has no column for them. `invariants.rs`'s
///   `liveness_never_reads_the_failure_events` guards the boundary.
/// - **`PostToolUseFailure`** (#1063) is the narrow slice of `PostToolUse`
///   that carries information. The volume argument does not transfer
///   because failures are the small fraction: successes dominate, and a
///   session where `Bash` failed 14 times is a session that was fighting
///   something.
/// - **`PermissionDenied`** (#1064) is the clearest case in the epic. A
///   denial is a thing that DID NOT HAPPEN: there is no tool result to
///   parse and no durable record a reader could find at any price. It is
///   also not an error -- it is a guardrail working -- and the UI is
///   required to word it that way.
///
/// None of the three records `tool_input`. It carries file contents,
/// command lines and credentials-adjacent strings, and `tool_name` plus
/// the reason already answers the question; storing it would create a
/// privacy surface in a file this app reads on every scan.
///
/// # This list is ADDITIVE, and that is a tested property (#1061)
///
/// Epic #1060 adds six events, one sub-issue each. #1061 makes adding one
/// a single-line change here by pinning what "additive" has to mean, so
/// that the seventh person to add an event does not have to re-derive it:
///
/// - **Adding an event does not disturb the existing ones.** [`install`]
///   iterates this list and appends one matcher per event, touching no
///   other key -- so the two events installed today are written exactly as
///   they were before the list grew.
///   [`tests::adding_an_event_leaves_the_existing_two_untouched`] proves it
///   against an expanded list rather than asserting it.
/// - **Uninstall removes every key it added, and nothing it did not.**
///   [`uninstall`] deliberately sweeps **every** key under `hooks` rather
///   than this list (see its docs), so an event removed from this list in a
///   later version is still cleaned up. That is what makes the round trip
///   byte-identical for any list, not just this one.
/// - **A PARTIAL install is a real state, not a bug to assume away.** A
///   user can be mid-upgrade, or have hand-deleted one matcher; [`status`]
///   models it as [`Status::Stale`] naming the missing event, and
///   `install` repairs it by dropping and rewriting every event in the
///   list. An uninstall from a partial install must still leave the file
///   byte-identical --
///   [`tests::uninstall_from_a_partial_install_of_an_expanded_set_is_byte_identical`]
///   pins that.
///
/// What adding an event does NOT need is a record-format change: #1061
/// already carries every event-specific field the epic names, optional at
/// `v: 1`. See [`super::hook::RECORD_VERSION`] for why that is not a bump.
/// The three events above were added without touching it, which is the
/// property #1061 was built to provide -- `RECORD_VERSION` is still `1`
/// and an old reader still accepts every line.
///
/// The one thing an addition still owes is the argument in the paragraphs
/// above: a new event must answer something the transcript cannot, or
/// cannot afford to. Being cheap to add is not a reason to add one.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    // #1062: why the turn died -- `rate_limit`, `overloaded`,
    // `authentication_failed`. Error context ONLY: it does not fire on
    // SIGKILL, so nothing deciding liveness may consult it.
    "StopFailure",
    // #1063: which tools fail, and how. The narrow slice of `PostToolUse`
    // that carries information, because failures are the rare fraction.
    "PostToolUseFailure",
    // #1064: what auto mode declined. A guardrail working, not an error,
    // and the only record of a thing that did not happen.
    "PermissionDenied",
    // #1065: context pressure. `PreCompact` and NOT `PostCompact` --
    // see `hook::compaction` for the argument and the measurement.
    "PreCompact",
    // #1066: the subagent's `agent_type` stated rather than inferred.
    // `SubagentStart` and NOT `SubagentStop`: the pair would double the
    // volume for the same two fields, and only the start is guaranteed
    // to fire -- see `hook::subagents`.
    "SubagentStart",
    // #1067: which session is waiting on the user.
    "Notification",
];

/// The subcommand the installed command line invokes.
///
/// # Why this constant is HERE and not imported from `cli`
///
/// #912 (PR 925) adds `claude::cli::SUBCOMMAND` with this same value, and
/// the two must agree or the installed hook invokes a subcommand the binary
/// does not recognise -- which fails in the worst possible way, silently:
/// `cli::classify` returns `Invocation::App`, so the hook boots the whole
/// GUI application instead of appending a line.
///
/// That PR is not merged yet, so importing it would make this change
/// unmergeable until it lands, for one string. The honest arrangement is a
/// local constant plus a GUARD:
/// [`tests::the_subcommand_agrees_with_the_cli_module`] asserts the two are
/// equal the moment `cli` exists, so the duplication cannot drift past the
/// merge. Until then the test reads this module's own value and passes
/// trivially, which the test says out loud rather than hiding.
///
/// `claude-hook` rather than `hook`: this string is written into a settings
/// file a human reads and edits, and a bare `hook` says nothing about whose
/// hook it is.
pub const SUBCOMMAND: &str = "claude-hook";

/// The key that marks a matcher as ours.
///
/// Underscore-prefixed because it is not part of Claude Code's schema. An
/// unknown key on a matcher is tolerated -- MEASURED (#910 §1.6): a matcher
/// tagged with this key still fired and still recorded its event.
///
/// This is the FIRST of [`is_ours`]'s two predicates, and the one that
/// survives the app moving: a reinstall after a move changes the command
/// path, so a path-only predicate would strand the old matcher.
pub const MARKER: &str = "_headstate";

/// Why an install, uninstall or status read could not proceed.
///
/// Each variant exists because the REMEDY differs, which is the only reason
/// to split an error. "Could not install" with a single message would leave
/// a user with a malformed file no way to tell it apart from a permissions
/// problem -- and the malformed case is the one where every one of their
/// hooks is already silently dead.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refusal {
    /// The file exists but is not valid JSON.
    ///
    /// REFUSED, not repaired (see the module docs). `detail` is serde's own
    /// message, with its line and column, because that is what a user needs
    /// in order to fix the file by hand -- and fixing it by hand is the only
    /// remedy, since a rewrite would discard whatever we could not parse.
    Malformed {
        /// The path, so the message can name the file to open.
        path: String,
        /// serde_json's error, verbatim: "expected `,` or `}` at line 12
        /// column 3" is actionable and "invalid settings" is not.
        detail: String,
    },
    /// The file parses, but `hooks` is present and is not an object.
    ///
    /// Same refusal as malformed and for the same reason: we would have to
    /// throw away a value the user put there. It is a separate variant
    /// because serde parsed the file fine, so there is no parse error to
    /// quote and no line number to point at.
    HooksNotAnObject {
        /// The path.
        path: String,
        /// What was found instead, for a message that says something.
        found: String,
    },
    /// A matcher under one of our events is not an object, or its `hooks`
    /// key is not an array.
    ///
    /// Refused rather than skipped. We cannot decide whether such an entry
    /// is ours, so an uninstall could not promise to have removed our hook
    /// and an install could not promise not to have duplicated it. Saying
    /// so beats acting on a structure we do not understand.
    MatcherNotUnderstood {
        /// The path.
        path: String,
        /// Which event's array holds it, so the user can find it.
        event: String,
    },
    /// The file could not be read, or the write could not be completed.
    ///
    /// Distinct from [`Refusal::Malformed`] because the remedy is
    /// permissions or disk rather than an editor.
    Io {
        /// The path involved.
        path: String,
        /// The OS error.
        detail: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed { path, detail } => write!(
                f,
                "{path} is not valid JSON ({detail}). Headstate will not rewrite it: \
                 Claude Code silently ignores a settings file it cannot parse, so every \
                 hook in this file is already inactive. Fix the JSON by hand, then install."
            ),
            Self::HooksNotAnObject { path, found } => write!(
                f,
                "{path} has a \"hooks\" key that is {found} rather than an object. \
                 Headstate will not overwrite it -- fix it by hand, then install."
            ),
            Self::MatcherNotUnderstood { path, event } => write!(
                f,
                "{path} has an entry under \"{event}\" that Headstate does not \
                 understand, so it cannot tell whether it is its own. Fix it by hand, \
                 then install."
            ),
            Self::Io { path, detail } => write!(f, "could not use {path}: {detail}"),
        }
    }
}

/// Whether the hooks are installed -- three states, because two would lie.
///
/// `CannotTell` is the state that must not render as `NotInstalled` (§5.5):
/// the remedy differs completely. "Not installed" invites a click on
/// Install, which for a malformed file is exactly the click that must be
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Every event in [`EVENTS`] has exactly one matcher of ours, and its
    /// command line is the one we would write now.
    Installed {
        /// The command line found, so the UI can show what will run.
        command: String,
    },
    /// No matcher of ours under any event in [`EVENTS`]. Includes the
    /// file-does-not-exist case, which is normal rather than an error.
    NotInstalled,
    /// Ours is there but not as we would write it now: a stale path from
    /// before the app moved, a duplicate from an interrupted install, a
    /// hand-edited command, or only one of the two events.
    ///
    /// A separate state from `Installed` because the user needs to know a
    /// REINSTALL will fix it, and separate from `NotInstalled` because
    /// something of ours really is in their file.
    Stale {
        /// What is wrong, in a sentence the UI shows verbatim.
        detail: String,
    },
    /// The file could not be read or parsed. Carries the [`Refusal`] so the
    /// UI can explain, rather than a bare boolean that cannot.
    CannotTell(Refusal),
}

/// What an install changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    /// The command line written into every matcher.
    pub command: String,
    /// Events that had no matcher of ours and now have one.
    pub added: Vec<String>,
    /// Events where a matcher of ours was dropped and rewritten.
    ///
    /// Reported rather than silent, per §5.4: this is the count that tells
    /// a user their hand-edit was reverted, and reverting someone's edit
    /// without saying so is its own defect.
    pub replaced: Vec<String>,
    /// True when the settings file did not exist and was created.
    pub created_file: bool,
}

/// What an uninstall removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Uninstalled {
    /// Events a matcher of ours was removed from.
    pub removed: Vec<String>,
    /// True when there was nothing of ours to remove. Not an error: an
    /// uninstall of something already absent has achieved what was asked.
    pub was_absent: bool,
}

/// Which events are actually being recorded right now.
///
/// The question [`super::events`] needs in order to tell a total from a
/// floor: a count of denials is complete only if `PermissionDenied` was
/// installed for the whole period, and a bare count cannot say which.
///
/// # Why this reads a [`Status`] rather than the file
///
/// The caller has already read the file once for the page's install
/// banner. Reading it again -- per session, on a list of 1,474 rows --
/// would be one settings parse per row for an answer that cannot change
/// between them.
///
/// # The three states, and why `CannotTell` is empty
///
/// | status | answer | why |
/// |---|---|---|
/// | `Installed` | every event in [`EVENTS`] | they are all there and current |
/// | `Stale` | none | we cannot tell WHICH are intact from the sentence |
/// | `NotInstalled` | none | nothing is recording |
/// | `CannotTell` | none | the file could not be read |
///
/// `Stale` returning NOTHING is the conservative direction and it is
/// chosen deliberately. A stale install may be missing an event, may have
/// a duplicate, or may point at a moved binary that is not running at all
/// -- and `Status::Stale` carries a human sentence rather than a list, so
/// there is nothing to parse. Claiming completeness on the strength of a
/// state that means "something is wrong with this install" would be
/// exactly the confidently-wrong number the house rule forbids. The cost
/// is that a stale install renders as a floor, which is true.
pub fn recording_events(status: &Status) -> Vec<String> {
    match status {
        Status::Installed { .. } => EVENTS.iter().map(|e| (*e).to_string()).collect(),
        // Qualify, or suppress. None of these three can say WHICH events
        // are intact, so none of them may claim any are.
        Status::Stale { .. } | Status::NotInstalled | Status::CannotTell(_) => Vec::new(),
    }
}

/// `~/.claude/settings.json` under a given home directory.
///
/// Parameterised on `home` rather than reading `$HOME` itself, for the
/// reason every path in this module is: the tests MUST NOT be able to reach
/// the developer's own settings file, and a function that resolves its own
/// home is a function a test can only run against the real one.
pub fn settings_path_in(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

/// The command line Claude Code will run, for a given app binary.
///
/// `<abs path to the app binary> claude-hook`. It is a SUBCOMMAND of the
/// app binary rather than a sidecar, which #912 decided and argued: the
/// bundle format guarantees `Headstate.app/Contents/MacOS/headstate` and the
/// Tauri updater replaces the bundle in place, whereas a sidecar lands under
/// `Contents/Resources` with a target-triple suffix in its name -- so the
/// installed command line would differ per architecture and an
/// Intel-to-ARM migration would point at a path that no longer exists.
///
/// # Why the path is quoted
///
/// Claude Code runs a hook command through a shell, so a path containing a
/// space would otherwise be split into two arguments. `/Applications` is
/// space-free but an app run from `~/Library/Mobile Documents` or a
/// user-renamed `Headstate 2.app` is not, and the failure mode is the
/// silent one: the hook simply never runs.
///
/// Single quotes rather than double: inside double quotes a `$` or a
/// backtick in a path would still be expanded by the shell. A single quote
/// in the path itself is escaped the POSIX way (`'\''`), which is ugly and
/// correct.
pub fn hook_command(exe: &Path) -> String {
    let p = exe.to_string_lossy();
    format!("'{}' {}", p.replace('\'', r"'\''"), SUBCOMMAND)
}

/// Where this process's own binary is, which is what gets installed.
///
/// Separate from [`hook_command`] so that every test can supply a path and
/// none of them depends on where the test harness happens to live.
///
/// `current_exe` can fail -- on a deleted or replaced binary -- and the
/// failure is reported rather than papered over with a guess, because a
/// guessed path installs a hook that silently never runs.
pub fn current_exe() -> Result<PathBuf, Refusal> {
    std::env::current_exe().map_err(|e| Refusal::Io {
        path: "the Headstate binary".to_string(),
        detail: e.to_string(),
    })
}

/// The matcher object Headstate writes, marked as ours.
fn our_matcher(command: &str) -> Value {
    let mut hook = Map::new();
    hook.insert("type".to_string(), Value::String("command".to_string()));
    hook.insert("command".to_string(), Value::String(command.to_string()));

    let mut matcher = Map::new();
    matcher.insert("hooks".to_string(), Value::Array(vec![Value::Object(hook)]));
    matcher.insert(MARKER.to_string(), Value::from(1));
    Value::Object(matcher)
}

/// Whether a matcher is Headstate's -- the two predicates of §5.2.
///
/// This is the ONLY place ownership is decided, and the whole safety
/// property of the module rests here: a matcher this returns `false` for is
/// never touched.
///
/// # Why the second predicate matches the SUBCOMMAND, not the whole line
///
/// A byte-equality check against the line we would write now would miss
/// exactly the case the predicate exists for. The marker already handles
/// everything whose marker is intact; what is left is a matcher a user
/// hand-edited, and the plausible hand-edit is to the path (a moved app, a
/// corrected typo) or to the quoting. Requiring equality would leave that
/// matcher behind on uninstall -- which is the one outcome uninstall must
/// not have: a hook we no longer track, still running, still writing to
/// disk, with nothing in the UI admitting it exists.
///
/// So the test is "does this command line invoke our subcommand at all",
/// which is `claude-hook` appearing as a whole word. A foreign command that
/// merely contains the letters cannot match --
/// [`tests::a_foreign_command_that_merely_mentions_the_subcommand_is_not_ours`]
/// pins that with `my-claude-hooks-helper` and three others.
///
/// This also means ownership does not depend on WHICH binary path is
/// installed, which is why the callers do not pass one.
fn is_ours(matcher: &Value) -> bool {
    let Some(obj) = matcher.as_object() else {
        return false;
    };
    if obj.contains_key(MARKER) {
        return true;
    }
    obj.get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(mentions_our_subcommand)
            })
        })
}

/// Whether a command line invokes `headstate claude-hook`.
///
/// A whole-word match on the subcommand, so a foreign tool whose name
/// merely contains the letters is not claimed as ours. Word boundaries are
/// judged on shell-ish separators rather than a regex, because adding a
/// regex engine for one predicate is not a trade worth making.
fn mentions_our_subcommand(command: &str) -> bool {
    let needle = SUBCOMMAND;
    let boundary = |c: Option<char>| match c {
        None => true,
        // `-` is deliberately NOT a boundary: it is what makes
        // `my-claude-hooks-helper` and `claude-hook-wrapper` foreign
        // rather than ours.
        Some(c) => !c.is_alphanumeric() && c != '-' && c != '_',
    };
    let bytes = command.as_bytes();
    let mut from = 0;
    while let Some(i) = command[from..].find(needle).map(|i| i + from) {
        let before = command[..i].chars().next_back();
        let after = command[i + needle.len()..].chars().next();
        if boundary(before) && boundary(after) {
            return true;
        }
        // Advance by one BYTE boundary past the match start. `needle` is
        // ASCII so `i + 1` is always a char boundary here, but step by the
        // needle's length when we can to avoid rescanning.
        from = i + 1;
        if from > bytes.len() {
            break;
        }
    }
    false
}

/// Read and parse the settings file, or say precisely why not.
///
/// `Ok(None)` is the file-does-not-exist case, which is normal: a user who
/// has never written a setting has no file, and creating one with just our
/// hooks is the right outcome rather than an error (§5.4).
fn read_settings(path: &Path) -> Result<Option<Value>, Refusal> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Refusal::Io {
                path: path.display().to_string(),
                detail: e.to_string(),
            })
        }
    };
    // An EMPTY file is treated as absent rather than as malformed. It is
    // not valid JSON, but it is also not a file with settings in it that a
    // rewrite would discard -- which is the entire reason malformed is
    // refused. Claude Code ignores it either way, so writing our hooks into
    // it strictly improves matters and destroys nothing.
    if body.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&body).map(Some).map_err(|e| {
        // The parse error verbatim, line and column included. This message
        // reaches the user, and it is the only actionable thing we have.
        Refusal::Malformed {
            path: path.display().to_string(),
            detail: e.to_string(),
        }
    })
}

/// Borrow the `hooks` object, refusing a `hooks` that is something else.
///
/// A missing `hooks` is created; a `hooks` that is an array, a string or
/// `null` is refused for the same reason malformed JSON is -- we would have
/// to discard whatever the user meant by it.
fn hooks_mut<'a>(root: &'a mut Value, path: &Path) -> Result<&'a mut Map<String, Value>, Refusal> {
    // The ROOT not being an object is the same class of problem as a
    // non-object `hooks`: there is nowhere to put a `hooks` key without
    // replacing what is there. Reported through the same variant rather than
    // a fifth one, because the remedy and the sentence are identical.
    //
    // Named BEFORE the mutable borrow, because `as_object_mut` borrows the
    // value the message wants to describe.
    let kind = type_name(root);
    let obj = root
        .as_object_mut()
        .ok_or_else(|| Refusal::HooksNotAnObject {
            path: path.display().to_string(),
            found: format!("inside a top-level {kind} rather than an object"),
        })?;
    match obj.entry("hooks".to_string()) {
        serde_json::map::Entry::Vacant(v) => Ok(v
            .insert(Value::Object(Map::new()))
            .as_object_mut()
            .expect("just inserted an object")),
        serde_json::map::Entry::Occupied(o) => {
            let v = o.into_mut();
            if v.is_object() {
                Ok(v.as_object_mut().expect("checked is_object"))
            } else {
                Err(Refusal::HooksNotAnObject {
                    path: path.display().to_string(),
                    found: type_name(v).to_string(),
                })
            }
        }
    }
}

/// A JSON value's kind, for a message a user can act on.
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a true/false value",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// Drop every matcher of ours from one event's array, refusing a shape we
/// cannot classify.
///
/// Returns whether anything was dropped. An event whose array becomes empty
/// has its KEY removed too, so an uninstall leaves no `"SessionStart": []`
/// behind -- that is what makes the round-trip in the headline test
/// byte-identical rather than merely equivalent.
fn drop_ours(hooks: &mut Map<String, Value>, event: &str, path: &Path) -> Result<bool, Refusal> {
    let Some(slot) = hooks.get_mut(event) else {
        return Ok(false);
    };
    let Some(list) = slot.as_array_mut() else {
        // `hooks.SessionStart` present but not a list. Same refusal as a
        // non-object `hooks`: we cannot append to it and we will not
        // replace it.
        return Err(Refusal::MatcherNotUnderstood {
            path: path.display().to_string(),
            event: event.to_string(),
        });
    };
    // Refuse BEFORE mutating anything, so a document with one entry we do
    // not understand is left exactly as it was rather than half-edited.
    for m in list.iter() {
        if m.as_object()
            .is_none_or(|o| o.get("hooks").is_some_and(|h| !h.is_array()))
        {
            return Err(Refusal::MatcherNotUnderstood {
                path: path.display().to_string(),
                event: event.to_string(),
            });
        }
    }
    let before = list.len();
    list.retain(|m| !is_ours(m));
    let dropped = list.len() != before;
    if list.is_empty() {
        hooks.remove(event);
    }
    Ok(dropped)
}

/// Write `root` to `path` atomically, refusing to ship a document that does
/// not parse back.
///
/// # The re-parse is the point
///
/// A file we cannot re-parse is a file Claude Code will ignore in silence
/// (§1.7), which would make this whole feature inert while reporting
/// success. So the serialised bytes are parsed again BEFORE the rename, and
/// a failure leaves the original file exactly as it was.
///
/// This guard cannot fire today -- `serde_json` does not emit invalid JSON
/// from a `Value`. It is here because the cost is one parse of a 9KB file
/// and the failure it guards against is invisible: if a later change writes
/// the document some other way (a formatter, a hand-built string, an
/// `unsafe` shortcut for speed), this is what catches it at the boundary
/// rather than on a user's machine three weeks later.
///
/// # Temp-and-rename, in the same directory
///
/// Same directory because `rename(2)` is only atomic within a filesystem,
/// and `/tmp` is routinely a different one on macOS. `sync_all` before the
/// rename so a power loss cannot leave a renamed-but-empty file -- which
/// would be the malformed state this module refuses to create.
fn write_atomically(path: &Path, root: &Value) -> Result<(), Refusal> {
    let io = |e: std::io::Error| Refusal::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    };

    // Two spaces, and a trailing newline: what an editor and every other
    // tool that has touched this file will have left. Pretty-printed rather
    // than compact because a user reads and hand-edits this file -- the
    // refusal path above tells them to.
    let mut body = serde_json::to_vec_pretty(root).map_err(|e| Refusal::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    body.push(b'\n');

    if let Err(e) = serde_json::from_slice::<Value>(&body) {
        return Err(Refusal::Malformed {
            path: path.display().to_string(),
            detail: format!(
                "Headstate built a settings document it could not read back ({e}), \
                 so it wrote nothing. The file on disk is unchanged."
            ),
        });
    }

    let dir = path.parent().ok_or_else(|| Refusal::Io {
        path: path.display().to_string(),
        detail: "has no parent directory".to_string(),
    })?;
    std::fs::create_dir_all(dir).map_err(io)?;

    // A fixed name rather than a random one: two concurrent installs are
    // not a case worth a random suffix, and a predictable name is one a
    // user who finds it can understand. The rename replaces it either way.
    let tmp = path.with_extension("json.headstate-tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(&body).map_err(io)?;
        // Before the rename, not after: the rename is what publishes the
        // file, and publishing bytes that are still only in the page cache
        // is how a power loss produces the malformed file above.
        f.sync_all().map_err(io)?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        // A failed rename leaves the temp file behind, and a stray
        // `settings.json.headstate-tmp` next to a settings file is
        // confusing at best. Best-effort: the rename error is what gets
        // reported either way.
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

/// Install (or reinstall) the hooks, in one atomic write.
///
/// Idempotent: this drops every matcher it recognises as ours and appends
/// exactly one fresh matcher per event in [`EVENTS`]. So "install" and
/// "reinstall" are the same operation, which is what §5.3 settled -- there
/// is no second code path to keep in step, and running it twice leaves one
/// matcher rather than two.
///
/// Everything else in the document is preserved, including hook matchers
/// belonging to other tools. See the module docs for why that is the
/// property this module exists to have.
pub fn install(path: &Path, exe: &Path) -> Result<Installed, Refusal> {
    install_events(path, exe, EVENTS)
}

/// [`install`], with the event list as a parameter.
///
/// The parameter exists for ONE reason, and it is the reason #1061 asks
/// for: "adding an event is additive" is a claim about a list that is
/// longer than today's, and the only honest way to test it is to install a
/// longer one. Doing that by adding a real event to [`EVENTS`] would ship
/// an event whose semantics belong to a sub-issue that has not been
/// written yet -- so the mechanism is demonstrated against a test fixture
/// instead, which is what the issue asks for.
///
/// Not `pub`: nothing outside this module may choose its own event list.
/// The events Headstate installs are [`EVENTS`] and the argument for each
/// of them lives there.
fn install_events(path: &Path, exe: &Path, events: &[&str]) -> Result<Installed, Refusal> {
    let command = hook_command(exe);
    let existing = read_settings(path)?;
    let created_file = existing.is_none();
    let mut root = existing.unwrap_or_else(|| Value::Object(Map::new()));

    let mut out = Installed {
        command: command.clone(),
        created_file,
        ..Installed::default()
    };
    {
        let hooks = hooks_mut(&mut root, path)?;
        for event in events {
            // Drop first, then append. A matcher of ours that is already
            // correct is still rewritten -- which costs nothing and means
            // there is exactly one code path, so a "already fine" branch
            // cannot drift from the one that repairs.
            let replaced = drop_ours(hooks, event, path)?;
            if replaced {
                out.replaced.push((*event).to_string());
            } else {
                out.added.push((*event).to_string());
            }
            hooks
                .entry((*event).to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                // `drop_ours` has already refused a non-array under this
                // event, so an array is what is here.
                .expect("drop_ours refuses a non-array under an event")
                .push(our_matcher(&command));
        }
    }

    write_atomically(path, &root)?;
    Ok(out)
}

/// Remove Headstate's hooks and nothing else.
///
/// Writes only when something was actually removed: an uninstall of
/// something already absent must not reformat a user's file as a side
/// effect. That is also what makes [`Uninstalled::was_absent`] honest
/// rather than cosmetic.
///
/// # Why this takes no binary path
///
/// It does not need one, and taking one would be actively wrong. Ownership
/// is decided by [`is_ours`], which asks about the marker and about the
/// subcommand -- neither of which depends on WHICH binary path is in the
/// matcher. An uninstall keyed on today's path would strand the matcher of
/// an app that has since moved, which is precisely the hook that most needs
/// removing: it is still running, and its path may not even exist.
pub fn uninstall(path: &Path) -> Result<Uninstalled, Refusal> {
    let Some(mut root) = read_settings(path)? else {
        // No file means nothing of ours is installed. Not an error.
        return Ok(Uninstalled {
            removed: Vec::new(),
            was_absent: true,
        });
    };

    let mut removed = Vec::new();
    {
        let hooks = hooks_mut(&mut root, path)?;
        // EVERY event, not just `EVENTS`. An older version of Headstate
        // could have installed a hook under an event this version no longer
        // uses, and an uninstall that only looked at today's list would
        // leave it behind -- still running, still writing, no longer
        // tracked by anything. The marker is what makes this safe to do
        // across events we never wrote.
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            if drop_ours(hooks, &event, path)? {
                removed.push(event);
            }
        }
        // An emptied `hooks` object is removed too, so an install-then-
        // uninstall on a file that had no hooks at all restores the
        // document byte for byte rather than leaving `"hooks": {}`.
        if hooks.is_empty() {
            root.as_object_mut()
                .expect("hooks_mut proved the root is an object")
                .remove("hooks");
        }
    }

    if removed.is_empty() {
        return Ok(Uninstalled {
            removed,
            was_absent: true,
        });
    }
    write_atomically(path, &root)?;
    Ok(Uninstalled {
        removed,
        was_absent: false,
    })
}

/// What the file says right now.
///
/// READ every time, never cached (§5.5). A cached "installed" is wrong the
/// moment the user edits the file by hand, and this is a file we have
/// invited them to edit -- the malformed refusal tells them to.
///
/// Never returns an error: every failure is a [`Status::CannotTell`]
/// carrying the reason. That is deliberate. A `Result` here would let a
/// caller write `.unwrap_or(NotInstalled)` and collapse "cannot tell" into
/// "not installed", which is the one confusion §5.5 names -- the remedies
/// are completely different, and one of them is "do not click Install".
pub fn status(path: &Path, exe: &Path) -> Status {
    let command = hook_command(exe);
    let root = match read_settings(path) {
        Ok(Some(root)) => root,
        // No file, or an empty one: nothing of ours is installed, and that
        // is a fact rather than a failure to determine one.
        Ok(None) => return Status::NotInstalled,
        Err(e) => return Status::CannotTell(e),
    };

    let hooks = match root.get("hooks") {
        None => return Status::NotInstalled,
        Some(h) => match h.as_object() {
            Some(o) => o,
            None => {
                return Status::CannotTell(Refusal::HooksNotAnObject {
                    path: path.display().to_string(),
                    found: type_name(h).to_string(),
                })
            }
        },
    };

    // Every event carrying a matcher of ours, and every command line those
    // matchers run. Both are needed: the events say whether the install is
    // complete, and the commands say whether it is current.
    let mut events_with_ours: BTreeSet<&str> = BTreeSet::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut extra_events: BTreeSet<&str> = BTreeSet::new();
    let mut duplicates: BTreeSet<&str> = BTreeSet::new();

    for (event, slot) in hooks {
        let Some(list) = slot.as_array() else {
            return Status::CannotTell(Refusal::MatcherNotUnderstood {
                path: path.display().to_string(),
                event: event.clone(),
            });
        };
        let mine: Vec<&Value> = list.iter().filter(|m| is_ours(m)).collect();
        if mine.is_empty() {
            continue;
        }
        if mine.len() > 1 {
            duplicates.insert(event.as_str());
        }
        if EVENTS.contains(&event.as_str()) {
            events_with_ours.insert(event.as_str());
        } else {
            // Ours, under an event this version does not install. A stale
            // state rather than an installed one: `uninstall` will clear
            // it, and a reinstall is how a user gets there.
            extra_events.insert(event.as_str());
        }
        for m in mine {
            let cmds = m.get("hooks").and_then(Value::as_array);
            for c in cmds.into_iter().flatten() {
                if let Some(s) = c.get("command").and_then(Value::as_str) {
                    found.insert(s.to_string());
                }
            }
        }
    }

    if found.is_empty() {
        return Status::NotInstalled;
    }

    let missing: Vec<&str> = EVENTS
        .iter()
        .copied()
        .filter(|e| !events_with_ours.contains(e))
        .collect();

    // Each of these is a DIFFERENT repair story, so each gets its own
    // sentence rather than one "needs reinstalling".
    let mut problems: Vec<String> = Vec::new();
    if !missing.is_empty() {
        problems.push(format!(
            "no hook is installed for {} — sessions started this way are \
             recorded only from the transcript, without a process id",
            missing.join(" or ")
        ));
    }
    if !duplicates.is_empty() {
        problems.push(format!(
            "{} has more than one Headstate hook, so each session is recorded twice",
            duplicates.iter().copied().collect::<Vec<_>>().join(" and ")
        ));
    }
    if !extra_events.is_empty() {
        problems.push(format!(
            "a Headstate hook is installed for {}, which this version no longer uses",
            extra_events
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .join(" and ")
        ));
    }
    let wrong: Vec<&String> = found.iter().filter(|c| *c != &command).collect();
    if !wrong.is_empty() {
        problems.push(format!(
            "the installed command is {} rather than {command} — most likely \
             the app has moved since it was installed, so the hook is running \
             a path that may no longer exist",
            wrong
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if problems.is_empty() {
        Status::Installed { command }
    } else {
        Status::Stale {
            detail: problems.join("; "),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary path every test installs, chosen to look like the real
    /// one without being it.
    const EXE: &str = "/Applications/Headstate.app/Contents/MacOS/headstate";

    /// The foreign hook that is really on the development machine, and the
    /// reason this module appends rather than assigns.
    ///
    /// Measured from `~/.claude/settings.json`: ten hook events pointing at
    /// this binary, plus `codegraph prompt-hook` under `UserPromptSubmit`.
    /// This fixture is that file's SHAPE with its personal content removed
    /// -- the same matcher structure, the same two-entry
    /// `UserPromptSubmit`, the same sibling top-level keys.
    ///
    /// It carries a foreign hook under EVERY event in [`EVENTS`], which is
    /// a property the tests below depend on rather than an accident: the
    /// claim they make is that installing APPENDS to somebody else's
    /// array instead of assigning over it, and an event where the fixture
    /// had no foreign hook could not distinguish the two. #1062-#1064
    /// added three events and two of them (`PostToolUseFailure`,
    /// `PermissionDenied`) had to be added here for that reason.
    const CC_STATUS: &str = "/Users/acme/.config/iterm2/cc-status";

    /// A fixture whose hooks are all somebody else's.
    ///
    /// Pretty-printed with two spaces and a trailing newline, which is what
    /// [`write_atomically`] emits -- so the round-trip assertion below is a
    /// genuine BYTE comparison rather than one that would pass on any
    /// formatting.
    fn foreign_fixture() -> String {
        let mut hooks = serde_json::Map::new();
        for event in [
            "Notification",
            "PermissionDenied",
            "PermissionRequest",
            "PostToolUse",
            "PostToolUseFailure",
            "PreToolUse",
            "SessionEnd",
            "SessionStart",
            "Stop",
            "StopFailure",
            "SubagentStop",
        ] {
            hooks.insert(
                event.to_string(),
                serde_json::json!([{ "hooks": [{ "command": CC_STATUS, "type": "command" }] }]),
            );
        }
        // TWO matchers, as the real file has: the clobber this module
        // refuses is losing one of several, not just losing one.
        hooks.insert(
            "UserPromptSubmit".to_string(),
            serde_json::json!([
                { "hooks": [{ "command": "codegraph prompt-hook", "type": "command" }] },
                { "hooks": [{ "command": CC_STATUS, "type": "command" }] },
            ]),
        );
        let root = serde_json::json!({
            "alwaysThinkingEnabled": true,
            "hooks": Value::Object(hooks),
            "permissions": { "allow": ["Bash(git status:*)"] },
            "statusLine": { "type": "command", "command": CC_STATUS },
        });
        let mut s = serde_json::to_string_pretty(&root).unwrap();
        s.push('\n');
        s
    }

    /// A settings file in a scratch home, so nothing here can reach the
    /// developer's own `~/.claude/settings.json`.
    fn scratch(body: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::TempDir::new().unwrap();
        let path = settings_path_in(home.path());
        if let Some(b) = body {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b).unwrap();
        }
        (home, path)
    }

    /// What [`foreign_fixture`] should look like under one of OUR events
    /// after an install: the foreign matcher first where the fixture had
    /// one, then ours.
    ///
    /// A helper rather than a literal in each test because the two sets
    /// stopped coinciding at #1065/#1066. The fixture mirrors the real
    /// development machine, which runs `cc-status` under nine events;
    /// `EVENTS` now includes `PreCompact` and `SubagentStart`, which no
    /// tool on that machine hooks. Hard-coding `[cc-status, ours]` for
    /// every event would assert a neighbour the real file does not have,
    /// and relaxing to "ours is in there somewhere" would stop testing
    /// the clobber this module exists to prevent. So the expectation is
    /// derived from the fixture itself.
    fn expected_after_install(event: &str, ours: &str) -> Vec<String> {
        if foreign_fixture().contains(&format!("\"{event}\"")) {
            vec![CC_STATUS.to_string(), ours.to_string()]
        } else {
            vec![ours.to_string()]
        }
    }

    fn commands_under(path: &Path, event: &str) -> Vec<String> {
        let root: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        root["hooks"][event]
            .as_array()
            .map(|l| {
                l.iter()
                    .flat_map(|m| m["hooks"].as_array().cloned().unwrap_or_default())
                    .map(|h| h["command"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------
    // The headline test (#915, epic §5.1).
    // -----------------------------------------------------------------

    /// THE test this issue is about: a foreign hook survives, and the
    /// document comes back byte for byte.
    ///
    /// Both halves matter and neither implies the other. Survival alone
    /// would pass for an installer that reordered or reformatted the
    /// document, which would produce a diff in a user's version-controlled
    /// dotfiles on every install. A byte round-trip alone would pass for an
    /// installer that did nothing at all -- which is why the middle of this
    /// test asserts the install really happened.
    ///
    /// PROVEN BY SABOTAGE. Replacing the append in `install` with an
    /// assignment (`hooks.insert(event, json!([our_matcher(&command)]))`)
    /// fails here, at the middle assertion, with the foreign command gone
    /// from `SessionStart`:
    ///
    /// ```text
    /// assertion `left == right` failed: replacing the array rather than
    ///   appending to it destroys another tool's hook -- the development
    ///   machine really has cc-status under all ten events
    ///   left: ["'/Applications/Headstate.app/Contents/MacOS/headstate' claude-hook"]
    ///  right: ["/Users/acme/.config/iterm2/cc-status",
    ///          "'/Applications/Headstate.app/Contents/MacOS/headstate' claude-hook"]
    /// ```
    #[test]
    fn install_then_uninstall_leaves_a_foreign_hook_and_the_document_untouched() {
        let before = foreign_fixture();
        let (_home, path) = scratch(Some(&before));
        let exe = Path::new(EXE);

        let installed = install(&path, exe).unwrap();
        assert_eq!(installed.added, EVENTS.to_vec());

        // The foreign entry is STILL THERE, beside ours, under every event
        // we installed into that HAD one.
        //
        // Split on whether the fixture carried a foreign matcher rather
        // than asserting one shape for all of `EVENTS`: the fixture is a
        // copy of the real development machine, which has `cc-status`
        // under nine events, and #1065/#1066 install two (`PreCompact`,
        // `SubagentStart`) that no tool on that machine uses. Asserting
        // `[cc-status, ours]` everywhere would demand a foreign hook the
        // real file does not have; asserting only `contains(ours)` would
        // stop testing the clobber. So each event is checked against what
        // was actually there before.
        for event in EVENTS {
            assert_eq!(
                commands_under(&path, event),
                expected_after_install(event, &hook_command(exe)),
                "replacing the array rather than appending to it destroys \
                 another tool's hook -- the development machine really has \
                 cc-status under nine events"
            );
        }
        // And every event we did NOT install into is exactly as it was,
        // including the two-matcher one.
        assert_eq!(
            commands_under(&path, "UserPromptSubmit"),
            vec!["codegraph prompt-hook".to_string(), CC_STATUS.to_string()]
        );
        // `Notification` is NOT in this list any more: #1067 installs it,
        // so it is covered by the loop above instead. `SubagentStop` still
        // is, and deliberately -- #1066 installs `SubagentStart` and not
        // its pair (see `hook::subagents`), so a foreign hook on the stop
        // event is exactly the kind of neighbour this test protects.
        for event in ["PreToolUse", "Stop", "SubagentStop"] {
            assert_eq!(commands_under(&path, event), vec![CC_STATUS.to_string()]);
        }

        let removed = uninstall(&path).unwrap();
        // Compared as a SET: `uninstall` sweeps the document's own `hooks`
        // keys (see its docs for why it does not iterate `EVENTS`), so the
        // order it reports follows the file rather than this list.
        let got: BTreeSet<&str> = removed.removed.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            EVENTS.iter().copied().collect::<BTreeSet<_>>(),
            "uninstall reports every event it took a matcher out of"
        );

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after, before,
            "install-then-uninstall must restore the document BYTE for byte: \
             a user whose dotfiles are in git should see no diff at all"
        );
    }

    /// The round-trip above, with the ten real events and the counts stated
    /// the way the epic's own measurement states them -- so the numbers in
    /// §5.1 are pinned by a test rather than only recorded in prose.
    #[test]
    fn the_foreign_matcher_counts_go_up_by_one_and_back_down() {
        let (_home, path) = scratch(Some(&foreign_fixture()));
        let exe = Path::new(EXE);

        let count = |event: &str| commands_under(&path, event).len();
        assert_eq!((count("SessionStart"), count("UserPromptSubmit")), (1, 2));

        install(&path, exe).unwrap();
        assert_eq!((count("SessionStart"), count("UserPromptSubmit")), (2, 2));

        uninstall(&path).unwrap();
        assert_eq!((count("SessionStart"), count("UserPromptSubmit")), (1, 2));
        assert!(
            commands_under(&path, "SessionStart").contains(&CC_STATUS.to_string()),
            "cc-status must have survived the whole round trip"
        );
    }

    /// The round trip on a file that had NO hooks at all: the `hooks` key
    /// itself must be gone again afterwards.
    ///
    /// This test exists because SABOTAGE FOUND THE GAP, which is the reason
    /// worth recording. Removing the empty-array cleanup from `drop_ours`
    /// (`if list.is_empty() { hooks.remove(event) }`) left all 29 other
    /// tests GREEN -- including the headline round-trip, because its fixture
    /// has a foreign `cc-status` entry under both events, so the array never
    /// empties and the leftover `"SessionStart": []` never appears.
    ///
    /// So the headline test proves the foreign entry survives, and proves
    /// nothing about the user who has no hooks yet. For them, the sabotaged
    /// code leaves this in a file that was previously clean:
    ///
    /// ```json
    /// "hooks": { "SessionStart": [], "SessionEnd": [] }
    /// ```
    ///
    /// Which is not a crash and not data loss -- it is a permanent diff in a
    /// file a lot of people keep in git, left behind by an uninstall that
    /// claimed to have removed everything. With the cleanup restored this
    /// test fails on the sabotage and the two together cover both shapes.
    #[test]
    fn a_round_trip_on_a_file_with_no_hooks_removes_the_hooks_key_again() {
        let before = "{\n  \"alwaysThinkingEnabled\": true,\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(git status:*)\"\n    ]\n  }\n}\n";
        let (_home, path) = scratch(Some(before));
        let exe = Path::new(EXE);

        install(&path, exe).unwrap();
        // The install really happened, or the round-trip below is vacuous.
        assert_eq!(
            commands_under(&path, "SessionStart"),
            vec![hook_command(exe)]
        );

        uninstall(&path).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "an uninstall must not leave `\"hooks\": {{}}` or an empty event \
             array behind: a lot of people keep this file in git, and a \
             permanent diff from an operation that claimed to remove \
             everything is the whole complaint"
        );
    }

    /// A document whose keys are NOT in alphabetical order round-trips with
    /// that order intact.
    ///
    /// This test exists because SABOTAGE FOUND A SECOND GAP, and this one is
    /// a dependency rather than a line of code. `serde_json` is declared with
    /// the `preserve_order` feature in `Cargo.toml`; without it a `Value`
    /// object is a `BTreeMap` and every write SORTS the document's keys.
    ///
    /// Removing the feature left all 30 other tests GREEN, for a reason
    /// worth writing down: Claude Code writes `~/.claude/settings.json`
    /// with its keys already sorted -- measured on the real file today,
    /// twelve top-level keys and ten hook events, both exactly
    /// alphabetical -- and the fixtures here were built to match it. So
    /// every round-trip assertion compared a sorted document against a
    /// sorted document and could not see the difference.
    ///
    /// A HAND-EDITED file is the case that breaks. Someone who added
    /// `"hooks"` at the top of their file, or who keeps `permissions` last
    /// because that is how they think about it, would have their whole
    /// settings file reordered by an install -- one line changed by us, a
    /// hundred by the serialiser, in a file people keep in version control.
    ///
    /// It also matters for our OWN matcher: `hooks` before `_headstate` is
    /// insertion order, and sorting puts the underscore key first.
    ///
    /// Restoring `preserve_order` makes this pass; removing it fails here
    /// with the keys reordered.
    #[test]
    fn a_hand_ordered_document_keeps_its_key_order() {
        // Deliberately NOT alphabetical, at all three levels: top-level
        // (`permissions` after `hooks` is alphabetical, so `zed` first is
        // the giveaway), the hook events, and the matcher's own keys.
        let before = "{\n  \"zzzLastByHand\": true,\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"/Users/acme/.config/iterm2/cc-status\"\n          }\n        ]\n      }\n    ],\n    \"Notification\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"/Users/acme/.config/iterm2/cc-status\"\n          }\n        ]\n      }\n    ]\n  },\n  \"alwaysThinkingEnabled\": true\n}\n";
        let (_home, path) = scratch(Some(before));
        let exe = Path::new(EXE);

        install(&path, exe).unwrap();

        // Ours is appended and the order of everything else is untouched.
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.find("\"zzzLastByHand\"").unwrap() < after.find("\"hooks\"").unwrap(),
            "a sorting serialiser would have moved `zzzLastByHand` to the \
             end, rewriting a file the user hand-ordered:\n{after}"
        );
        assert!(
            after.find("\"SessionStart\"").unwrap() < after.find("\"Notification\"").unwrap(),
            "the hook events were reordered:\n{after}"
        );
        // Our own matcher too. `our_matcher` inserts `hooks` and then
        // `_headstate`, and each command object gets `type` then `command`;
        // a sorting serialiser reverses both pairs.
        let marker = after.rfind(MARKER).expect("the marker must be written");
        let our_hooks = after[..marker]
            .rfind("\"hooks\"")
            .expect("our matcher must carry a hooks key before the marker");
        assert!(
            our_hooks < marker,
            "our matcher's keys were reordered -- `_headstate` sorts before \
             `hooks`:\n{after}"
        );
        // Our command object sits between our matcher's `hooks` key and the
        // marker that follows it.
        let ours = &after[our_hooks..marker];
        let our_type = ours
            .find("\"type\"")
            .expect("our command object must have a type");
        let our_command = ours
            .find("\"command\"")
            .expect("our command object must have a command");
        assert!(
            our_type < our_command,
            "our command object's keys were reordered -- `command` sorts \
             before `type`:\n{after}"
        );

        uninstall(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "the round trip must restore a hand-ordered document exactly"
        );
    }

    // -----------------------------------------------------------------
    // The malformed file: refused, never rewritten (#915, epic §5.4).
    // -----------------------------------------------------------------

    /// A file that is not valid JSON is REFUSED, and the bytes on disk are
    /// unchanged afterwards.
    ///
    /// The second assertion is the one that matters. Refusing while having
    /// already written a temp file over the original would be the same
    /// defect with a better error message.
    ///
    /// PROVEN BY SABOTAGE. Making `read_settings` fall back to an empty
    /// document on a parse error (`.unwrap_or(Value::Object(Map::new()))`
    /// instead of the `Err`) fails here:
    ///
    /// ```text
    /// thread 'claude::install::tests::a_malformed_settings_file_is_refused_and_not_rewritten'
    ///   panicked at src/claude/install.rs:
    ///   a malformed file must be REFUSED: Claude Code ignores it silently,
    ///   so the user's every hook is already dead and a rewrite would
    ///   discard the settings we could not parse -- got
    ///   Ok(Installed { command: "...", added: ["SessionStart", "SessionEnd"], .. })
    /// ```
    ///
    /// and the file on disk had become a document containing only our
    /// hooks, with `permissions` and `statusLine` gone.
    #[test]
    fn a_malformed_settings_file_is_refused_and_not_rewritten() {
        // Realistic damage rather than gibberish: a trailing comma and a
        // truncated object, which is what a hand edit actually produces.
        let broken = r#"{
  "permissions": { "allow": ["Bash(git status:*)"], },
  "hooks": {
    "SessionStart": [ { "hooks": [ { "type": "command", "command": "/usr/local/bin/cc-status" } ] } ]
"#;
        let (_home, path) = scratch(Some(broken));
        let exe = Path::new(EXE);

        let refused = install(&path, exe);
        match &refused {
            Err(Refusal::Malformed { path: p, detail }) => {
                assert!(p.ends_with("settings.json"), "got {p}");
                // serde's own message, with a position in it. This is what
                // the user needs in order to fix the file by hand, which is
                // the only remedy we offer.
                assert!(
                    detail.contains("line") && detail.contains("column"),
                    "the refusal must quote serde's position so the user can \
                     find the damage: {detail}"
                );
            }
            other => panic!(
                "a malformed file must be REFUSED: Claude Code ignores it \
                 silently, so the user's every hook is already dead and a \
                 rewrite would discard the settings we could not parse -- \
                 got {other:?}"
            ),
        }

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "a refusal that has already overwritten the file is the same \
             defect with a nicer message"
        );
        // And the temp file is not left lying beside it.
        assert!(!path.with_extension("json.headstate-tmp").exists());
    }

    /// Uninstall refuses a malformed file too, for the same reason.
    ///
    /// Worth its own test because the tempting shortcut differs: an
    /// uninstall "only removes things", so treating an unparseable file as
    /// "nothing of ours is there" looks harmless. It is not -- our hook may
    /// well be in that file, still running, and reporting it gone would be
    /// a lie the user acts on.
    #[test]
    fn uninstall_refuses_a_malformed_file_rather_than_reporting_success() {
        let broken = "{ this is not json }";
        let (_home, path) = scratch(Some(broken));

        let r = uninstall(&path);

        assert!(
            matches!(r, Err(Refusal::Malformed { .. })),
            "an unparseable file may well contain our hook; reporting it \
             removed would be a lie the user acts on -- got {r:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    /// Status on a malformed file is `CannotTell`, NOT `NotInstalled`.
    ///
    /// This is the distinction §5.5 exists for. `NotInstalled` invites a
    /// click on Install, and for a malformed file that is exactly the click
    /// that has to be refused -- so rendering the two the same would send
    /// the user in a loop with no explanation.
    #[test]
    fn status_on_a_malformed_file_cannot_tell_rather_than_reporting_absent() {
        let (_home, path) = scratch(Some("{ \"hooks\": "));

        match status(&path, Path::new(EXE)) {
            Status::CannotTell(Refusal::Malformed { detail, .. }) => {
                assert!(!detail.is_empty());
            }
            other => panic!(
                "a file we cannot parse must not render as 'not installed': \
                 the remedy is an editor, not the Install button -- got {other:?}"
            ),
        }
    }

    /// A `hooks` key that is not an object is refused rather than replaced.
    #[test]
    fn a_non_object_hooks_key_is_refused() {
        let (_home, path) = scratch(Some(r#"{"hooks": ["SessionStart"]}"#));

        let r = install(&path, Path::new(EXE));

        match r {
            Err(Refusal::HooksNotAnObject { found, .. }) => assert_eq!(found, "a list"),
            other => panic!("got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"hooks": ["SessionStart"]}"#
        );
    }

    /// A settings file whose ROOT is not an object is refused.
    ///
    /// Valid JSON, so `read_settings` passes it through happily -- and then
    /// there is nowhere to put a `hooks` key without replacing what is
    /// there, which is the same refusal a non-object `hooks` gets. Worth a
    /// test because the alternative is not a wrong answer but a PANIC: an
    /// `as_object_mut().unwrap()` here would take down the command.
    #[test]
    fn a_settings_file_that_is_not_an_object_is_refused() {
        for body in ["[1, 2, 3]", "\"a string\"", "42", "null"] {
            let (_home, path) = scratch(Some(body));

            let r = install(&path, Path::new(EXE));

            assert!(
                matches!(r, Err(Refusal::HooksNotAnObject { .. })),
                "{body} must be refused rather than panicking or being \
                 replaced -- got {r:?}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
            // And the same for status, which must say cannot-tell rather
            // than reporting the hook absent.
            assert!(
                matches!(
                    status(&path, Path::new(EXE)),
                    Status::NotInstalled | Status::CannotTell(_)
                ),
                "status must not claim anything is installed in {body}"
            );
        }
    }

    /// A matcher whose `hooks` key is not an array is refused, and refused
    /// BEFORE anything is written -- including the other event, which
    /// `install` would otherwise have already appended to.
    #[test]
    fn an_unintelligible_matcher_is_refused_without_a_partial_write() {
        let body = r#"{
  "hooks": {
    "SessionStart": [ { "hooks": "not-an-array" } ],
    "SessionEnd": [ { "hooks": [ { "type": "command", "command": "/bin/true" } ] } ]
  }
}
"#;
        let (_home, path) = scratch(Some(body));

        let r = install(&path, Path::new(EXE));

        match r {
            Err(Refusal::MatcherNotUnderstood { event, .. }) => {
                assert_eq!(event, "SessionStart")
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            body,
            "a refusal must leave the document exactly as it was, not \
             half-edited with SessionEnd already done"
        );
    }

    // -----------------------------------------------------------------
    // Ownership: the two predicates (epic §5.2).
    // -----------------------------------------------------------------

    /// The MARKER predicate on its own: a matcher whose command is nothing
    /// like ours is still ours if it carries the marker.
    ///
    /// This is the predicate that survives the app moving. Without it, a
    /// reinstall after a move to `/Applications` would leave the old
    /// matcher behind and the user would have two hooks, one pointing at a
    /// path that no longer exists.
    #[test]
    fn the_marker_alone_identifies_our_matcher() {
        let body = serde_json::json!({
            "hooks": { "SessionStart": [
                { "_headstate": 1, "hooks": [{ "type": "command", "command": "/old/path/that/moved --something" }] },
                { "hooks": [{ "type": "command", "command": CC_STATUS }] },
            ]}
        });
        let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

        let removed = uninstall(&path).unwrap();

        assert_eq!(removed.removed, vec!["SessionStart"]);
        assert_eq!(
            commands_under(&path, "SessionStart"),
            vec![CC_STATUS.to_string()],
            "the marker must identify a matcher whose path has gone stale, \
             or a reinstall after the app moves leaves a hook behind"
        );
    }

    /// The COMMAND predicate on its own: a matcher with no marker, whose
    /// command invokes our subcommand, is still ours.
    ///
    /// This is the predicate that catches a hand-edit. A user who deleted
    /// the odd-looking `_headstate` key while tidying their file must still
    /// be able to uninstall -- otherwise the leftover hook keeps writing to
    /// disk with nothing in the UI admitting it exists.
    #[test]
    fn the_command_alone_identifies_our_matcher() {
        let body = serde_json::json!({
            "hooks": { "SessionEnd": [
                { "hooks": [{ "type": "command", "command": "/somewhere/else/headstate claude-hook" }] },
                { "hooks": [{ "type": "command", "command": CC_STATUS }] },
            ]}
        });
        let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

        let removed = uninstall(&path).unwrap();

        assert_eq!(removed.removed, vec!["SessionEnd"]);
        assert_eq!(
            commands_under(&path, "SessionEnd"),
            vec![CC_STATUS.to_string()],
            "a user who deleted the marker while hand-editing must still be \
             able to uninstall, or the hook keeps running untracked"
        );
    }

    /// A foreign command that merely MENTIONS the subcommand is not ours.
    ///
    /// The command predicate is a whole-word match for exactly this reason.
    /// A substring test would claim someone else's tool and delete it on
    /// uninstall, which is the same clobber the append rule exists to
    /// prevent -- arriving from the other direction.
    #[test]
    fn a_foreign_command_that_merely_mentions_the_subcommand_is_not_ours() {
        for foreign in [
            "/usr/local/bin/my-claude-hooks-helper",
            "claude-hooks --all",
            "/opt/tools/claude-hook-wrapper run",
            "echo not_claude-hook_either",
        ] {
            let body = serde_json::json!({
                "hooks": { "SessionStart": [
                    { "hooks": [{ "type": "command", "command": foreign }] },
                ]}
            });
            let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

            let removed = uninstall(&path).unwrap();

            assert!(
                removed.was_absent,
                "{foreign} is somebody else's tool and must not be claimed as \
                 ours -- a substring match would delete it on uninstall"
            );
            assert_eq!(
                commands_under(&path, "SessionStart"),
                vec![foreign.to_string()]
            );
        }
    }

    /// Ours under an event this version no longer installs is still found
    /// and still removed.
    ///
    /// The case is a future one and the test is cheap: if `EVENTS` ever
    /// loses an entry, an uninstall that only looked at today's list would
    /// leave a live hook behind with nothing in the UI acknowledging it.
    #[test]
    fn uninstall_removes_ours_from_an_event_this_version_no_longer_installs() {
        let body = serde_json::json!({
            "hooks": { "StopFailure": [
                { "_headstate": 1, "hooks": [{ "type": "command", "command": hook_command(Path::new(EXE)) }] },
            ]}
        });
        let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

        let removed = uninstall(&path).unwrap();

        assert_eq!(removed.removed, vec!["StopFailure"]);
        assert!(!removed.was_absent);
    }

    // -----------------------------------------------------------------
    // Idempotence (epic §5.3).
    // -----------------------------------------------------------------

    /// Installing twice leaves ONE matcher per event, not two.
    ///
    /// `install` is the reinstall, which is what makes this true with one
    /// code path rather than an "already installed?" branch that could
    /// drift from the repair path.
    #[test]
    fn installing_twice_leaves_one_matcher_and_reports_the_replacement() {
        let (_home, path) = scratch(Some(&foreign_fixture()));
        let exe = Path::new(EXE);

        let first = install(&path, exe).unwrap();
        assert_eq!(first.replaced, Vec::<String>::new());

        let second = install(&path, exe).unwrap();

        assert_eq!(
            second.replaced,
            EVENTS.to_vec(),
            "a second install must REPORT that it replaced ours -- silently \
             reverting a hand-edit is its own defect (§5.4)"
        );
        assert_eq!(second.added, Vec::<String>::new());
        for event in EVENTS {
            assert_eq!(
                commands_under(&path, event),
                expected_after_install(event, &hook_command(exe)),
                "an install must be idempotent: twice means one matcher, not two"
            );
        }
    }

    /// A reinstall repairs a stale path, which is the case that actually
    /// happens -- the app moved to `/Applications` after a first run from
    /// `~/Downloads`.
    #[test]
    fn a_reinstall_repairs_a_stale_path() {
        let (_home, path) = scratch(Some(&foreign_fixture()));
        let old = Path::new("/Users/acme/Downloads/Headstate.app/Contents/MacOS/headstate");
        let new = Path::new(EXE);

        install(&path, old).unwrap();
        assert!(matches!(status(&path, new), Status::Stale { .. }));

        install(&path, new).unwrap();

        assert_eq!(
            status(&path, new),
            Status::Installed {
                command: hook_command(new)
            }
        );
        for event in EVENTS {
            assert_eq!(
                commands_under(&path, event),
                expected_after_install(event, &hook_command(new)),
                "the stale matcher must be gone, not sitting beside the new one"
            );
        }
    }

    /// An interrupted install that left a duplicate is repaired to one.
    #[test]
    fn a_reinstall_collapses_a_duplicate() {
        let ours = serde_json::json!({ "_headstate": 1, "hooks": [{ "type": "command", "command": hook_command(Path::new(EXE)) }] });
        let body = serde_json::json!({
            "hooks": { "SessionStart": [ours.clone(), ours.clone()], "SessionEnd": [ours] }
        });
        let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

        assert!(matches!(
            status(&path, Path::new(EXE)),
            Status::Stale { .. }
        ));
        install(&path, Path::new(EXE)).unwrap();

        assert_eq!(commands_under(&path, "SessionStart").len(), 1);
        assert_eq!(
            status(&path, Path::new(EXE)),
            Status::Installed {
                command: hook_command(Path::new(EXE))
            }
        );
    }

    // -----------------------------------------------------------------
    // The absent-file case (epic §5.4).
    // -----------------------------------------------------------------

    /// No settings file at all: created with just our hooks, not an error.
    #[test]
    fn an_absent_file_is_created_rather_than_refused() {
        let (_home, path) = scratch(None);

        let out = install(&path, Path::new(EXE)).unwrap();

        assert!(out.created_file);
        assert_eq!(out.added, EVENTS.to_vec());
        assert_eq!(
            status(&path, Path::new(EXE)),
            Status::Installed {
                command: hook_command(Path::new(EXE))
            },
            "status must READ the file back -- 'we wrote it' is not evidence \
             that Claude Code can parse it (§1.7)"
        );
    }

    /// An EMPTY file is the absent case, not the malformed one.
    ///
    /// Empty is not valid JSON, so the malformed rule would refuse it. But
    /// the reason malformed is refused is that a rewrite discards settings
    /// we could not parse -- and an empty file has none to discard. Claude
    /// Code ignores it either way, so writing our hooks strictly improves
    /// matters.
    #[test]
    fn an_empty_file_is_treated_as_absent() {
        let (_home, path) = scratch(Some("   \n"));

        let out = install(&path, Path::new(EXE)).unwrap();

        assert_eq!(out.added, EVENTS.to_vec());
        assert!(matches!(
            status(&path, Path::new(EXE)),
            Status::Installed { .. }
        ));
    }

    /// Uninstalling when there is no file is success, not an error.
    #[test]
    fn uninstalling_an_absent_file_is_not_an_error() {
        let (_home, path) = scratch(None);

        let out = uninstall(&path).unwrap();

        assert!(out.was_absent);
        assert!(out.removed.is_empty());
        assert!(!path.exists(), "uninstall must not CREATE a settings file");
    }

    /// An uninstall with nothing of ours to remove does not rewrite the
    /// file.
    ///
    /// Byte-compared rather than structurally: a "harmless" reformat shows
    /// up as a diff in a user's version-controlled dotfiles, which is a
    /// real cost for an operation that did nothing.
    #[test]
    fn uninstalling_when_nothing_is_ours_leaves_the_file_byte_identical() {
        let before = foreign_fixture();
        let (_home, path) = scratch(Some(&before));

        let out = uninstall(&path).unwrap();

        assert!(out.was_absent);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    // -----------------------------------------------------------------
    // Status (epic §5.5).
    // -----------------------------------------------------------------

    /// Only one of the two events installed is `Stale`, not `Installed`.
    ///
    /// A half-install is the case that would otherwise look fine: sessions
    /// get a start record and never an end one, so every session reads as
    /// possibly-still-running forever.
    #[test]
    fn a_half_install_is_stale_and_says_which_event_is_missing() {
        let body = serde_json::json!({
            "hooks": { "SessionStart": [
                { "_headstate": 1, "hooks": [{ "type": "command", "command": hook_command(Path::new(EXE)) }] },
            ]}
        });
        let (_home, path) = scratch(Some(&serde_json::to_string_pretty(&body).unwrap()));

        match status(&path, Path::new(EXE)) {
            Status::Stale { detail } => assert!(
                detail.contains("SessionEnd"),
                "the message must name the missing event: {detail}"
            ),
            other => panic!("got {other:?}"),
        }
    }

    /// A file with foreign hooks and none of ours is `NotInstalled` --
    /// which is the state that SHOULD invite the Install button.
    #[test]
    fn foreign_hooks_alone_are_not_installed() {
        let (_home, path) = scratch(Some(&foreign_fixture()));

        assert_eq!(status(&path, Path::new(EXE)), Status::NotInstalled);
    }

    /// Status never returns a `Result`, and this pins why: an absent file
    /// and an unreadable one are DIFFERENT answers.
    ///
    /// A directory where the file should be is the cheapest way to produce
    /// a read error that is not `NotFound`.
    #[test]
    fn an_unreadable_file_cannot_tell_rather_than_reporting_absent() {
        let home = tempfile::TempDir::new().unwrap();
        let path = settings_path_in(home.path());
        // A DIRECTORY named settings.json: reading it fails with something
        // other than NotFound.
        std::fs::create_dir_all(&path).unwrap();

        match status(&path, Path::new(EXE)) {
            Status::CannotTell(Refusal::Io { .. }) => {}
            other => panic!(
                "an unreadable file is not an absent one: the remedy is \
                 permissions, not the Install button -- got {other:?}"
            ),
        }
    }

    // -----------------------------------------------------------------
    // The command line itself.
    // -----------------------------------------------------------------

    /// The installed line is the app binary plus the subcommand, quoted.
    #[test]
    fn the_command_is_the_app_binary_and_the_subcommand() {
        assert_eq!(
            hook_command(Path::new(EXE)),
            "'/Applications/Headstate.app/Contents/MacOS/headstate' claude-hook"
        );
        assert!(
            hook_command(Path::new(EXE)).ends_with(SUBCOMMAND),
            "the installed line must end in the subcommand this module \
             declares, not a second copy of the string"
        );
    }

    /// The drift guard for [`SUBCOMMAND`]'s duplication.
    ///
    /// #912 (PR 925) adds `claude::cli::SUBCOMMAND` with the same value. If
    /// the two ever disagree, the installed hook invokes a subcommand the
    /// binary does not recognise, `cli::classify` returns `Invocation::App`,
    /// and the hook boots the entire GUI application instead of appending a
    /// line -- twice per session, inside a 1.5s budget. Silent, and
    /// expensive.
    ///
    /// Until #912 merges there is nothing to compare against, so this
    /// asserts the literal, which still catches a rename on this side. When
    /// that PR lands this becomes
    /// `assert_eq!(SUBCOMMAND, super::super::cli::SUBCOMMAND)` and the
    /// duplication is checked by the compiler-adjacent thing instead.
    #[test]
    fn the_subcommand_agrees_with_the_cli_module() {
        assert_eq!(
            SUBCOMMAND, "claude-hook",
            "#912's `claude::cli::SUBCOMMAND` is this same literal, and a \
             disagreement makes the installed hook boot the whole GUI app \
             instead of appending a line"
        );
    }

    /// A path with a space is quoted so the shell does not split it.
    ///
    /// Not a hypothetical: `Headstate 2.app` is what macOS produces when a
    /// second copy is dropped into `/Applications`, and the failure mode is
    /// the silent one -- the hook simply never runs.
    #[test]
    fn a_path_with_a_space_is_quoted() {
        let cmd = hook_command(Path::new(
            "/Applications/Headstate 2.app/Contents/MacOS/headstate",
        ));
        assert_eq!(
            cmd,
            "'/Applications/Headstate 2.app/Contents/MacOS/headstate' claude-hook"
        );
        // Round-trips through install and back out unchanged, so the
        // quoting survives JSON encoding too.
        let (_home, path) = scratch(None);
        let exe = Path::new("/Applications/Headstate 2.app/Contents/MacOS/headstate");
        install(&path, exe).unwrap();
        assert_eq!(commands_under(&path, "SessionStart"), vec![cmd]);
    }

    /// A single quote in the path is escaped the POSIX way rather than
    /// breaking out of the quoting.
    #[test]
    fn a_single_quote_in_the_path_is_escaped() {
        let cmd = hook_command(Path::new("/Users/o'brien/Headstate"));
        assert_eq!(cmd, r"'/Users/o'\''brien/Headstate' claude-hook");
    }

    /// The marker is written on every matcher we install.
    ///
    /// Without it the only predicate left is the path, and a reinstall
    /// after the app moves would strand the old matcher -- see
    /// `the_marker_alone_identifies_our_matcher`.
    #[test]
    fn every_installed_matcher_carries_the_marker() {
        let (_home, path) = scratch(None);
        install(&path, Path::new(EXE)).unwrap();

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for event in EVENTS {
            let m = &root["hooks"][event][0];
            assert_eq!(
                m[MARKER],
                Value::from(1),
                "missing the marker under {event}"
            );
            assert_eq!(m["hooks"][0]["type"], "command");
        }
    }

    /// The written file is pretty-printed with a trailing newline.
    ///
    /// Not cosmetic. The refusal path tells the user to fix this file by
    /// hand, so it has to be a file a human can read -- and a missing
    /// trailing newline is a diff in every tool that touches it afterwards.
    #[test]
    fn the_written_file_is_readable_and_newline_terminated() {
        let (_home, path) = scratch(None);
        install(&path, Path::new(EXE)).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.ends_with("}\n"), "got {body:?}");
        assert!(body.contains("\n  \"hooks\""), "got {body}");
    }

    /// No temp file is left behind after a successful install.
    #[test]
    fn the_temp_file_does_not_survive_a_successful_install() {
        let (_home, path) = scratch(None);
        install(&path, Path::new(EXE)).unwrap();

        assert!(!path.with_extension("json.headstate-tmp").exists());
    }

    /// The JSON every state serialises to, pinned because the TypeScript
    /// that reads it is a hand-written type in `src/api/tauri.ts`.
    ///
    /// There is no generated binding between the two, so the only thing
    /// keeping them in step is this test plus a matching comment there. The
    /// shape worth pinning is `cannot_tell`: `#[serde(tag = "state")]` on an
    /// enum with a NEWTYPE variant FLATTENS the inner value's fields into the
    /// same object, so the refusal's own `kind` arrives as a sibling of
    /// `state` rather than nested under a key. A TypeScript type written the
    /// other way -- `{ state: "cannot_tell"; refusal: ... }` -- would read
    /// `undefined` at runtime with no type error, which is exactly the silent
    /// failure this whole module is about.
    #[test]
    fn the_wire_shape_matches_the_typescript_type() {
        let j = |s: &Status| serde_json::to_string(s).unwrap();

        assert_eq!(
            j(&Status::Installed {
                command: "cmd".into()
            }),
            r#"{"state":"installed","command":"cmd"}"#
        );
        assert_eq!(j(&Status::NotInstalled), r#"{"state":"not_installed"}"#);
        assert_eq!(
            j(&Status::Stale { detail: "d".into() }),
            r#"{"state":"stale","detail":"d"}"#
        );
        // FLATTENED: `kind` is a sibling of `state`, not nested.
        assert_eq!(
            j(&Status::CannotTell(Refusal::Malformed {
                path: "p".into(),
                detail: "d".into()
            })),
            r#"{"state":"cannot_tell","kind":"malformed","path":"p","detail":"d"}"#
        );
        // And the other refusals, so a new variant added without a
        // TypeScript counterpart shows up here.
        assert_eq!(
            j(&Status::CannotTell(Refusal::Io {
                path: "p".into(),
                detail: "d".into()
            })),
            r#"{"state":"cannot_tell","kind":"io","path":"p","detail":"d"}"#
        );
        assert_eq!(
            j(&Status::CannotTell(Refusal::HooksNotAnObject {
                path: "p".into(),
                found: "a list".into()
            })),
            r#"{"state":"cannot_tell","kind":"hooks_not_an_object","path":"p","found":"a list"}"#
        );
        assert_eq!(
            j(&Status::CannotTell(Refusal::MatcherNotUnderstood {
                path: "p".into(),
                event: "SessionStart".into()
            })),
            r#"{"state":"cannot_tell","kind":"matcher_not_understood","path":"p","event":"SessionStart"}"#
        );
    }

    /// A healthy install reports every event as recording; every other
    /// status reports none.
    ///
    /// Both directions, because the failure mode is asymmetric. Claiming
    /// too MANY would let a half-installed machine present a floor as a
    /// total, which is the confidently-wrong number the house rule
    /// forbids; claiming too FEW only costs a "could not tell" on a
    /// machine that is fine.
    #[test]
    fn only_a_healthy_install_is_reported_as_recording() {
        assert_eq!(
            recording_events(&Status::Installed {
                command: "x".into()
            }),
            EVENTS.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );

        for status in [
            Status::NotInstalled,
            Status::Stale {
                detail: "no hook is installed for PermissionDenied".into(),
            },
            Status::CannotTell(Refusal::Io {
                path: "p".into(),
                detail: "boom".into(),
            }),
        ] {
            assert!(
                recording_events(&status).is_empty(),
                "{status:?} cannot say which events are intact, so it must \
                 not claim any are -- a floor presented as a total is the \
                 confidently-wrong number the house rule forbids"
            );
        }
    }

    /// The installed events are exactly the ones with an issue behind
    /// them.
    ///
    /// A guard rather than a tautology: adding a per-turn hook here would
    /// multiply the handoff file's volume by the number of tool calls in a
    /// session, and the transcript already carries that detail. A change to
    /// this list should be a deliberate one with an issue behind it.
    ///
    /// #1061 made the list additive without adding to it. #1065, #1066 and
    /// #1067 are the first additions, and each installs ONE event where the
    /// docs offer a pair -- `PreCompact` not `PostCompact`, `SubagentStart`
    /// not `SubagentStop` -- because the epic's rule rejects the second of
    /// a pair that answers the same question at twice the volume. Those
    /// arguments live on [`super::super::hook::compaction`] and
    /// [`super::super::hook::subagents`], next to the measurements that
    /// settle them.
    ///
    /// Asserted as a literal list rather than against a length or a
    /// `contains`, so that installing a further event is a change someone
    /// has to make here on purpose.
    #[test]
    fn only_the_events_with_an_issue_behind_them_are_installed() {
        assert_eq!(
            EVENTS,
            &[
                "SessionStart",
                "SessionEnd",
                "StopFailure",
                "PostToolUseFailure",
                "PermissionDenied",
                "PreCompact",
                "SubagentStart",
                "Notification",
            ]
        );

        // The bulk per-tool-call hook stays rejected. #1063 admits the
        // FAILURE slice precisely because failures are rare; admitting the
        // whole event would be the "volume without an answer" this module
        // has argued against since #910.
        assert!(
            !EVENTS.contains(&"PostToolUse"),
            "the bulk per-tool-call hook is one record per tool call, which \
             is the volume #1063's rarity argument exists to avoid"
        );
        // Headstate is an OBSERVER. A `PreToolUse` hook can deny a tool
        // call, which would turn a read-only dashboard into something that
        // can break the user's session -- epic #1060 rules it out
        // explicitly and this is that rule as a test.
        assert!(
            !EVENTS.contains(&"PreToolUse"),
            "a hook that can block a tool call is not an observer"
        );
    }

    // -----------------------------------------------------------------
    // #1061: the install surface is additive.
    //
    // Each of these installs an EXPANDED list through `install_events`
    // rather than through `EVENTS`, because the claim is about a list
    // longer than today's and no assertion over today's list can test it.
    // The fixture events are the ones epic #1060 names, so the shape is
    // the real one -- but nothing here installs them for real.
    // -----------------------------------------------------------------

    /// The event list a fully-landed epic #1060 would produce: everything
    /// installed today plus every event the epic still proposes.
    ///
    /// It must stay a SUPERSET of [`EVENTS`], which
    /// [`tests::the_expanded_fixture_is_a_superset_of_the_real_list`]
    /// pins. The differential test below installs both lists into copies
    /// of one document and asserts the shared events come out identical;
    /// if `EVENTS` ever held an event `EXPANDED` did not, that comparison
    /// would fail on a key that exists in one document and not the other,
    /// reporting a clobber that never happened.
    ///
    /// Order matters -- the events installed today stay FIRST, because an
    /// addition appends rather than reorders.
    const EXPANDED: &[&str] = &[
        "SessionStart",
        "SessionEnd",
        "StopFailure",
        "PostToolUseFailure",
        "PermissionDenied",
        "PreCompact",
        "SubagentStart",
        "SubagentStop",
        "Notification",
    ];

    /// The fixture list covers everything really installed.
    ///
    /// Guards the assumption the differential test rests on. Without it,
    /// a later sub-issue installing an event nobody added to `EXPANDED`
    /// would make that test compare a key present in one document against
    /// a missing key in the other -- which fails, but names a clobber
    /// rather than the stale fixture that actually caused it. Two hours
    /// were spent on exactly that shape while writing #1065/#1066/#1067.
    #[test]
    fn the_expanded_fixture_is_a_superset_of_the_real_list() {
        let missing: Vec<&&str> = EVENTS.iter().filter(|e| !EXPANDED.contains(e)).collect();
        assert!(
            missing.is_empty(),
            "EXPANDED is the differential test's fixture and must contain \
             every event EVENTS installs; missing {missing:?}"
        );
    }

    /// Adding an event does not disturb the two already installed.
    ///
    /// The comparison is between two real installs into two copies of the
    /// SAME starting document -- one with today's list, one with the
    /// expanded one -- and the assertion is that the existing two events'
    /// arrays come out byte-identical in both. That is stronger than
    /// asserting the new keys appeared: an installer that rewrote,
    /// reordered or re-quoted the existing matchers while adding a new one
    /// would pass "the new key is there" and fail here.
    ///
    /// PROVEN BY SABOTAGE, and the first attempt did NOT fail -- which is
    /// the more useful half of the result, so both are recorded.
    ///
    /// **The sabotage that did not fail.** Clearing the event's array
    /// before appending (`hooks.insert((*event).to_string(),
    /// Value::Array(Vec::new()));`) destroys the foreign hook and left this
    /// test GREEN. It is a DIFFERENTIAL test: both installs were sabotaged
    /// identically, so both lost the foreign matcher and the two documents
    /// still agreed. That defect is real, and it is caught by
    /// [`tests::a_foreign_hook_under_a_new_event_survives_install_and_uninstall`]
    /// and by the headline round trip -- which both failed on it. This
    /// test is not the one that carries that rule.
    ///
    /// **The sabotage it does catch**, and the reason it exists: a defect
    /// whose effect SCALES WITH THE LIST LENGTH, which no assertion over
    /// today's two events can see. Making the append push one matcher per
    /// event in the list (`.extend(repeat_n(our_matcher(&command),
    /// events.len() - 1).chain(once(...)))`) fails here, with `SessionStart`
    /// carrying **eight** of our matchers under the expanded list against
    /// two under today's:
    ///
    /// ```text
    /// adding an event must leave the existing ones exactly as they were
    ///   left: [cc-status, ours, ours, ours, ours, ours, ours, ours, ours]
    ///  right: [cc-status, ours, ours]
    /// ```
    ///
    /// That is precisely the class of bug "additive" is a claim about, and
    /// it is invisible to every other test in this module.
    #[test]
    fn adding_an_event_leaves_the_existing_two_untouched() {
        let exe = Path::new(EXE);
        let (_home_a, today) = scratch(Some(&foreign_fixture()));
        let (_home_b, expanded) = scratch(Some(&foreign_fixture()));

        install(&today, exe).unwrap();
        let grown = install_events(&expanded, exe, EXPANDED).unwrap();

        // The expanded install really did add events the plain one did
        // not, or the comparison below is between two identical runs and
        // proves nothing.
        //
        // Selected by DIFFERENCE rather than by `skip(EVENTS.len())`:
        // that index trick was only correct while `EXPANDED` began with
        // exactly today's list, and #1065/#1066/#1067 moved three of its
        // entries into `EVENTS`. A positional assumption that silently
        // stops selecting the right events would leave this test passing
        // while checking nothing.
        let extra: Vec<&&str> = EXPANDED.iter().filter(|e| !EVENTS.contains(e)).collect();
        assert!(
            !extra.is_empty(),
            "EXPANDED must contain events EVENTS does not, or this test is \
             comparing a list against itself"
        );
        for event in extra {
            assert!(
                commands_under(&expanded, event).contains(&hook_command(exe)),
                "the expanded install must really have installed {event}"
            );
        }
        assert_eq!(grown.added.len() + grown.replaced.len(), EXPANDED.len());

        // And the two events that existed before are bit-for-bit what the
        // unexpanded install produced.
        let a: Value = serde_json::from_str(&std::fs::read_to_string(&today).unwrap()).unwrap();
        let b: Value = serde_json::from_str(&std::fs::read_to_string(&expanded).unwrap()).unwrap();
        for event in EVENTS {
            assert_eq!(
                b["hooks"][event], a["hooks"][event],
                "adding an event must leave the existing ones exactly as \
                 they were"
            );
        }
    }

    /// Install then uninstall with the EXPANDED set restores the document
    /// byte for byte.
    ///
    /// The headline round-trip, re-run against a list longer than today's.
    /// It is the property that makes `EVENTS` safe to grow: uninstall
    /// sweeps every key under `hooks` rather than today's list, so the six
    /// keys the expanded install added are removed along with the two, and
    /// no `"StopFailure": []` is left behind.
    ///
    /// PROVEN BY SABOTAGE. Narrowing `uninstall`'s sweep from every key to
    /// `EVENTS` (`let events: Vec<String> = EVENTS.iter().map(|e|
    /// (*e).to_string()).collect();`) fails here with the six extra keys
    /// still carrying our matcher:
    ///
    /// ```text
    /// install then uninstall with an expanded event set must restore the
    ///   document BYTE for byte
    ///   left: ... "PermissionDenied": [ { "_headstate": 1, ... } ], ...
    ///  right: ... (no such key)
    /// ```
    #[test]
    fn uninstall_after_an_expanded_install_is_byte_identical() {
        let before = foreign_fixture();
        let (_home, path) = scratch(Some(&before));
        let exe = Path::new(EXE);

        let installed = install_events(&path, exe, EXPANDED).unwrap();
        assert_eq!(
            installed.added.len() + installed.replaced.len(),
            EXPANDED.len()
        );
        // The middle of the test: the install really happened. Without
        // this, an `install_events` that did nothing would pass the byte
        // comparison below.
        for event in EXPANDED {
            assert!(
                commands_under(&path, event).contains(&hook_command(exe)),
                "{event} must carry our hook after the install"
            );
        }

        uninstall(&path).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "install then uninstall with an expanded event set must restore \
             the document BYTE for byte -- including removing the KEY of an \
             event the fixture never had, not leaving an empty array behind"
        );
    }

    /// A PARTIAL install of an expanded set still uninstalls cleanly.
    ///
    /// `install.rs` already models a partial install as a real state
    /// (`Status::Stale` names the missing event), so uninstall has to
    /// handle one: a user can be interrupted mid-write, or hand-delete one
    /// matcher, or be running a version whose `EVENTS` is shorter than the
    /// one that wrote the file.
    ///
    /// The fixture here is the one a shorter-list version produces: three
    /// of the expanded events are installed, five are not. Uninstall must
    /// remove exactly those three keys, leave the foreign hooks alone, and
    /// restore the document byte for byte.
    #[test]
    fn uninstall_from_a_partial_install_of_an_expanded_set_is_byte_identical() {
        let before = foreign_fixture();
        let (_home, path) = scratch(Some(&before));
        let exe = Path::new(EXE);
        let partial: &[&str] = &["SessionStart", "StopFailure", "Notification"];

        install_events(&path, exe, partial).unwrap();

        // Exactly the partial set carries our hook, and the five events we
        // did not install do not -- a partial install is partial.
        for event in EXPANDED {
            let has_ours = commands_under(&path, event).contains(&hook_command(exe));
            assert_eq!(
                has_ours,
                partial.contains(event),
                "{event} should{} carry our hook after a partial install",
                if partial.contains(event) { "" } else { " not" }
            );
        }

        let removed = uninstall(&path).unwrap();
        assert!(!removed.was_absent);
        let mut got = removed.removed.clone();
        got.sort();
        let mut want: Vec<String> = partial.iter().map(|e| (*e).to_string()).collect();
        want.sort();
        assert_eq!(
            got, want,
            "uninstall must report exactly the events it removed ours from, \
             not every event in the file"
        );

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "an uninstall from a PARTIAL install must still restore the \
             document byte for byte"
        );
    }

    /// A foreign hook under an event of the expanded set survives both
    /// halves of the round trip.
    ///
    /// This is the failure the module exists to prevent, restated for the
    /// events #1060 adds: `StopFailure`, `Notification` and `SubagentStop`
    /// all carry a foreign `cc-status` matcher on the development machine
    /// TODAY, before Headstate installs anything under them. So the first
    /// sub-issue that adds one of those events is installing beside
    /// somebody else's tool from its very first run -- and an uninstall
    /// that took the key with it would silently disable that tool.
    ///
    /// PROVEN BY SABOTAGE, twice, because the two halves fail differently:
    ///
    /// - Replacing the append in `install_events` with an assignment
    ///   (`hooks.insert((*event).to_string(),
    ///   Value::Array(vec![our_matcher(&command)]))`) fails at the install
    ///   assertion, with `cc-status` gone from `StopFailure`.
    /// - Weakening `is_ours` to `true` fails at the uninstall assertion,
    ///   with the foreign matcher removed and the key gone entirely.
    #[test]
    fn a_foreign_hook_under_a_new_event_survives_install_and_uninstall() {
        let before = foreign_fixture();
        let (_home, path) = scratch(Some(&before));
        let exe = Path::new(EXE);
        // These three are in `foreign_fixture` already -- the same shape
        // the real development machine has.
        let shared = ["StopFailure", "Notification", "SubagentStop"];

        install_events(&path, exe, EXPANDED).unwrap();

        for event in shared {
            assert_eq!(
                commands_under(&path, event),
                vec![CC_STATUS.to_string(), hook_command(exe)],
                "installing into {event} must APPEND beside the foreign hook \
                 already there, not replace the array"
            );
        }

        uninstall(&path).unwrap();

        for event in shared {
            assert_eq!(
                commands_under(&path, event),
                vec![CC_STATUS.to_string()],
                "uninstalling from {event} must leave the foreign hook -- \
                 removing someone else's tool is the worst failure this \
                 module can have"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}
