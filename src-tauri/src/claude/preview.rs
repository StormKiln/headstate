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
/// Every field is [`Observed`], because every one of them can be absent
/// for a reason that is not zero -- the record is outside the window, or
/// the session predates the feature that writes it. `None` is "not
/// observed", never "none happened".
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
    /// A tool call. The NAME, not the arguments: "Read" or "Bash" tells
    /// the reader what the session was doing, and a 40 KB argument blob
    /// does not.
    ToolUse { name: String },
    /// A tool's output, bounded like any other text.
    ToolResult { text: String, truncated: bool },
    /// A block kind this build does not know.
    ///
    /// Reported rather than dropped: Claude Code owns this format, and a
    /// pane that silently omitted a future block type would show a reader
    /// an exchange with a hole in it and no sign that anything was
    /// missing.
    Other { block_type: String },
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
            blocks: blocks_of(message.get("content")),
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
        "tool_use" => Block::ToolUse {
            name: v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("a tool")
                .to_owned(),
            // Deliberately NOT the `input`. See `Block::ToolUse`.
        },
        "tool_result" => {
            // A tool result's content is itself either a string or a
            // block array -- the same split as a message's, one level
            // down -- so it is flattened to its text rather than
            // assumed to be a string.
            let (text, truncated) = clamp(&flatten_text(v.get("content")));
            Block::ToolResult { text, truncated }
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
                r#"{"type":"assistant","timestamp":"2026-09-13T10:00:00Z","message":{"role":"assistant","model":"claude-opus-5","content":[{"type":"text","text":"Looking now."},{"type":"thinking","thinking":"hmm"},{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/x"}}]}}"#,
                r#"{"type":"user","timestamp":"2026-09-13T10:00:01Z","message":{"role":"user","content":[{"type":"tool_result","content":[{"type":"text","text":"file contents"}]}]}}"#,
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
                    name: "Read".into()
                },
            ]
        );
        assert_eq!(
            v.messages[1].blocks,
            vec![Block::ToolResult {
                text: "file contents".into(),
                truncated: false
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
