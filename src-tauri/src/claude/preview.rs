//! The tail of one transcript, as conversation rather than as JSONL
//! (#982, epic #941).
//!
//! Before pasting `cd … && claude --resume 3a92d7c9-…`, the user wants to
//! check that this is the right session. Titles are not enough: 286 of
//! 1,438 sessions share a title with another and 147 do inside the
//! largest directory, mostly repeated `/security-review` runs. And on a
//! phone there is no path to the content at all -- `claude_reveal_path`
//! is `Class::Local`, so the companion user who can see that a session
//! died cannot see one word of what it was doing.
//!
//! # Why this is not the whole file
//!
//! #910 deferred a preview with a reason that is still half true:
//! *"the median transcript is 178 KB and the largest on this machine is
//! 76 MB, and rendering that in the webview is how you hang the app"*.
//! Re-measured here over the 1,502 real session transcripts:
//!
//! ```text
//! median   183,237 bytes (179 KB)
//! p90          464 KB
//! p99         11.4 MB
//! max     76,740,099 bytes   (one file, 8.4% of the 916 MB corpus)
//! over  1 MB    39 files (2.6%)
//! over 10 MB    16 files (1.1%)
//! ```
//!
//! The hazard is real and the remedy is the byte cap #910's own design
//! already specified. [`TAIL_BYTES`] is 256 KB, which returns the whole
//! conversation for the median session and a genuine tail for the 39
//! large ones -- so the cap is never the common case, and when it does
//! bind, [`Preview::truncated`] says so. A pane that silently showed a
//! tail would be the #846 defect in its purest form: the reader cannot
//! tell a short conversation from a truncated one.
//!
//! 256 KB rather than more because this crosses the pairing transport to
//! the phone on every selection, and 256 KB of JSONL reduces to far less
//! once the machinery records are dropped -- see below. It is also what
//! `transcript.rs`'s own `TAIL_BYTES_RETRY` already reads 1,502 times in
//! 0.35 s, so the machinery and its cost are both established.
//!
//! # An allowlist of record types, not a denylist
//!
//! Sixteen record types exist. Measured over 19,725 records sampled from
//! 60 files:
//!
//! ```text
//!   6950  assistant           862  last-prompt         138  file-history-delta
//!   4050  user                831  ai-title             86  file-history-snapshot
//!   3099  attachment          662  mode                 85  relocated
//!                             662  permission-mode      85  worktree-state
//!                             634  pr-link               4  cost-state
//!                             628  queue-operation
//!                             493  atis-latch
//!                             456  system
//! ```
//!
//! `assistant` and `user` are 55.8% of records and carry the conversation.
//! The other fourteen are bookkeeping, and a reader that rendered them
//! would show the user a wall of internal state.
//!
//! It is an ALLOWLIST because Claude Code owns this format and changes it
//! -- `liveness.rs`'s module docs already anticipate *"a release changing
//! it"*. An allowlist degrades to showing less; a denylist degrades to
//! showing a user the internals of a record type nobody has seen yet.
//!
//! # Content is a block array, not a string
//!
//! Measured over the same sample: of 1,586 `assistant`/`user` messages,
//! 1,500 carry `content` as a LIST and 86 as a bare string. The blocks:
//!
//! ```text
//! tool_use 510   tool_result 509   text 286   thinking 195
//! ```
//!
//! So a renderer that assumed a string would show nothing for 95% of
//! messages. [`Block`] carries each kind with its own shape, and the
//! non-text kinds are reported as what they are rather than flattened
//! into prose -- a `tool_use` summarised as "Read" tells the reader what
//! the session was doing, and its full arguments do not.
//!
//! # Read-only
//!
//! `mod.rs` lists exactly two write exceptions under `~/.claude` and says
//! *"Nothing else in here writes to `~/.claude` at all."* A reader adds
//! no exception.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::signals::Observed;

/// How much of a transcript's tail is read.
///
/// See the module docs for the size distribution this is chosen against:
/// 256 KB returns the whole conversation for the 97.4% of transcripts
/// under 1 MB and a genuine tail for the 39 above it.
pub const TAIL_BYTES: u64 = 256 * 1024;

/// The most messages returned, however many the window held.
///
/// A second bound, because the byte cap alone does not bound the RENDER:
/// 256 KB of short `user` records is thousands of messages, and a phone
/// drawing thousands of them is the hang the byte cap was supposed to
/// prevent, arrived at by a different route. 200 is roughly `RENDER_CAP`
/// on the session list and for the same reason -- a budget with the total
/// stated, not a filter.
pub const MAX_MESSAGES: usize = 200;

/// The most characters kept from one text block.
///
/// A single `tool_result` can be an entire file. Truncating per block
/// rather than per response keeps the SHAPE of the exchange -- the reader
/// still sees that four tools ran -- where a whole-message cap would drop
/// the last three entirely.
const MAX_TEXT_CHARS: usize = 4_000;

/// The record types that are conversation. See the module docs on why
/// this is an allowlist.
const CONVERSATION_TYPES: [&str; 2] = ["assistant", "user"];

/// The record types that are lifecycle: read for their own fields, never
/// rendered as conversation.
///
/// Widening the allowlist, NOT loosening it. Each name here was measured
/// on the corpus and has a struct below that says what is taken from it.
/// A type absent from BOTH lists still increments
/// [`Preview::non_conversation_records`] and is dropped, so the module
/// docs' guarantee -- an unknown record degrades to showing LESS -- is
/// unchanged. The two lists are disjoint by construction; see
/// `the_two_allowlists_do_not_overlap`.
const LIFECYCLE_TYPES: [&str; 3] = ["queue-operation", "permission-mode", "worktree-state"];

/// What the lifecycle records in the window said (#1206).
///
/// Every field can be absent for a reason that is not zero -- the record
/// is outside the window, or the session predates the feature that
/// writes it -- so every field carries that state explicitly. "Not
/// observed" never means "none happened".
///
/// Two of them use [`Observed`], which is `Option`. [`Self::worktree`]
/// cannot: it has THREE states rather than two, and nested `Option`
/// serialises two of them to the same JSON `null`. See [`Worktree`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    /// The prompt queue, or `None` when it could not be counted.
    ///
    /// `None` whenever the read was truncated, however many
    /// `queue-operation` records the window held. See [`Queue`].
    pub queue: Observed<Queue>,
    /// The newest `permission-mode` value in the window, verbatim.
    ///
    /// `None` is the COMMON case and must render as absent, never as a
    /// default. The record is written on CHANGE, not continuously -- 48
    /// records against 411 assistant messages in #1206's sample -- so a
    /// session that set its mode at minute two and ran six hours has it
    /// outside this window and outside `transcript.rs`'s head window
    /// too. Substituting "ask every time" or "auto" here would be #846
    /// in its exact original form: a benign-looking default standing in
    /// for a measurement nobody made.
    pub permission_mode: Observed<String>,
    /// The newest `worktree-state` in the window.
    ///
    /// Three-valued on purpose, and the middle value is the common one:
    /// no record in the window; a record whose `worktreeSession` is
    /// explicitly `null`, which is the session saying it is NOT in a
    /// worktree; or a worktree. Measured over 1,907 real records: 1,564
    /// (82%) carry `null`. A reader that collapsed the first two would
    /// report "unknown" for the 82% that are a definite answer.
    ///
    /// A NAMED enum and not `Observed<Option<WorktreeState>>`, which was
    /// the first shape tried: nested `Option` serialises BOTH absent and
    /// explicit-null to a bare JSON `null`, so the distinction this
    /// field exists to carry was lost at the wire and the UI could not
    /// have recovered it. See
    /// `the_serialised_shape_keeps_absent_and_explicit_null_apart`.
    pub worktree: Worktree,
}

/// The prompt queue as a BALANCE, not a list.
///
/// # Why a truncated read reports `None`
///
/// [`TAIL_BYTES`] cuts the middle of things. An `enqueue` outside the
/// window whose `dequeue` is inside yields a NEGATIVE queue; the reverse
/// yields a PHANTOM pending item that was in fact delivered long ago.
/// Both are wrong answers that LOOK right -- the reader cannot tell a
/// real pending prompt from an artefact of where the byte cap landed.
///
/// So [`Lifecycle::queue`] is `None` whenever [`Preview::truncated`] is
/// set. That is not a failure to parse; it is the honest report that the
/// balance was not measurable from this window. `Some` means the read
/// reached the start of the file, so every operation is accounted for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    /// Prompts queued behind the running turn.
    pub enqueued: usize,
    /// Prompts taken off the queue to run.
    pub dequeued: usize,
    /// Prompts taken off the queue WITHOUT running, by reason, verbatim.
    ///
    /// `reason: "absorbed_mid_turn"` is a prompt the user typed that
    /// never ran as its own turn -- information worth keeping, not
    /// noise. Measured: 750 `absorbed_mid_turn`, 115
    /// `delivered_to_agent`, and 1,233 `remove` records in total, so
    /// other reasons exist and are kept as themselves.
    pub removed: Vec<(String, usize)>,
    /// `remove` records that carried no `reason` at all.
    ///
    /// Counted apart from [`Self::removed`], for the reason
    /// [`Compactions::untriggered`] is: "we saw a removal and it named no
    /// reason" is not the same fact as any named reason.
    pub removed_unexplained: usize,
    /// Operations that are none of the three documented kinds, verbatim
    /// and counted.
    ///
    /// Never bucketed into "other". A vocabulary that grew is
    /// information; a bucket named "other" destroys it -- the rule
    /// [`super::signals::PROMPTS`] already states.
    pub unknown_operations: Vec<(String, usize)>,
}

impl Queue {
    /// What is still stacked behind the running turn, when that is
    /// answerable.
    ///
    /// `None` when the arithmetic comes out NEGATIVE. Within an
    /// untruncated window it should not, but a transcript is a file
    /// Claude Code owns and a reader that returned a `usize` would have
    /// to saturate at zero -- reporting "nothing queued" for a file it
    /// did not understand. An impossible balance is an unknown one.
    pub fn pending(&self) -> Observed<usize> {
        let taken = self.dequeued
            + self.removed.iter().map(|(_, n)| n).sum::<usize>()
            + self.removed_unexplained;
        self.enqueued.checked_sub(taken)
    }
}

/// What the window could say about a session's worktree (#1206).
///
/// Tagged on the wire so the three states stay three states. `Unknown`
/// is absent-is-not-zero; `NotInWorktree` is a measured negative; and
/// they must never render the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Worktree {
    /// No `worktree-state` record in the window. NOT a statement that
    /// the session is outside a worktree -- the record may simply sit
    /// before the byte cap, exactly as `permission_mode` does.
    #[default]
    Unknown,
    /// A record whose `worktreeSession` was explicitly `null`: the
    /// session stating it is not in a worktree. 82% of real records.
    NotInWorktree,
    /// A record naming a worktree.
    In(WorktreeState),
}

/// The worktree a session moved into, as its record states it.
///
/// `originalBranch` and `originalHeadCommit` are the facts the
/// path-based session-to-worktree join cannot recover once the directory
/// is gone (#1137).
///
/// Every field is `Option` and none is substituted: all eight keys were
/// present on all 343 non-null records measured, but a record is a
/// format Claude Code owns, and a fabricated path cannot be told from a
/// real one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeState {
    /// Where the session was before it entered the worktree.
    pub original_cwd: Option<String>,
    /// The worktree's directory.
    pub worktree_path: Option<String>,
    /// The worktree's short name.
    pub worktree_name: Option<String>,
    /// The branch checked out IN the worktree.
    pub worktree_branch: Option<String>,
    /// The branch the session was on before it entered.
    pub original_branch: Option<String>,
    /// The commit that branch pointed at -- unrecoverable afterwards.
    pub original_head_commit: Option<String>,
}

/// The tail of one transcript, as messages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Preview {
    /// Oldest first, so it reads as a conversation.
    pub messages: Vec<Message>,
    /// Whether anything before these messages was NOT read.
    ///
    /// `true` when the byte window did not reach the start of the file,
    /// or when [`MAX_MESSAGES`] dropped older messages from a window that
    /// did. Either way the pane must say it is showing a tail: a reader
    /// who cannot tell a short conversation from a truncated one has been
    /// told something false by omission (#846).
    pub truncated: bool,
    /// Bytes read from the tail, so the label can state the window rather
    /// than assert the constant.
    pub bytes_read: u64,
    /// The file's whole size, so the label can state the fraction.
    pub file_bytes: u64,
    /// Records inside the window that were not conversation.
    ///
    /// Counted rather than dropped silently, for the reason
    /// `Scan::subagent_files_skipped` is counted: an exclusion that is
    /// invisible invites the reader to conclude the reader is broken. On
    /// the real corpus this is the larger number -- 44.2% of records are
    /// machinery -- so a pane showing six messages out of a 300-record
    /// window needs to say where the rest went.
    pub non_conversation_records: usize,
    /// What the lifecycle records in the window said (#1206).
    ///
    /// Read rather than counted-and-dropped, but still not rendered as
    /// conversation: these carry session state, not messages. They do
    /// NOT increment `non_conversation_records` -- that count means "we
    /// opened this and threw it away", and these are no longer thrown
    /// away.
    pub lifecycle: Lifecycle,
    /// Lines inside the window that would not parse as JSON at all.
    ///
    /// Distinct from `non_conversation_records`: one is a record we
    /// understood and chose not to show, the other is a record we could
    /// not read. Absent is not zero, and "skipped" is not "failed".
    pub unparseable_records: usize,
    /// Every `tool_use_id` in the window, and how it paired (#1209).
    ///
    /// A side table rather than a field on [`Block`], because pairing is
    /// a fact about the WINDOW and not about the block: the identical
    /// `tool_use` block is paired in a window that reached its result and
    /// unanswered in one that stopped a line short. Putting it on the
    /// block would make two reads of the same bytes produce unequal
    /// blocks, which is the property the existing tests rest on.
    ///
    /// Keyed by id, so the UI looks up the block it is drawing rather
    /// than tracking position.
    pub pairings: std::collections::BTreeMap<String, Pairing>,
    /// Calls in the window with no result in it.
    ///
    /// A count, so the pane can say "3 calls have no recorded result"
    /// without walking every block. The MEANING of a non-zero count is
    /// not decided here: a running session has calls in flight, and a
    /// dead one has calls that never came back. Those are different
    /// facts and `liveness.rs` owns the difference.
    pub unanswered_calls: usize,
    /// Results in the window whose call is above it.
    ///
    /// Non-zero is the normal consequence of a tail read, not a defect,
    /// and the pane says so rather than leaving the reader to wonder why
    /// output appeared with nothing that asked for it.
    pub results_above_window: usize,
}

/// One message in the conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// `"assistant"` or `"user"`, from the record `type`.
    pub role: String,
    /// RFC 3339, or `None` for a record that carried none. Not
    /// substituted: a fabricated time cannot be told from a real one.
    pub timestamp: Option<String>,
    /// The model that wrote it, for `assistant` messages that name one.
    pub model: Option<String>,
    /// The content blocks, in order.
    pub blocks: Vec<Block>,
}

/// One content block, as the kind it actually is.
///
/// A tagged enum rather than a flattened string, because the four kinds
/// answer different questions and the UI renders them differently: text
/// is what was said, a tool call is what was done, and a tool result is
/// usually far too long to show whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    /// Prose. `truncated` when [`MAX_TEXT_CHARS`] bound it.
    Text { text: String, truncated: bool },
    /// The model's reasoning, kept separate so the UI can fold it away:
    /// 195 of 1,500 blocks sampled are thinking, and a reader scanning
    /// for "what was it doing" does not want it inline by default.
    Thinking { text: String, truncated: bool },
    /// A tool call: its name, its pairing key, and its arguments PARSED
    /// into the shape that tool actually takes.
    ///
    /// # Why not the raw `input`, and why not nothing either
    ///
    /// The original reading was right about the blob and wrong about the
    /// structure. "Read" or "Bash" does tell the reader what the session
    /// was doing, and a 40 KB argument blob dumped verbatim does not --
    /// so this does not dump it. It PARSES it: [`ToolArgs`] carries one
    /// variant per tool shape the corpus actually holds, and the UI
    /// renders a command as a command and an edit as a diff. Saying a
    /// session edited 40 files while being unable to say what it changed
    /// in any of them is the gap this closes (#1209).
    ///
    /// `id` is the pairing key. It is on the `tool_use` block in
    /// **28,370 of 28,370** sampled calls, and the matching
    /// `tool_result.tool_use_id` in 28,393 of 28,393 results -- so
    /// pairing is not best-effort, it is the format's own contract.
    ToolUse {
        name: String,
        /// The `id` this call's result will name. `None` only for a call
        /// that carried none, which the corpus does not contain but the
        /// format does not forbid -- and such a call can never be paired,
        /// so it must not be silently treated as orphaned.
        id: Option<String>,
        args: ToolArgs,
    },
    /// A tool's output, bounded like any other text, carrying the key
    /// that names the call it answers.
    ToolResult {
        text: String,
        truncated: bool,
        /// The `tool_use_id` this result answers.
        tool_use_id: Option<String>,
        /// Whether the tool reported failure. `None` when the record
        /// carried no `is_error` at all (4,065 of 28,393 sampled), which
        /// is not the same as `Some(false)`.
        is_error: Option<bool>,
        /// The diff this result recorded, when it recorded one.
        ///
        /// Read from the record's `toolUseResult` sibling, NOT from the
        /// block -- see [`FileChange`] on where this actually lives.
        change: Option<FileChange>,
    },
    /// A block kind this build does not know.
    ///
    /// Reported rather than dropped: Claude Code owns this format, and a
    /// pane that silently omitted a future block type would show a reader
    /// an exchange with a hole in it and no sign that anything was
    /// missing.
    Other { block_type: String },
}

/// A tool call's arguments, as the shape that tool actually takes.
///
/// One variant per tool the corpus carries in quantity, measured over 40
/// real transcripts (28,370 calls):
///
/// ```text
/// Bash 24,258   Edit 543   Write 378   Read 255   Grep 69   Glob 1
/// ```
///
/// [`ToolArgs::Other`] keeps [`Block::Other`]'s guarantee one level down:
/// a tool this build does not know is REPORTED by name with its argument
/// keys, never silently dropped and never dumped whole. Claude Code owns
/// this format and adds tools -- 18 distinct names appear in the sample,
/// most of them MCP tools that did not exist when the pane was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolArgs {
    /// `Edit`: a string-level replacement in one file.
    Edit {
        file_path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
        /// Whether either string was clipped by [`MAX_TEXT_CHARS`].
        truncated: bool,
    },
    /// `MultiEdit`: several replacements in one file, applied in order.
    ///
    /// Absent from this machine's corpus (0 of 28,370 calls) but named in
    /// the ticket and shipped by Claude Code, so it is parsed rather than
    /// left to fall through to [`ToolArgs::Other`]. `edits` is bounded by
    /// [`MAX_EDITS`] and says when the bound bit.
    MultiEdit {
        file_path: String,
        edits: Vec<Replacement>,
        /// Edits beyond [`MAX_EDITS`] that are not listed. `0` when all
        /// of them are -- a count, not a flag, so the pane can say how
        /// many are missing rather than only that some are.
        edits_omitted: usize,
    },
    /// `Write`: the whole new contents of a file.
    Write {
        file_path: String,
        content: String,
        truncated: bool,
    },
    /// `Bash`: the command, and the description the caller gave it.
    Bash {
        command: String,
        /// The one-line description. Present on 24,258 of 24,258 sampled
        /// `Bash` calls, and it is the sentence a reader actually scans.
        description: Option<String>,
        truncated: bool,
    },
    /// `Read`: a file, optionally a window within it.
    Read {
        file_path: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    /// `Grep`: the pattern and where it was run.
    Grep {
        pattern: String,
        path: Option<String>,
        output_mode: Option<String>,
    },
    /// `Glob`: the pattern and where it was run.
    Glob {
        pattern: String,
        path: Option<String>,
    },
    /// `Task` / `Agent`: a delegated sub-session.
    ///
    /// Both names, because this machine's corpus spells it `Agent` (236
    /// calls) and the ticket and older transcripts spell it `Task`. One
    /// variant rather than two: it is the same act, and a pane that
    /// rendered them differently would be reporting a rename as a
    /// distinction.
    Task {
        description: Option<String>,
        subagent_type: Option<String>,
        prompt: String,
        truncated: bool,
    },
    /// A tool whose argument shape this build does not know.
    ///
    /// The KEYS, not the values: the keys are what tell a reader whether
    /// Headstate is simply behind, and the values are the 40 KB blob the
    /// original reasoning was right to refuse. Keys are sorted so two
    /// renderings of the same call cannot differ by `serde_json`'s map
    /// order.
    Other { keys: Vec<String> },
    /// The call carried no `input` object at all.
    ///
    /// Distinct from [`ToolArgs::Other`] with no keys: one is "arguments
    /// we did not recognise", the other is "no arguments were recorded".
    /// Absent is not zero.
    None,
}

/// One replacement inside a [`ToolArgs::MultiEdit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replacement {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
    pub truncated: bool,
}

/// The most edits listed from one `MultiEdit`.
///
/// A bound for the same reason [`MAX_MESSAGES`] is one: the render is
/// what has to stay finite, and a 200-edit call drawn whole is the hang
/// the byte cap exists to prevent reached by another route. Stated, never
/// silent -- `edits_omitted` carries the remainder.
const MAX_EDITS: usize = 20;

/// The most hunks kept from one recorded patch.
const MAX_HUNKS: usize = 20;

/// The most lines kept from one hunk.
///
/// Per hunk rather than per patch, for [`MAX_TEXT_CHARS`]'s own reason:
/// clipping per unit keeps the SHAPE -- the reader still sees that five
/// regions changed -- where a whole-patch cap would drop the last four
/// entirely.
const MAX_HUNK_LINES: usize = 60;

/// What a file change was reconstructed FROM.
///
/// The whole point of the type, and the reason it is an enum rather than
/// a boolean on [`FileChange`]. A diff built from content the transcript
/// RECORDED and one reconstructed from the replacement strings alone are
/// different epistemic objects: the first shows the surrounding lines as
/// they actually were, the second cannot show them at all. Rendering both
/// as "a diff" tells the reader the second has context it does not have.
///
/// # The option that is not here
///
/// Reading the file from disk NOW would produce a diff with context for
/// every edit. It is forbidden, and not as a matter of taste: the file
/// has changed since -- that is what a session DOES -- so its current
/// content is not its content at the time of the edit. Presenting it as
/// the historical original is fabrication in the exact shape the reader
/// is least able to detect, because it looks like a well-formed diff. The
/// #846 rule ("a reading you could not take is not a reading of zero")
/// with the failure mode inverted: here the fabricated reading is not
/// zero but a plausible wrong number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSource {
    /// Built from a `structuredPatch` the transcript recorded, so the
    /// context lines around each change are the file as it WAS.
    ///
    /// Measured: present on 915 of 915 sampled records that carry any
    /// edit information at all -- which is why it is the primary source
    /// and not the rare one.
    Recorded,
    /// Reconstructed from `old_string`/`new_string` alone.
    ///
    /// Honest and narrow: the replaced text and its replacement, with NO
    /// surrounding context, because none was recorded. The fallback when
    /// no patch was recorded.
    Reconstructed,
}

/// A change to one file, as the transcript recorded it.
///
/// # Where this actually lives, which is not where the ticket said
///
/// The ticket expected `originalFile` on the `tool_result` BLOCK. It is
/// not there. It is on a `toolUseResult` field of the enclosing RECORD,
/// a sibling of `message` -- so a parser that only ever descends into
/// `message.content` cannot see it at all. Measured over 40 real
/// transcripts:
///
/// ```text
/// records carrying edit information   915
///   with `structuredPatch`            915  (100%)
///   with a non-null `originalFile`    181  ( 20%)
///   with `oldString`/`newString`      539  ( 59%)
/// ```
///
/// So `originalFile` is the MINORITY case, not the primary one, and
/// `structuredPatch` -- a ready-made unified diff with real context lines
/// -- is universal. This takes `structuredPatch` as the recorded source,
/// which is the same epistemic object the ticket wanted `originalFile`
/// for (content the transcript wrote down, not content read back now) and
/// available five times as often.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub file_path: Option<String>,
    /// What this was built from. Renders differently per variant; see
    /// [`DiffSource`].
    pub source: DiffSource,
    pub hunks: Vec<Hunk>,
    /// Hunks beyond [`MAX_HUNKS`] that are not shown.
    pub hunks_omitted: usize,
    /// Whether the file did not exist before -- a creation, not an edit.
    ///
    /// `Some(true)` from a recorded `type: "create"`. `None` when the
    /// record said nothing, which is not "it existed".
    pub created: Option<bool>,
}

/// One region of a change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hunk {
    /// The first line of this region in the file as it was, when the
    /// record said. `None` for a reconstructed hunk, which has no line
    /// numbers because nothing recorded any.
    pub old_start: Option<i64>,
    pub new_start: Option<i64>,
    pub lines: Vec<DiffLine>,
    /// Lines dropped from THIS hunk by [`MAX_HUNK_LINES`].
    ///
    /// Per hunk, not per pane, and a count rather than a flag. A clipped
    /// diff that does not say it was clipped is a lie about what changed:
    /// the reader sees three changed lines and concludes three lines
    /// changed. Stating it once for the whole pane is not enough either,
    /// because it does not say WHICH hunk is short.
    pub lines_omitted: usize,
}

/// One line of a hunk, as the role it plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DiffLine {
    /// Present in both. Only ever from a RECORDED patch -- a
    /// reconstructed hunk has no context by construction.
    Context {
        text: String,
    },
    Added {
        text: String,
    },
    Removed {
        text: String,
    },
}

/// How a tool call and its result did or did not meet.
///
/// Four states, and the three orphan ones do NOT mean the same thing.
/// The pairing key crosses a message boundary and [`TAIL_BYTES`] cuts
/// wherever 256 KB lands, so orphans are the common case at a window
/// edge, not a corruption.
///
/// Resolved in the UI rather than here for the middle two: this module
/// reads a file and has no business asking whether a process is alive.
/// See [`Preview::unanswered_calls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pairing {
    /// The call and its result are both inside the window.
    Paired,
    /// A result whose call is NOT in the window.
    ///
    /// Means the call is older than the 256 KB we read -- "the call this
    /// answers is above the window". It is not a missing call; it is a
    /// call we did not look at. Nothing is wrong with the session.
    CallAboveWindow,
    /// A call with no result in the window.
    ///
    /// Deliberately does NOT say why. Whether this means "still running"
    /// or "it never came back" depends on whether the session is alive,
    /// which is `liveness.rs`'s question and has exactly one answer in
    /// this codebase. Deriving a second one here would produce two
    /// answers that disagree the first time either changes.
    Unanswered,
    /// The block carried no pairing key at all, so it can never be
    /// matched.
    ///
    /// Distinct from [`Pairing::Unanswered`]: that is "we looked and
    /// found no result", this is "we could not look". A call with no `id`
    /// is not evidence of anything about the session.
    Unkeyed,
}

/// Read the tail of `path` as conversation.
///
/// # Errors
///
/// Only when the file cannot be OPENED, sized or read. A file that opens
/// but holds nothing renderable yields a [`Preview`] with no messages and
/// its counts filled, which is honest ("we read it and there was no
/// conversation in the window") and distinct from the unreadable case --
/// the same split `transcript.rs` draws.
pub fn tail(path: &Path) -> Result<Preview, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let file_bytes = file
        .metadata()
        .map_err(|e| format!("{}: could not read its size: {e}", path.display()))?
        .len();

    let start = file_bytes.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("{}: could not seek in it: {e}", path.display()))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("{}: could not read it: {e}", path.display()))?;

    let mut out = Preview {
        bytes_read: buf.len() as u64,
        file_bytes,
        truncated: start > 0,
        ..Default::default()
    };

    let text = String::from_utf8_lossy(&buf);
    let mut body: &str = &text;
    if start > 0 {
        // The seek almost certainly landed mid-record. Drop everything up
        // to the first newline: a half record is unparseable anyway, and
        // keeping it would mean reasoning about partial JSON. This is
        // `transcript.rs`'s `newest_timestamp` rule, and its test
        // `a_huge_record_does_not_hide_the_last_activity` is the evidence
        // that a single record CAN exceed a window in this corpus.
        body = match body.find('\n') {
            Some(nl) => &body[nl + 1..],
            None => "",
        };
    }

    // Accumulated apart from `out.lifecycle` because whether it may be
    // PUBLISHED is not known until the whole window has been read -- see
    // the `truncated` gate below.
    let mut queue = QueueTally::default();

    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            out.unparseable_records += 1;
            continue;
        };
        let kind = rec.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if LIFECYCLE_TYPES.contains(&kind) {
            lifecycle_record(kind, &rec, &mut queue, &mut out.lifecycle);
            continue;
        }
        if !CONVERSATION_TYPES.contains(&kind) {
            // Still counted, still dropped. The allowlist was widened by
            // three NAMED types, not turned into a passthrough.
            out.non_conversation_records += 1;
            continue;
        }
        let Some(message) = rec.get("message") else {
            // A conversation record with no message body. Counted as
            // machinery rather than rendered as an empty bubble.
            out.non_conversation_records += 1;
            continue;
        };
        // The change lives on the RECORD (`toolUseResult`), not in
        // `message.content`, so it is read here and attached to the
        // result block below. A parser that only descends into the
        // message cannot see it at all -- which is why the ticket
        // expected `originalFile` in a place it has never been.
        let change = file_change(&rec);
        let mut blocks = blocks_of(message.get("content"));
        if let Some(change) = change {
            // Onto the FIRST result block only. A record carries one
            // `toolUseResult`, so attaching it to every result block in
            // a batched message would claim the same diff came back from
            // several different calls.
            if let Some(Block::ToolResult { change: slot, .. }) = blocks
                .iter_mut()
                .find(|b| matches!(b, Block::ToolResult { .. }))
            {
                *slot = Some(change);
            }
        }
        out.messages.push(Message {
            role: kind.to_owned(),
            // The record's own timestamp, not the message's: the record
            // is what `transcript.rs` dates sessions from, so the two
            // views of one file cannot disagree about when something
            // happened.
            timestamp: rec
                .get("timestamp")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
            model: message
                .get("model")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .map(str::to_owned),
            blocks,
        });
    }

    if out.messages.len() > MAX_MESSAGES {
        // The OLDEST go, not the newest: the last exchange before a
        // session died is the thing a user wants before resuming, and it
        // is at the end by construction.
        out.messages.drain(..out.messages.len() - MAX_MESSAGES);
        out.truncated = true;
    }

    // AFTER the drain, because the drain can set `truncated` too. A
    // balance is only publishable when every operation in the file was
    // seen: an `enqueue` we did not read whose `dequeue` we did gives a
    // negative queue, and the reverse gives a phantom pending prompt.
    // Both look like answers. `None` says we did not measure it.
    if !out.truncated {
        out.lifecycle.queue = Some(queue.into_queue());
    }
    // Pairing runs AFTER the message cap, so it describes the window the
    // reader is actually shown. Computing it before would pair a call
    // against a result that then got dropped, and the pane would render
    // "paired" beside a block with nothing to pair to.
    pair(&mut out);

    Ok(out)
}

/// The queue operations seen so far, before it is known whether the
/// window justifies publishing them.
#[derive(Debug, Default)]
struct QueueTally {
    enqueued: usize,
    dequeued: usize,
    removed: HashMap<String, usize>,
    removed_unexplained: usize,
    unknown: HashMap<String, usize>,
}

impl QueueTally {
    fn into_queue(self) -> Queue {
        Queue {
            enqueued: self.enqueued,
            dequeued: self.dequeued,
            removed: sorted_counts(self.removed),
            removed_unexplained: self.removed_unexplained,
            unknown_operations: sorted_counts(self.unknown),
        }
    }
}

/// Counted pairs in a stable order: commonest first, then by name.
///
/// Sorted rather than left in `HashMap` order so the serialised shape
/// does not change between two reads of the same unchanged file, which
/// would make a UI flicker and a test flake.
fn sorted_counts(counts: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Take what one lifecycle record says.
///
/// `permission-mode` and `worktree-state` OVERWRITE: the file is in
/// order, so the last one in the window is the newest, and a mode set
/// twice is one mode, not two. `queue-operation` accumulates, because a
/// queue is the sum of its operations.
fn lifecycle_record(
    kind: &str,
    rec: &serde_json::Value,
    queue: &mut QueueTally,
    out: &mut Lifecycle,
) {
    let field = |key: &str| {
        rec.get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    match kind {
        "queue-operation" => match rec.get("operation").and_then(|o| o.as_str()) {
            Some("enqueue") => queue.enqueued += 1,
            Some("dequeue") => queue.dequeued += 1,
            Some("remove") => match field("reason") {
                // Verbatim, never bucketed. `absorbed_mid_turn` is a
                // prompt the user typed that never ran as its own turn.
                Some(reason) => *queue.removed.entry(reason).or_default() += 1,
                None => queue.removed_unexplained += 1,
            },
            // A fourth operation kind keeps its own name. A vocabulary
            // that grew is information; a bucket named "other" destroys
            // it. An operation field that is absent or not a string is
            // NOT invented into one of the three known kinds.
            Some(other) => *queue.unknown.entry(other.to_owned()).or_default() += 1,
            None => *queue.unknown.entry(String::new()).or_default() += 1,
        },
        // Verbatim. A mode this build has not heard of renders as
        // itself; the set is Claude Code's and is not closed.
        "permission-mode" => {
            if let Some(mode) = field("permissionMode") {
                out.permission_mode = Some(mode);
            }
        }
        "worktree-state" => {
            let Some(ws) = rec.get("worktreeSession") else {
                // No field at all is not the same fact as an explicit
                // `null`, so it is not recorded as one.
                return;
            };
            if ws.is_null() {
                // The session stating it is NOT in a worktree. 82% of
                // real records. A definite answer, kept apart from
                // "no record in the window".
                out.worktree = Worktree::NotInWorktree;
                return;
            }
            let s = |key: &str| {
                ws.get(key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            };
            out.worktree = Worktree::In(WorktreeState {
                original_cwd: s("originalCwd"),
                worktree_path: s("worktreePath"),
                worktree_name: s("worktreeName"),
                worktree_branch: s("worktreeBranch"),
                original_branch: s("originalBranch"),
                original_head_commit: s("originalHeadCommit"),
            });
        }
        // Unreachable: the caller gates on `LIFECYCLE_TYPES`. Doing
        // nothing rather than panicking, because a parser must not take
        // the app down over a record.
        _ => {}
    }
}

/// Match each `tool_use` to its `tool_result` across the message
/// boundary, and classify what did not match (#1209).
///
/// The keys are on the blocks -- `tool_use.id` and
/// `tool_result.tool_use_id`, present on 28,370 and 28,393 of the same
/// number of sampled blocks -- so this is a lookup, not a heuristic.
///
/// The three unmatched cases are not one case. A result whose call is
/// above the window is a consequence of reading a tail and says nothing
/// is wrong. A call with no result is a genuine open question, and its
/// ANSWER depends on whether the session is still running -- which this
/// module does not ask and must not, because `liveness.rs` already
/// answers it and two answers to one question disagree the first time
/// either changes.
fn pair(out: &mut Preview) {
    let mut calls: Vec<&str> = Vec::new();
    let mut results: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in &out.messages {
        for b in &m.blocks {
            match b {
                Block::ToolUse { id: Some(id), .. } => calls.push(id),
                Block::ToolResult {
                    tool_use_id: Some(id),
                    ..
                } => {
                    results.insert(id);
                }
                _ => {}
            }
        }
    }
    let called: std::collections::HashSet<&str> = calls.iter().copied().collect();

    for id in &calls {
        let state = if results.contains(id) {
            Pairing::Paired
        } else {
            Pairing::Unanswered
        };
        if state == Pairing::Unanswered {
            out.unanswered_calls += 1;
        }
        out.pairings.insert((*id).to_owned(), state);
    }
    for id in &results {
        if called.contains(id) {
            continue;
        }
        // A result with no call in the window. The call is OLDER than the
        // 256 KB we read, which is what a tail read does at its edge.
        out.results_above_window += 1;
        out.pairings
            .insert((*id).to_owned(), Pairing::CallAboveWindow);
    }
}

/// The blocks of one `content` value.
///
/// Handles both shapes the corpus actually carries -- a list of blocks
/// (1,500 of 1,586 sampled messages) and a bare string (86) -- because a
/// reader that assumed either one alone is wrong on the other.
fn blocks_of(content: Option<&serde_json::Value>) -> Vec<Block> {
    match content {
        Some(serde_json::Value::String(s)) => vec![text_block(s)],
        Some(serde_json::Value::Array(items)) => items.iter().map(block_of).collect(),
        // Neither shape, including absent. An empty block list renders as
        // a message with nothing in it, which is what it is.
        _ => Vec::new(),
    }
}

/// One content block.
fn block_of(v: &serde_json::Value) -> Block {
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let field = |key: &str| v.get(key).and_then(|s| s.as_str()).unwrap_or("");
    match kind {
        "text" => text_block(field("text")),
        "thinking" => {
            let (text, truncated) = clamp(field("thinking"));
            Block::Thinking { text, truncated }
        }
        "tool_use" => {
            let name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("a tool")
                .to_owned();
            Block::ToolUse {
                // PARSED, not dumped and not discarded. See
                // `Block::ToolUse` on why the original reasoning was
                // right about the blob and wrong about the structure.
                args: tool_args(&name, v.get("input")),
                name,
                id: v
                    .get("id")
                    .and_then(|i| i.as_str())
                    .filter(|i| !i.is_empty())
                    .map(str::to_owned),
            }
        }
        "tool_result" => {
            // A tool result's content is itself either a string or a
            // block array -- the same split as a message's, one level
            // down -- so it is flattened to its text rather than
            // assumed to be a string.
            let (text, truncated) = clamp(&flatten_text(v.get("content")));
            Block::ToolResult {
                text,
                truncated,
                tool_use_id: v
                    .get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .filter(|i| !i.is_empty())
                    .map(str::to_owned),
                // `None` when the record carried no `is_error` at all,
                // which 4,065 of 28,393 sampled results do. Not folded
                // into `false`: "it did not say" is not "it succeeded".
                is_error: v.get("is_error").and_then(serde_json::Value::as_bool),
                // Filled by the caller, which can see the RECORD. A
                // block cannot: `toolUseResult` is a sibling of
                // `message`, not a field inside it.
                change: None,
            }
        }
        other => Block::Other {
            block_type: if other.is_empty() {
                "unknown".to_owned()
            } else {
                other.to_owned()
            },
        },
    }
}

/// One tool call's arguments, as the shape that tool takes.
///
/// Dispatches on the NAME, because the input object carries no type tag
/// of its own. `Task`/`Agent` share a variant: see [`ToolArgs::Task`].
fn tool_args(name: &str, input: Option<&serde_json::Value>) -> ToolArgs {
    let Some(serde_json::Value::Object(map)) = input else {
        // No `input` object at all. Distinct from an input whose keys we
        // did not recognise -- absent is not zero.
        return ToolArgs::None;
    };
    let text = |k: &str| map.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let opt = |k: &str| {
        map.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let flag = |k: &str| map.get(k).and_then(serde_json::Value::as_bool);
    let num = |k: &str| map.get(k).and_then(serde_json::Value::as_i64);

    match name {
        "Edit" => {
            let (old_string, a) = clamp(text("old_string"));
            let (new_string, b) = clamp(text("new_string"));
            ToolArgs::Edit {
                file_path: text("file_path").to_owned(),
                old_string,
                new_string,
                replace_all: flag("replace_all").unwrap_or(false),
                truncated: a || b,
            }
        }
        "MultiEdit" => {
            let all = map
                .get("edits")
                .and_then(|e| e.as_array())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let edits = all
                .iter()
                .take(MAX_EDITS)
                .map(|e| {
                    let f = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("");
                    let (old_string, a) = clamp(f("old_string"));
                    let (new_string, b) = clamp(f("new_string"));
                    Replacement {
                        old_string,
                        new_string,
                        replace_all: e
                            .get("replace_all")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        truncated: a || b,
                    }
                })
                .collect();
            ToolArgs::MultiEdit {
                file_path: text("file_path").to_owned(),
                edits,
                edits_omitted: all.len().saturating_sub(MAX_EDITS),
            }
        }
        "Write" => {
            let (content, truncated) = clamp(text("content"));
            ToolArgs::Write {
                file_path: text("file_path").to_owned(),
                content,
                truncated,
            }
        }
        "Bash" => {
            let (command, truncated) = clamp(text("command"));
            ToolArgs::Bash {
                command,
                description: opt("description"),
                truncated,
            }
        }
        "Read" => ToolArgs::Read {
            file_path: text("file_path").to_owned(),
            offset: num("offset"),
            limit: num("limit"),
        },
        "Grep" => ToolArgs::Grep {
            pattern: text("pattern").to_owned(),
            path: opt("path"),
            output_mode: opt("output_mode"),
        },
        "Glob" => ToolArgs::Glob {
            pattern: text("pattern").to_owned(),
            path: opt("path"),
        },
        // One variant, two spellings. This machine's corpus says `Agent`
        // 236 times and `Task` none; older transcripts say `Task`. A
        // rename is not a distinction.
        "Task" | "Agent" => {
            let (prompt, truncated) = clamp(text("prompt"));
            ToolArgs::Task {
                description: opt("description"),
                subagent_type: opt("subagent_type"),
                prompt,
                truncated,
            }
        }
        // Everything else -- 18 distinct tool names appear in the sample,
        // most of them MCP tools. The KEYS, so a reader can see that
        // Headstate is behind rather than that the call was empty; not
        // the values, which is the blob the original reasoning refused.
        _ => ToolArgs::Other {
            keys: {
                let mut k: Vec<String> = map.keys().cloned().collect();
                // `serde_json` preserves insertion order without the
                // `preserve_order` feature off, and either way two reads
                // of one call must render identically.
                k.sort();
                k
            },
        },
    }
}

/// The file change a record's `toolUseResult` recorded, if it recorded
/// one.
///
/// Takes the RECORD, not the block: see [`FileChange`] on why
/// `toolUseResult` is a sibling of `message` and invisible to anything
/// that only descends into `message.content`.
///
/// Prefers the recorded `structuredPatch` (915 of 915 sampled edit
/// records carry one) and falls back to reconstructing from
/// `oldString`/`newString`. It never reads the file from disk; see
/// [`DiffSource`] for why that option is fabrication rather than a
/// trade-off.
fn file_change(record: &serde_json::Value) -> Option<FileChange> {
    let tur = record.get("toolUseResult")?.as_object()?;
    let file_path = tur
        .get("filePath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let created = match tur.get("type").and_then(|v| v.as_str()) {
        Some("create") => Some(true),
        Some("update") => Some(false),
        // The record said nothing. NOT "it existed" -- 539 of 915 edit
        // records carry no `type` at all.
        _ => None,
    };

    if let Some(patch) = tur.get("structuredPatch").and_then(|p| p.as_array()) {
        // A recorded patch, with the surrounding lines as the file
        // actually was. An EMPTY patch array is still a recorded answer
        // -- "this changed nothing" -- but it is only a `FileChange` at
        // all if the record is about a file, which `filePath` decides.
        if file_path.is_some() || !patch.is_empty() {
            let hunks: Vec<Hunk> = patch.iter().take(MAX_HUNKS).map(recorded_hunk).collect();
            return Some(FileChange {
                file_path,
                source: DiffSource::Recorded,
                hunks,
                hunks_omitted: patch.len().saturating_sub(MAX_HUNKS),
                created,
            });
        }
    }

    // No patch was recorded. Reconstruct the replacement itself, with no
    // surrounding context, and LABEL it as that -- the reader must be
    // able to tell a diff with real context from one that has none.
    let old = tur.get("oldString").and_then(|v| v.as_str());
    let new = tur.get("newString").and_then(|v| v.as_str());
    if old.is_none() && new.is_none() {
        return None;
    }
    Some(FileChange {
        file_path,
        source: DiffSource::Reconstructed,
        hunks: vec![reconstructed_hunk(old.unwrap_or(""), new.unwrap_or(""))],
        hunks_omitted: 0,
        created,
    })
}

/// One hunk of a recorded `structuredPatch`.
///
/// Claude Code writes each line with its unified-diff marker in column
/// zero: `+`, `-` or a space. A line with no marker at all is treated as
/// context, which is what an empty trailing line in a patch is.
fn recorded_hunk(h: &serde_json::Value) -> Hunk {
    let all = h
        .get("lines")
        .and_then(|l| l.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let lines = all
        .iter()
        .filter_map(|l| l.as_str())
        .take(MAX_HUNK_LINES)
        .map(|l| {
            let rest = l.get(1..).unwrap_or("").to_owned();
            match l.as_bytes().first() {
                Some(b'+') => DiffLine::Added { text: rest },
                Some(b'-') => DiffLine::Removed { text: rest },
                Some(b' ') => DiffLine::Context { text: rest },
                _ => DiffLine::Context { text: l.to_owned() },
            }
        })
        .collect();
    Hunk {
        old_start: h.get("oldStart").and_then(serde_json::Value::as_i64),
        new_start: h.get("newStart").and_then(serde_json::Value::as_i64),
        lines,
        // Per hunk. A clipped diff that does not say so is a lie about
        // what changed.
        lines_omitted: all.len().saturating_sub(MAX_HUNK_LINES),
    }
}

/// A hunk reconstructed from the replacement strings alone.
///
/// No line numbers and no context, because nothing recorded any. The
/// [`DiffSource::Reconstructed`] label is what stops this being read as
/// the narrower thing it is not.
fn reconstructed_hunk(old: &str, new: &str) -> Hunk {
    let mut lines = Vec::new();
    let mut omitted = 0usize;
    // One budget SHARED across both sides, so a 10,000-line `old_string`
    // cannot push the replacement off the end entirely -- and so the
    // omission count is exact rather than per-side.
    let mut budget = MAX_HUNK_LINES;
    let removed = |text: String| DiffLine::Removed { text };
    let added = |text: String| DiffLine::Added { text };
    let ctors: [(&str, &dyn Fn(String) -> DiffLine); 2] = [(old, &removed), (new, &added)];
    for (text, ctor) in ctors {
        if text.is_empty() {
            continue;
        }
        let total = text.lines().count();
        let taken = total.min(budget);
        for l in text.lines().take(taken) {
            lines.push(ctor(l.to_owned()));
        }
        budget -= taken;
        omitted += total - taken;
    }
    Hunk {
        // No line numbers: a reconstructed hunk does not know where in
        // the file it sits, and inventing one would be the same class of
        // fabrication as reading the file back.
        old_start: None,
        new_start: None,
        lines,
        lines_omitted: omitted,
    }
}

fn text_block(s: &str) -> Block {
    let (text, truncated) = clamp(s);
    Block::Text { text, truncated }
}

/// Whatever text is inside a nested `content`, concatenated.
fn flatten_text(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Bound one block's text, reporting whether it bound.
///
/// Counts CHARACTERS, not bytes, and truncates on a char boundary:
/// slicing a UTF-8 string at a byte index panics mid-codepoint, and
/// transcripts carry every language a user writes in.
fn clamp(s: &str) -> (String, bool) {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= MAX_TEXT_CHARS {
            return (out, true);
        }
        out.push(c);
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "headstate-preview-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn renders_the_four_block_kinds_the_corpus_carries() {
        // 1,500 of 1,586 sampled messages carry a LIST, and the blocks
        // are tool_use 510, tool_result 509, text 286, thinking 195. A
        // reader that assumed a string shows nothing for 95% of them.
        let tmp = Tmp::new("blocks");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-09-13T10:00:00Z","message":{"role":"assistant","model":"claude-opus-5","content":[{"type":"text","text":"Looking now."},{"type":"thinking","thinking":"hmm"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/tmp/x","offset":10,"limit":20}}]}}"#,
                r#"{"type":"user","timestamp":"2026-09-13T10:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":false,"content":[{"type":"text","text":"file contents"}]}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(v.messages.len(), 2);
        assert_eq!(v.messages[0].role, "assistant");
        assert_eq!(v.messages[0].model.as_deref(), Some("claude-opus-5"));
        assert_eq!(
            v.messages[0].blocks,
            vec![
                Block::Text {
                    text: "Looking now.".into(),
                    truncated: false
                },
                Block::Thinking {
                    text: "hmm".into(),
                    truncated: false
                },
                Block::ToolUse {
                    name: "Read".into(),
                    id: Some("toolu_1".into()),
                    // PARSED, not discarded and not dumped: the reader
                    // now learns WHICH file and which window of it.
                    args: ToolArgs::Read {
                        file_path: "/tmp/x".into(),
                        offset: Some(10),
                        limit: Some(20),
                    },
                },
            ]
        );
        assert_eq!(
            v.messages[1].blocks,
            vec![Block::ToolResult {
                text: "file contents".into(),
                truncated: false,
                tool_use_id: Some("toolu_1".into()),
                is_error: Some(false),
                change: None,
            }]
        );
    }

    #[test]
    fn a_bare_string_content_still_renders() {
        // 86 of 1,586 sampled messages carry `content` as a string.
        let tmp = Tmp::new("string");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"user","message":{"role":"user","content":"just text"}}"#],
        );
        let v = tail(&p).unwrap();
        assert_eq!(
            v.messages[0].blocks,
            vec![Block::Text {
                text: "just text".into(),
                truncated: false
            }]
        );
    }

    #[test]
    fn machinery_records_are_excluded_and_counted() {
        // Fourteen of the sixteen record types are bookkeeping. An
        // exclusion the reader cannot see invites them to conclude the
        // reader is broken -- the same argument `subagent_files_skipped`
        // carries.
        let tmp = Tmp::new("machinery");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"atis-latch"}"#,
                r#"{"type":"file-history-delta","delta":{}}"#,
                r#"{"type":"ai-title","title":"x"}"#,
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(v.messages.len(), 1);
        assert_eq!(v.non_conversation_records, 3);
        assert_eq!(v.unparseable_records, 0);
    }

    #[test]
    fn an_unknown_record_type_is_excluded_not_rendered() {
        // The allowlist's whole point: a future record type degrades to
        // showing LESS, never to showing a reader internal state.
        let tmp = Tmp::new("future");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"some-future-record","secret":"internals"}"#],
        );
        let v = tail(&p).unwrap();
        assert!(v.messages.is_empty());
        assert_eq!(v.non_conversation_records, 1);
    }

    #[test]
    fn an_unknown_block_kind_is_reported_not_dropped() {
        // A hole in an exchange with no sign that anything is missing is
        // worse than a labelled one.
        let tmp = Tmp::new("block");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"assistant","message":{"content":[{"type":"image","source":{}}]}}"#],
        );
        let v = tail(&p).unwrap();
        assert_eq!(
            v.messages[0].blocks,
            vec![Block::Other {
                block_type: "image".into()
            }]
        );
    }

    #[test]
    fn an_unparseable_line_is_counted_separately_from_a_skipped_one() {
        // "We could not read this" and "we understood it and chose not to
        // show it" are different facts with different remedies.
        let tmp = Tmp::new("torn");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"typ"#,
                r#"{"type":"mode","mode":"default"}"#,
                r#"{"type":"user","message":{"role":"user","content":"ok"}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(v.unparseable_records, 1);
        assert_eq!(v.non_conversation_records, 1);
        assert_eq!(v.messages.len(), 1);
    }

    #[test]
    fn a_file_past_the_window_says_it_is_showing_a_tail() {
        // The 39 real files over 1 MB. Without this flag the reader
        // cannot tell a short conversation from a truncated one, which is
        // #846 in its purest form.
        let tmp = Tmp::new("big");
        let p = tmp.path().join("big.jsonl");
        {
            let mut f = std::fs::File::create(&p).unwrap();
            let filler = format!(
                r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#,
                "x".repeat(2_000)
            );
            let mut written: u64 = 0;
            while written <= TAIL_BYTES + 32 * 1024 {
                writeln!(f, "{filler}").unwrap();
                written += filler.len() as u64 + 1;
            }
            writeln!(
                f,
                r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"the end"}}]}}}}"#
            )
            .unwrap();
        }
        let v = tail(&p).unwrap();
        assert!(v.truncated, "a tail read must say it is a tail");
        assert!(v.file_bytes > v.bytes_read);
        assert!(v.bytes_read <= TAIL_BYTES);
        // The LAST exchange survives, which is the one a user wants.
        let last = v.messages.last().unwrap();
        assert_eq!(
            last.blocks,
            vec![Block::Text {
                text: "the end".into(),
                truncated: false
            }]
        );
    }

    #[test]
    fn a_short_transcript_does_not_claim_to_be_a_tail() {
        // The happy-path pair. 97.4% of the corpus is under 1 MB, so the
        // common case must not wear a "showing the last N KB" label it
        // has not earned.
        let tmp = Tmp::new("short");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"user","message":{"role":"user","content":"hi"}}"#],
        );
        let v = tail(&p).unwrap();
        assert!(!v.truncated);
        assert_eq!(v.bytes_read, v.file_bytes);
    }

    #[test]
    fn more_messages_than_the_cap_keeps_the_newest_and_says_so() {
        // The second bound: 256 KB of short records is thousands of
        // messages, and drawing thousands is the hang the byte cap was
        // meant to prevent arrived at by another route.
        let tmp = Tmp::new("many");
        let p = tmp.path().join("many.jsonl");
        {
            let mut f = std::fs::File::create(&p).unwrap();
            for i in 0..(MAX_MESSAGES + 20) {
                writeln!(
                    f,
                    r#"{{"type":"user","message":{{"role":"user","content":"m{i}"}}}}"#
                )
                .unwrap();
            }
        }
        let v = tail(&p).unwrap();
        assert_eq!(v.messages.len(), MAX_MESSAGES);
        assert!(v.truncated);
        // The NEWEST kept: the last exchange is what decides a resume.
        assert_eq!(
            v.messages.last().unwrap().blocks,
            vec![Block::Text {
                text: format!("m{}", MAX_MESSAGES + 19),
                truncated: false
            }]
        );
    }

    #[test]
    fn a_long_block_is_clamped_on_a_char_boundary() {
        // A tool_result can be a whole file, and transcripts carry every
        // language a user writes in -- slicing UTF-8 at a byte index
        // panics mid-codepoint.
        let tmp = Tmp::new("clamp");
        let long: String = "é".repeat(MAX_TEXT_CHARS + 500);
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[&format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
            )],
        );
        let v = tail(&p).unwrap();
        match &v.messages[0].blocks[0] {
            Block::Text { text, truncated } => {
                assert!(truncated);
                assert_eq!(text.chars().count(), MAX_TEXT_CHARS);
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Pairing and argument rendering (#1209).
    // ---------------------------------------------------------------

    #[test]
    fn a_call_and_its_result_are_paired_across_the_message_boundary() {
        // The key is on the blocks and the blocks are on DIFFERENT
        // messages -- the call on an `assistant` record and the result on
        // the `user` record after it. Nothing joined them before this.
        let tmp = Tmp::new("pair");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_A","name":"Bash","input":{"command":"ls -la","description":"List files"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_A","content":"a.txt"}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(v.pairings.get("toolu_A"), Some(&Pairing::Paired));
        assert_eq!(v.unanswered_calls, 0);
        assert_eq!(v.results_above_window, 0);
        // And the call now says WHAT it ran, not merely that Bash ran.
        assert_eq!(
            v.messages[0].blocks,
            vec![Block::ToolUse {
                name: "Bash".into(),
                id: Some("toolu_A".into()),
                args: ToolArgs::Bash {
                    command: "ls -la".into(),
                    description: Some("List files".into()),
                    truncated: false,
                },
            }]
        );
    }

    /// The three orphan cases must not render identically.
    ///
    /// This asserts the two the MODULE distinguishes -- a result whose
    /// call is above the window, and a call with no result. The third
    /// (`Unanswered` on a dead session) is the same `Pairing` resolved
    /// against `liveness.rs`'s answer, and it is asserted where that
    /// resolution happens: `ClaudeCodePage.test.tsx`. Deriving liveness
    /// here would be the second answer to one question that
    /// `liveness.rs`'s module docs forbid.
    #[test]
    fn an_orphan_result_and_an_orphan_call_are_different_states() {
        let tmp = Tmp::new("orphans");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                // A result whose call is above the window: the window
                // starts here, so nothing ever issued `toolu_OLD`.
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_OLD","content":"output from before"}]}}"#,
                // A call whose result never arrives.
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_NEW","name":"Bash","input":{"command":"sleep 600"}}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(
            v.pairings.get("toolu_OLD"),
            Some(&Pairing::CallAboveWindow),
            "a result with no call means the call is older than the window"
        );
        assert_eq!(
            v.pairings.get("toolu_NEW"),
            Some(&Pairing::Unanswered),
            "a call with no result is an open question, not a stale window"
        );
        // The two are counted separately, because they are different
        // facts with different remedies: one is "read more of the file",
        // the other is "the session may have died".
        assert_eq!(v.results_above_window, 1);
        assert_eq!(v.unanswered_calls, 1);
        assert_ne!(
            v.pairings.get("toolu_OLD"),
            v.pairings.get("toolu_NEW"),
            "collapsing the two loses the distinction that makes this worth having"
        );
    }

    #[test]
    fn a_call_with_no_id_is_unpairable_rather_than_unanswered() {
        // "We could not look" is not "we looked and found nothing" --
        // the same split `unparseable_records` draws against
        // `non_conversation_records`. A call with no key is not evidence
        // that a session died.
        let tmp = Tmp::new("unkeyed");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"x"}}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert!(v.pairings.is_empty());
        assert_eq!(
            v.unanswered_calls, 0,
            "an unkeyed call must not be counted as a call that never came back"
        );
        match &v.messages[0].blocks[0] {
            Block::ToolUse { id, .. } => assert_eq!(*id, None),
            other => panic!("expected a tool_use, got {other:?}"),
        }
    }

    #[test]
    fn a_recorded_patch_is_labelled_differently_from_a_reconstructed_one() {
        // The heart of the diff half of #1209. Both records describe an
        // edit; only the first wrote down the file's surrounding lines.
        // Rendering both as "a diff" tells the reader the second has
        // context it does not have.
        let tmp = Tmp::new("diffsrc");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/a.rs","old_string":"let x=1;","new_string":"let x=2;"}}]}}"#,
                r#"{"type":"user","toolUseResult":{"filePath":"/a.rs","structuredPatch":[{"oldStart":10,"newStart":10,"lines":[" fn main() {","-    let x=1;","+    let x=2;"," }"]}]},"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/b.rs","old_string":"let y=1;","new_string":"let y=2;"}}]}}"#,
                r#"{"type":"user","toolUseResult":{"filePath":"/b.rs","oldString":"let y=1;","newString":"let y=2;"},"message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();

        let recorded = match &v.messages[1].blocks[0] {
            Block::ToolResult { change, .. } => change.clone().expect("a change was recorded"),
            other => panic!("expected a tool_result, got {other:?}"),
        };
        let reconstructed = match &v.messages[3].blocks[0] {
            Block::ToolResult { change, .. } => change.clone().expect("a change was recorded"),
            other => panic!("expected a tool_result, got {other:?}"),
        };

        assert_eq!(recorded.source, DiffSource::Recorded);
        assert_eq!(reconstructed.source, DiffSource::Reconstructed);
        assert_ne!(
            recorded.source, reconstructed.source,
            "two different epistemic objects must be distinguishable"
        );

        // The recorded one carries CONTEXT lines and real line numbers,
        // which is exactly what the other cannot have.
        assert_eq!(recorded.hunks[0].old_start, Some(10));
        assert!(
            recorded.hunks[0]
                .lines
                .iter()
                .any(|l| matches!(l, DiffLine::Context { .. })),
            "a recorded patch carries the surrounding lines as they were"
        );

        // The reconstructed one has no line numbers and no context,
        // because nothing recorded any. Inventing either would be the
        // same fabrication as reading the file back off disk.
        assert_eq!(reconstructed.hunks[0].old_start, None);
        assert_eq!(reconstructed.hunks[0].new_start, None);
        assert!(
            !reconstructed.hunks[0]
                .lines
                .iter()
                .any(|l| matches!(l, DiffLine::Context { .. })),
            "a reconstructed hunk has no context to show"
        );
        assert_eq!(
            reconstructed.hunks[0].lines,
            vec![
                DiffLine::Removed {
                    text: "let y=1;".into()
                },
                DiffLine::Added {
                    text: "let y=2;".into()
                },
            ]
        );
    }

    /// A clipped diff says so, PER HUNK.
    ///
    /// The fixture genuinely exceeds `MAX_HUNK_LINES` -- a small one read
    /// whole proves nothing about a bound that never bound. Two hunks,
    /// one over the limit and one under, so the test also proves the
    /// statement is per hunk and not a single flag for the pane: a
    /// reader told "something here was clipped" still cannot tell which
    /// region is short.
    #[test]
    fn a_clipped_hunk_says_so_and_an_unclipped_one_does_not() {
        let tmp = Tmp::new("clip");
        let big: Vec<String> = (0..MAX_HUNK_LINES + 40)
            .map(|i| format!("\"+added line {i}\""))
            .collect();
        let small = r#"" context","+one added""#;
        let line = format!(
            r#"{{"type":"user","toolUseResult":{{"filePath":"/big.rs","structuredPatch":[{{"oldStart":1,"newStart":1,"lines":[{}]}},{{"oldStart":900,"newStart":900,"lines":[{}]}}]}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"t9","content":"ok"}}]}}}}"#,
            big.join(","),
            small
        );
        let p = write(tmp.path(), "s.jsonl", &[&line]);
        let v = tail(&p).unwrap();
        let change = match &v.messages[0].blocks[0] {
            Block::ToolResult { change, .. } => change.clone().expect("a change was recorded"),
            other => panic!("expected a tool_result, got {other:?}"),
        };
        assert_eq!(change.hunks.len(), 2);

        // The bound actually bound -- the fixture is genuinely over it.
        assert_eq!(change.hunks[0].lines.len(), MAX_HUNK_LINES);
        assert_eq!(
            change.hunks[0].lines_omitted, 40,
            "the clipped hunk must say how much is missing, not merely that some is"
        );

        // And the hunk that was NOT clipped must not claim it was: a
        // false clip warning is its own lie about what changed.
        assert_eq!(change.hunks[1].lines.len(), 2);
        assert_eq!(change.hunks[1].lines_omitted, 0);
    }

    #[test]
    fn a_patch_with_more_hunks_than_the_cap_says_how_many_are_missing() {
        let tmp = Tmp::new("hunks");
        let hunks: Vec<String> = (0..MAX_HUNKS + 7)
            .map(|i| {
                format!(
                    r#"{{"oldStart":{i},"newStart":{i},"lines":["+line {i}"]}}"#,
                    i = i + 1
                )
            })
            .collect();
        let line = format!(
            r#"{{"type":"user","toolUseResult":{{"filePath":"/m.rs","structuredPatch":[{}]}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"tz","content":"ok"}}]}}}}"#,
            hunks.join(",")
        );
        let p = write(tmp.path(), "s.jsonl", &[&line]);
        let v = tail(&p).unwrap();
        let change = match &v.messages[0].blocks[0] {
            Block::ToolResult { change, .. } => change.clone().unwrap(),
            other => panic!("expected a tool_result, got {other:?}"),
        };
        assert_eq!(change.hunks.len(), MAX_HUNKS);
        assert_eq!(change.hunks_omitted, 7);
    }

    #[test]
    fn a_write_records_a_creation_rather_than_an_edit() {
        // 357 of 915 sampled edit records carry `type: "create"` with a
        // NULL `originalFile` -- there was no original. Rendering that as
        // an edit to an empty file would be a claim about a file that did
        // not exist.
        let tmp = Tmp::new("create");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","toolUseResult":{"type":"create","filePath":"/new.md","originalFile":null,"structuredPatch":[{"oldStart":1,"newStart":1,"lines":["+hello"]}]},"message":{"content":[{"type":"tool_result","tool_use_id":"tc","content":"created"}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        let change = match &v.messages[0].blocks[0] {
            Block::ToolResult { change, .. } => change.clone().unwrap(),
            other => panic!("expected a tool_result, got {other:?}"),
        };
        assert_eq!(change.created, Some(true));
        assert_eq!(change.source, DiffSource::Recorded);
    }

    #[test]
    fn a_result_that_recorded_no_change_carries_none() {
        // Most results are not edits at all -- 23,889 of 28,393 sampled
        // `toolUseResult`s are Bash stdout/stderr. A `FileChange` there
        // would be a diff invented from nothing.
        let tmp = Tmp::new("nochange");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","toolUseResult":{"stdout":"hi","stderr":"","interrupted":false,"isImage":false},"message":{"content":[{"type":"tool_result","tool_use_id":"tb","content":"hi"}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        match &v.messages[0].blocks[0] {
            Block::ToolResult { change, .. } => assert_eq!(*change, None),
            other => panic!("expected a tool_result, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_tool_reports_its_argument_keys_rather_than_its_values() {
        // `Block::Other`'s guarantee, one level down. 18 distinct tool
        // names appear in the sample and most are MCP tools that did not
        // exist when this pane was written; a pane that dropped their
        // arguments silently would show a call with no sign anything was
        // missing, and one that dumped them would be the 40 KB blob the
        // original reasoning was right to refuse.
        let tmp = Tmp::new("unknowntool");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tm","name":"mcp__enclave__enclave_sql","input":{"query":"SELECT secret FROM t","enclave":"one"}}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        match &v.messages[0].blocks[0] {
            Block::ToolUse { name, args, .. } => {
                assert_eq!(name, "mcp__enclave__enclave_sql");
                // Sorted, so two reads of one call cannot differ by map
                // order.
                assert_eq!(
                    *args,
                    ToolArgs::Other {
                        keys: vec!["enclave".into(), "query".into()]
                    }
                );
            }
            other => panic!("expected a tool_use, got {other:?}"),
        }
    }

    #[test]
    fn a_call_with_no_input_is_distinct_from_one_with_unknown_arguments() {
        // Absent is not zero: "no arguments were recorded" and
        // "arguments we did not recognise" are different readings.
        let tmp = Tmp::new("noinput");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tn","name":"Whatever"},{"type":"tool_use","id":"to","name":"Whatever","input":{}}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        let args: Vec<&ToolArgs> = v.messages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { args, .. } => Some(args),
                _ => None,
            })
            .collect();
        assert_eq!(*args[0], ToolArgs::None);
        assert_eq!(*args[1], ToolArgs::Other { keys: vec![] });
        assert_ne!(args[0], args[1]);
    }

    #[test]
    fn a_huge_bash_command_is_clamped_and_says_so() {
        // The arguments go through the SAME bound as any other text --
        // adding a parsed shape must not reopen the 40 KB blob by
        // another door. The fixture genuinely exceeds the limit.
        let tmp = Tmp::new("bigcmd");
        let long = "x".repeat(MAX_TEXT_CHARS + 500);
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[&format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"tq","name":"Bash","input":{{"command":"{long}"}}}}]}}}}"#
            )],
        );
        let v = tail(&p).unwrap();
        match &v.messages[0].blocks[0] {
            Block::ToolUse {
                args: ToolArgs::Bash {
                    command, truncated, ..
                },
                ..
            } => {
                assert!(truncated, "a clipped command must say it was clipped");
                assert_eq!(command.chars().count(), MAX_TEXT_CHARS);
            }
            other => panic!("expected a Bash call, got {other:?}"),
        }
    }

    #[test]
    fn a_multi_edit_lists_its_replacements_and_counts_the_ones_it_drops() {
        let tmp = Tmp::new("multi");
        let edits: Vec<String> = (0..MAX_EDITS + 5)
            .map(|i| format!(r#"{{"old_string":"a{i}","new_string":"b{i}"}}"#))
            .collect();
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"tme","name":"MultiEdit","input":{{"file_path":"/x.rs","edits":[{}]}}}}]}}}}"#,
            edits.join(",")
        );
        let p = write(tmp.path(), "s.jsonl", &[&line]);
        let v = tail(&p).unwrap();
        match &v.messages[0].blocks[0] {
            Block::ToolUse {
                args:
                    ToolArgs::MultiEdit {
                        file_path,
                        edits,
                        edits_omitted,
                    },
                ..
            } => {
                assert_eq!(file_path, "/x.rs");
                assert_eq!(edits.len(), MAX_EDITS);
                assert_eq!(*edits_omitted, 5);
            }
            other => panic!("expected a MultiEdit call, got {other:?}"),
        }
    }

    #[test]
    fn task_and_agent_are_one_shape_because_a_rename_is_not_a_distinction() {
        let tmp = Tmp::new("task");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Task","input":{"description":"find it","prompt":"go","subagent_type":"Explore"}},{"type":"tool_use","id":"t2","name":"Agent","input":{"description":"find it","prompt":"go","subagent_type":"Explore"}}]}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        let args: Vec<&ToolArgs> = v.messages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { args, .. } => Some(args),
                _ => None,
            })
            .collect();
        assert_eq!(args[0], args[1]);
        assert!(matches!(args[0], ToolArgs::Task { .. }));
    }

    /// Pairing describes the window the reader is SHOWN, not the window
    /// that was read.
    ///
    /// The message cap drops the oldest messages, and a call dropped by
    /// it leaves its result behind. Pairing computed before the cap would
    /// mark that result `Paired` and the pane would render "paired"
    /// beside a block with nothing in view to pair to. The fixture
    /// genuinely exceeds `MAX_MESSAGES`.
    #[test]
    fn a_call_dropped_by_the_message_cap_leaves_its_result_above_the_window() {
        let tmp = Tmp::new("capped");
        let pth = tmp.path().join("c.jsonl");
        {
            let mut f = std::fs::File::create(&pth).unwrap();
            // The call, then enough messages to push it past the cap.
            writeln!(
                f,
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"toolu_FAR","name":"Bash","input":{{"command":"x"}}}}]}}}}"#
            )
            .unwrap();
            for i in 0..MAX_MESSAGES {
                writeln!(
                    f,
                    r#"{{"type":"user","message":{{"role":"user","content":"m{i}"}}}}"#
                )
                .unwrap();
            }
            writeln!(
                f,
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_FAR","content":"late"}}]}}}}"#
            )
            .unwrap();
        }
        let v = tail(&pth).unwrap();
        assert_eq!(v.messages.len(), MAX_MESSAGES);
        assert!(v.truncated);
        assert_eq!(
            v.pairings.get("toolu_FAR"),
            Some(&Pairing::CallAboveWindow),
            "a call the cap dropped is above the window the reader sees"
        );
        assert_eq!(v.unanswered_calls, 0);
    }

    /// The parser against the REAL corpus, not only fixtures.
    ///
    /// `#[ignore]` for the reason the other corpus tests in this tree
    /// are: it needs `~/.claude/projects/` to exist with real
    /// transcripts, which a CI runner does not have. Run with
    /// `cargo test -- --ignored real_transcripts_pair`.
    #[test]
    #[ignore = "reads the developer's real ~/.claude corpus"]
    fn real_transcripts_pair_and_carry_arguments() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let root = PathBuf::from(home).join(".claude").join("projects");
        let Ok(dirs) = std::fs::read_dir(&root) else {
            return;
        };
        let mut files = Vec::new();
        for d in dirs.flatten() {
            let Ok(inner) = std::fs::read_dir(d.path()) else {
                continue;
            };
            for f in inner.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|e| e == "jsonl")
                    && f.metadata().is_ok_and(|m| m.len() > 50_000)
                {
                    files.push(p);
                }
            }
            if files.len() >= 40 {
                break;
            }
        }
        if files.is_empty() {
            return;
        }

        let (mut paired, mut above, mut unanswered) = (0usize, 0usize, 0usize);
        let (mut recorded, mut reconstructed, mut parsed_args) = (0usize, 0usize, 0usize);
        for f in &files {
            let Ok(v) = tail(f) else { continue };
            for state in v.pairings.values() {
                match state {
                    Pairing::Paired => paired += 1,
                    Pairing::CallAboveWindow => above += 1,
                    Pairing::Unanswered => unanswered += 1,
                    Pairing::Unkeyed => {}
                }
            }
            for m in &v.messages {
                for b in &m.blocks {
                    match b {
                        Block::ToolUse { args, .. } => {
                            if !matches!(args, ToolArgs::Other { .. } | ToolArgs::None) {
                                parsed_args += 1;
                            }
                        }
                        Block::ToolResult {
                            change: Some(c), ..
                        } => match c.source {
                            DiffSource::Recorded => recorded += 1,
                            DiffSource::Reconstructed => reconstructed += 1,
                        },
                        _ => {}
                    }
                }
            }
        }
        // The pairing key is on the blocks in the real corpus, so the
        // overwhelming majority of calls inside one window DO pair. If
        // this is zero the parser is reading a field that is not there.
        assert!(
            paired > 0,
            "no call paired across {} real transcripts",
            files.len()
        );
        assert!(parsed_args > 0, "no tool arguments parsed from real calls");
        // Orphans are the normal consequence of a tail read, so at least
        // one of the two orphan kinds should appear over 40 files.
        assert!(
            above + unanswered > 0,
            "a 256 KB tail read over {} files produced no orphan at all, which \
             means the window never cut between a call and its result -- \
             suspicious enough to check the parser",
            files.len()
        );
        eprintln!(
            "real corpus: {} files, paired {paired}, above-window {above}, \
             unanswered {unanswered}, recorded diffs {recorded}, \
             reconstructed {reconstructed}, parsed args {parsed_args}",
            files.len()
        );
    }

    #[test]
    fn a_truncated_read_reports_the_queue_as_unknown_not_as_a_short_list() {
        // The balance argument (#1206). The fixture genuinely exceeds
        // TAIL_BYTES so the window really binds -- a small fixture that
        // gets read whole proves nothing about truncation.
        //
        // The enqueues are at the START, past the cut; the dequeues at
        // the END, inside it. A reader that just summed what it saw
        // would report -3: a NEGATIVE queue, an answer that looks like
        // an answer. The only honest report is `None`.
        let tmp = Tmp::new("qtrunc");
        let p = tmp.path().join("q.jsonl");
        {
            let mut f = std::fs::File::create(&p).unwrap();
            for i in 0..3 {
                writeln!(
                    f,
                    r#"{{"type":"queue-operation","operation":"enqueue","content":"early{i}"}}"#
                )
                .unwrap();
            }
            let filler = format!(
                r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#,
                "x".repeat(2_000)
            );
            let mut written: u64 = 0;
            while written <= TAIL_BYTES + 64 * 1024 {
                writeln!(f, "{filler}").unwrap();
                written += filler.len() as u64 + 1;
            }
            for _ in 0..3 {
                writeln!(f, r#"{{"type":"queue-operation","operation":"dequeue"}}"#).unwrap();
            }
        }

        let v = tail(&p).unwrap();
        // The window really bound -- otherwise this test proves nothing.
        assert!(v.truncated, "fixture must actually exceed the window");
        assert!(
            v.file_bytes > v.bytes_read,
            "fixture must not have been read whole: {} vs {}",
            v.file_bytes,
            v.bytes_read
        );
        assert_eq!(
            v.lifecycle.queue, None,
            "a truncated read must report the queue as unknown, not as a partial balance"
        );
    }

    #[test]
    fn an_untruncated_read_reports_the_queue_as_a_balance() {
        // The pair to the test above: `None` must mean "truncated", not
        // "this parser never reports a queue". Without this, the
        // truncation test would pass against a field that is always
        // None.
        let tmp = Tmp::new("qwhole");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"queue-operation","operation":"enqueue","content":"a"}"#,
                r#"{"type":"queue-operation","operation":"enqueue","content":"b"}"#,
                r#"{"type":"queue-operation","operation":"enqueue","content":"c"}"#,
                r#"{"type":"queue-operation","operation":"dequeue"}"#,
                r#"{"type":"queue-operation","operation":"remove","content":"c","reason":"absorbed_mid_turn"}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert!(!v.truncated);
        let q = v
            .lifecycle
            .queue
            .expect("a whole read can state the balance");
        assert_eq!(q.enqueued, 3);
        assert_eq!(q.dequeued, 1);
        // The reason is kept verbatim: a prompt typed that never ran as
        // its own turn is information.
        assert_eq!(q.removed, vec![("absorbed_mid_turn".to_owned(), 1)]);
        assert_eq!(q.removed_unexplained, 0);
        assert_eq!(q.pending(), Some(1));
    }

    #[test]
    fn a_session_with_no_permission_mode_record_reports_it_as_absent() {
        // #1206's third trap, and #846 in its exact original form. The
        // record is written on CHANGE, not continuously -- 48 records
        // against 411 assistant messages -- so a session whose mode sits
        // outside the window is the COMMON case. Rendering that as
        // "ask every time" or as "auto" would be a benign-looking
        // default standing in for a measurement nobody made.
        let tmp = Tmp::new("nomode");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":"ok"}}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(
            v.lifecycle.permission_mode, None,
            "an unread permission mode must be absent, never a default value"
        );
        // And nothing else silently stood in for it either.
        assert_eq!(v.lifecycle.worktree, Worktree::Unknown);
    }

    #[test]
    fn a_permission_mode_record_is_read_verbatim_and_the_newest_wins() {
        // The pair: `None` must mean "not observed", not "never read".
        // And the vocabulary is not ours -- a mode this build has not
        // heard of renders as itself, never bucketed into "other".
        let tmp = Tmp::new("mode");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"s"}"#,
                r#"{"type":"permission-mode","permissionMode":"a-mode-from-the-future","sessionId":"s"}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert_eq!(
            v.lifecycle.permission_mode.as_deref(),
            Some("a-mode-from-the-future"),
            "the newest record wins and its value is verbatim"
        );
    }

    #[test]
    fn an_unrecognised_record_type_is_still_counted_and_dropped() {
        // The allowlist was WIDENED by three named types, not loosened
        // into a passthrough. The module docs' guarantee -- an unknown
        // record degrades to showing LESS, never to showing a reader
        // internal state -- is unchanged by #1206.
        let tmp = Tmp::new("widen");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"some-future-record","secret":"internals"}"#,
                r#"{"type":"atis-latch"}"#,
                r#"{"type":"cost-state","cost":1}"#,
                // The three newly-read types, to prove the widening is
                // by name and the count excludes exactly them.
                r#"{"type":"queue-operation","operation":"enqueue","content":"q"}"#,
                r#"{"type":"permission-mode","permissionMode":"auto"}"#,
                r#"{"type":"worktree-state","worktreeSession":null}"#,
            ],
        );
        let v = tail(&p).unwrap();
        assert!(v.messages.is_empty(), "none of these is conversation");
        assert_eq!(
            v.non_conversation_records, 3,
            "the three unrecognised types are still counted and dropped"
        );
        // And nothing of the unknown record leaked into what was read.
        assert_eq!(v.lifecycle.permission_mode.as_deref(), Some("auto"));
        assert_eq!(v.lifecycle.worktree, Worktree::NotInWorktree);
    }

    #[test]
    fn the_two_allowlists_do_not_overlap() {
        // A type in both lists would be read as lifecycle and never
        // reach the conversation branch -- a silent loss of messages.
        for k in LIFECYCLE_TYPES {
            assert!(
                !CONVERSATION_TYPES.contains(&k),
                "{k} is in both allowlists"
            );
        }
    }

    #[test]
    fn a_null_worktree_session_is_a_definite_answer_not_an_absent_one() {
        // Measured over 1,907 real `worktree-state` records: 1,564 (82%)
        // carry `worktreeSession: null`. That is the session stating it
        // is NOT in a worktree. Collapsing it into "no record" would
        // report "unknown" for the 82% that are an answer.
        let tmp = Tmp::new("wtnull");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"worktree-state","worktreeSession":null,"sessionId":"s"}"#],
        );
        assert_eq!(
            tail(&p).unwrap().lifecycle.worktree,
            Worktree::NotInWorktree
        );

        // And a record with no `worktreeSession` field at all is a third
        // thing again: not read as an explicit null.
        let p2 = write(
            tmp.path(),
            "t.jsonl",
            &[r#"{"type":"worktree-state","sessionId":"s"}"#],
        );
        assert_eq!(tail(&p2).unwrap().lifecycle.worktree, Worktree::Unknown);
    }

    #[test]
    fn a_worktree_state_keeps_the_commit_the_path_join_cannot_recover() {
        // `originalBranch` and `originalHeadCommit` are the facts #1137's
        // path-based join cannot recover once the directory is gone.
        let tmp = Tmp::new("wt");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"worktree-state","worktreeSession":{"originalCwd":"/c/proj","preEnterOriginalCwd":"/c/proj","worktreePath":"/c/proj/.claude/worktrees/w","worktreeName":"w","worktreeBranch":"worktree-w","originalBranch":"main","originalHeadCommit":"03e8473bd378e39cbe7bfc29939516058958ef68","sessionId":"s"},"sessionId":"s"}"#,
            ],
        );
        let got = tail(&p).unwrap().lifecycle.worktree;
        assert_eq!(
            got,
            Worktree::In(WorktreeState {
                original_cwd: Some("/c/proj".into()),
                worktree_path: Some("/c/proj/.claude/worktrees/w".into()),
                worktree_name: Some("w".into()),
                worktree_branch: Some("worktree-w".into()),
                original_branch: Some("main".into()),
                original_head_commit: Some("03e8473bd378e39cbe7bfc29939516058958ef68".into()),
            })
        );
    }

    #[test]
    fn an_unknown_queue_operation_keeps_its_own_name() {
        // "A vocabulary that grew is information; a bucket named 'other'
        // destroys it." A fourth operation kind must not be folded into
        // enqueue, dequeue or remove.
        let tmp = Tmp::new("qunk");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"queue-operation","operation":"enqueue","content":"a"}"#,
                r#"{"type":"queue-operation","operation":"reprioritise"}"#,
                r#"{"type":"queue-operation","operation":"reprioritise"}"#,
                r#"{"type":"queue-operation","operation":"remove","content":"a"}"#,
            ],
        );
        let q = tail(&p).unwrap().lifecycle.queue.unwrap();
        assert_eq!(q.enqueued, 1);
        assert_eq!(q.dequeued, 0);
        assert_eq!(
            q.unknown_operations,
            vec![("reprioritise".to_owned(), 2)],
            "an unknown operation is kept verbatim, not bucketed"
        );
        // A `remove` with no reason is its own fact, not an invented one.
        assert_eq!(q.removed_unexplained, 1);
        assert!(q.removed.is_empty());
        assert_eq!(q.pending(), Some(0));
    }

    #[test]
    fn an_impossible_queue_balance_is_unknown_not_saturated_to_zero() {
        // Within an untruncated window this should not arise, but the
        // file is a format Claude Code owns. Saturating would report
        // "nothing queued" for a file we did not understand.
        let q = Queue {
            enqueued: 1,
            dequeued: 4,
            ..Default::default()
        };
        assert_eq!(q.pending(), None);
    }

    #[test]
    fn the_serialised_shape_keeps_absent_and_explicit_null_apart() {
        // The wire is what the renderer sees, so the wire is what this
        // asserts. `Observed<Option<WorktreeState>>` was the first shape
        // tried and it FAILED here: nested `Option` collapses both
        // absent and explicit-null to a bare JSON `null`, losing the
        // very distinction the field exists to carry. The tagged enum
        // is what keeps the three states three states.
        let absent = serde_json::to_value(Lifecycle::default()).unwrap();
        assert_eq!(absent["worktree"]["state"], "unknown");
        // The two fields that ARE plain nulls, and legitimately so:
        // there is no third state for either.
        assert_eq!(absent["permission_mode"], serde_json::Value::Null);
        assert_eq!(absent["queue"], serde_json::Value::Null);

        let not_in = serde_json::to_value(Lifecycle {
            worktree: Worktree::NotInWorktree,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(not_in["worktree"]["state"], "not_in_worktree");
        assert_ne!(
            absent["worktree"], not_in["worktree"],
            "absent and a measured negative must not serialise alike"
        );

        let in_wt = serde_json::to_value(Lifecycle {
            worktree: Worktree::In(WorktreeState {
                original_branch: Some("main".into()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(in_wt["worktree"]["state"], "in");
        assert_eq!(in_wt["worktree"]["original_branch"], "main");
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_empty_conversation() {
        let tmp = Tmp::new("gone");
        let e = tail(&tmp.path().join("nope.jsonl")).unwrap_err();
        assert!(e.contains("could not open it"), "{e}");
    }

    /// A transcript whose permissions forbid reading is an error, not an
    /// empty conversation.
    ///
    /// `#[cfg(unix)]` because `PermissionsExt` is a Unix API and mode
    /// bits do not govern readability on Windows -- a `0o000` file there
    /// is still readable by its owner, so this test would report a
    /// success as a failure. Four Windows-only failures have already cost
    /// this repository; this gate is deliberate.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_transcript_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = Tmp::new("perm");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"user","message":{"role":"user","content":"hi"}}"#],
        );
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        let got = tail(&p);
        // Restored BEFORE any assertion can panic, so a failure does not
        // leave an unremovable file behind and break `Tmp::drop`.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        // Running as root defeats mode bits entirely, which is a real CI
        // configuration -- so the test asserts the error only when the
        // permission actually bit.
        if let Err(e) = got {
            assert!(e.contains("could not open it"), "{e}");
        }
    }
}
