//! A transcript as stable, render-ready messages (#1475, epic #1473).
//!
//! `preview.rs` answers "what was this session doing" for the pane that
//! exists today. It keys nothing, renders two record types and counts
//! the rest away. That is right for a preview and wrong for a viewer: the
//! desktop terminal renderer, the phone bubbles, live follow, paging and
//! 7.10's sent-message echo all need one model with
//!
//! - an id per message that survives a re-read,
//! - the turn each message belongs to,
//! - every record kind a reader should see, and
//! - per-block truncation that says how much was kept.
//!
//! This module is that model. It is ADDITIVE: `preview.rs` keeps serving
//! the current pane unchanged, and shares its tool-argument and diff
//! parsers with this one ([`super::preview::tool_args`],
//! [`super::preview::file_change`]) so the two readings of a tool call
//! cannot drift.
//!
//! # Record shapes, measured
//!
//! Over 300 real transcripts (96,361 records) on the development
//! machine:
//!
//! ```text
//!  30908 attachment        6415 queue-operation     2999 mode
//!  21664 assistant         4122 atis-latch          2997 permission-mode
//!  12784 user              4115 last-prompt         2466 system
//!                          3923 ai-title             304 file-history-snapshot
//!                          3496 pr-link              110 cost-state
//! ```
//!
//! `system` subtypes: `turn_duration` 3625, `stop_hook_summary` 3605,
//! `away_summary` 1153, `compact_boundary` 45, `informational` 18,
//! `api_error` 10, `local_command` 8, `agents_killed` 1 (3,000 files).
//!
//! `attachment` is mostly machinery (`total_tokens_reminder`,
//! `skill_listing`, `environment`, ...) with two exceptions a reader
//! wants: hook output (`hook_success` 15,631, `hook_cancelled`,
//! `hook_system_message`, `hook_additional_context`) and queued prompts.
//!
//! `user` records are not all "You". Measured over the non-tool-result
//! ones: 2,515 are `<task-notification>` (a background agent reporting
//! back), 17 are `<command-name>` slash commands, 13 are local command
//! output, 18 are `!` shell input, 12 are interruptions, and 65 carry
//! `isMeta` (skill bodies, caveats, auto-continuations). Rendering those
//! as the user speaking is the defect the issue names.
//!
//! # An allowlist that grows, and an explicit remainder
//!
//! Still an allowlist, for `preview.rs`'s reason: Claude Code owns this
//! format and changes it. What changes is what happens to a record that
//! is on no list. `preview.rs` counts it and drops it; here it becomes a
//! [`MessageKind::Unrecognised`] message carrying its type, so a renderer
//! draws a thin "unrecognised record" divider where it sat. An exchange
//! with an invisible hole in it is the #846 defect.
//!
//! Records that ARE recognised as bookkeeping -- the `queue-operation`,
//! `ai-title` and `skill_listing` kind -- are counted by type in
//! [`TranscriptPage::machinery_records`] and not rendered. That list is
//! named in [`MACHINERY_TYPES`] and [`MACHINERY_ATTACHMENTS`]; a record
//! type nobody listed is unrecognised, never machinery by default.
//!
//! Run over the tail windows of 1,133 real transcripts before the lists
//! were final, this produced 10,690 messages of 16 kinds and two
//! unrecognised types -- `agent-setting` and
//! `attachment/read_truncation_notice`, both bookkeeping, both since
//! listed. That is the remainder working as designed: the first sight of
//! a new type was a divider naming it, not a silent gap.
//!
//! # Ids
//!
//! See [`IdSource`]. Every record type that carries conversation has a
//! `uuid` (measured: 100% of `user`, `assistant`, `system` and
//! `attachment` records), so the documented fallback only ever applies to
//! the uuid-less bookkeeping types -- of which `permission-mode` is the
//! one rendered.
//!
//! **Duplicate uuids exist.** 32 in 3,000 files: the same message written
//! twice, differing only in `cwd` / `gitBranch` / `slug` / `promptId`
//! (a resumed session re-recording its head). The FIRST occurrence wins
//! and later ones are counted in [`TranscriptPage::duplicate_records`],
//! so ids stay unique and a React key never collides.
//!
//! # Read-only
//!
//! Like `preview.rs`, this adds no write exception under `~/.claude`.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::preview::{self, FileChange, ToolArgs};

/// The most messages returned from one window.
///
/// Larger than `preview::MAX_MESSAGES` because this model renders hook
/// output, notices and command output as messages where the preview
/// dropped them, so the same 256 KB window holds more messages. Still a
/// bound for the render's sake, and stated when it binds.
pub const MAX_MESSAGES: usize = 400;

/// The most characters returned by a by-id full-text fetch.
///
/// Full text on demand is not "unbounded on demand": a single tool result
/// can be an entire 38 MB log, and the fetch crosses the pairing transport
/// to the phone. 256 K characters is 64 times the per-block cap, which
/// covers every file a reader would scroll, and the response says when it
/// still bound -- exactly as the per-block cap does.
pub const FULL_TEXT_CHARS: usize = 256 * 1024;

/// Top-level record types that are bookkeeping: counted, never rendered.
///
/// Every name was measured on the corpus (module docs). `queue-operation`
/// and `worktree-state` are read by `preview.rs`'s lifecycle pass; here
/// they are bookkeeping, because they carry session state rather than
/// something that happened in the conversation.
pub const MACHINERY_TYPES: [&str; 13] = [
    "queue-operation",
    "agent-setting",
    "atis-latch",
    "last-prompt",
    "ai-title",
    "custom-title",
    "pr-link",
    "mode",
    "file-history-snapshot",
    "file-history-delta",
    "cost-state",
    "worktree-state",
    "relocated",
];

/// `attachment` subtypes that are bookkeeping: counted, never rendered.
///
/// The attachments a reader wants -- hooks and queued prompts -- are
/// matched BEFORE this list is consulted, in [`attachment_message`].
pub const MACHINERY_ATTACHMENTS: [&str; 29] = [
    "total_tokens_reminder",
    "prompt_snapshot",
    "deferred_tools_delta",
    "deferred_tools_record",
    "agent_listing_delta",
    "skill_listing",
    "dynamic_skill",
    "invoked_skills",
    "environment",
    "structured_output",
    "date",
    "date_change",
    "model",
    "remote_session_change",
    "instructions",
    "nested_memory",
    "batching_reminder_sent",
    "session_context",
    "bash_output_audience_note",
    "task_reminder",
    "mcp_instructions_delta",
    "file",
    "directory",
    "edited_text_file",
    "compact_file_reference",
    "auto_mode",
    "silent_turn_reminder",
    "command_permissions",
    "read_truncation_notice",
];

/// Where a message's id came from.
///
/// # The rule
///
/// 1. **`Uuid`** -- the record's own `uuid`. Every conversation record
///    carries one, so this is what the renderers, "unread since" markers
///    and 7.10's echo reconciliation key on. It survives a re-read, a
///    different window, and a compaction (a compaction writes NEW records;
///    it does not renumber old ones).
/// 2. **`Anchored`** -- a record with no `uuid` gets
///    `"<previous uuid>+<n>"`: the nearest preceding uuid-bearing record,
///    and its ordinal among the uuid-less records since. At the start of
///    the file the anchor is `^`. Stable for any read that includes the
///    anchor record, which every append-only follow and every page that
///    starts before it does.
/// 3. **`Unanchored`** -- a uuid-less record read before any uuid-bearing
///    one in a window that starts mid-file. There is no anchor to name,
///    so the id is `"~<hash of the line>"` (with `.2`, `.3` for identical
///    lines in the window). Stable across re-reads of the same bytes;
///    it can change if the window moves, and says so by being this
///    variant.
/// 4. **`Derived`** -- a message this module produced rather than read:
///    a [`MessageKind::ModelChange`] between two assistant messages,
///    keyed `"<assistant id>/model"`.
///
/// A record with neither a uuid nor a stable position is not given a
/// confident-looking id: the variant says which promise the id makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdSource {
    Uuid,
    Anchored,
    Unanchored,
    Derived,
}

/// How much of one block's text was kept.
///
/// Carried per block so a renderer can say "showing the first 4,000 of
/// 38,210 characters" beside the block it clipped, rather than one
/// warning for the whole pane that does not say WHICH block is short.
/// `None` on the block means nothing was clipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptClip {
    pub shown_chars: usize,
    pub total_chars: usize,
}

/// Token usage as the record states it.
///
/// Every field is `Option`: a record that carried no `usage` has no
/// message-level usage at all (the whole struct is `None`), and a usage
/// object missing one counter has THAT counter absent, never zero.
///
/// Claude Code writes one API response as several records -- one per
/// content block -- and repeats the response's `usage` on each. Summing
/// usage across messages therefore over-counts; sum once per
/// [`TranscriptMessage::api_message_id`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

/// An image, as a placeholder. Never its bytes.
///
/// A pasted screenshot is ~570 KB of base64 in one record; shipping it to
/// the phone to draw a thumbnail is the hang the byte caps exist for.
/// Dimensions are only recorded for tool-produced images
/// (`toolUseResult.file.dimensions`); for a pasted one they are `None`,
/// which is "not recorded", not zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptImage {
    pub media_type: Option<String>,
    /// Decoded size estimated from the base64 length, when there was one.
    pub approx_bytes: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

/// A subagent (Task/Agent) call's link to its own transcript.
///
/// The link target is `<session dir>/<session id>/subagents/agent-<id>.jsonl`
/// -- the layout Claude Code writes, with a sibling `.meta.json` naming
/// the `toolUseId` that spawned it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSubagent {
    /// `toolUseResult.agentId`, verbatim.
    pub agent_id: String,
    /// `async_launched`, `completed`, ... verbatim. `None` when unstated.
    pub status: Option<String>,
    pub agent_type: Option<String>,
    /// The subagent transcript's path. Suitable for
    /// `claude_transcript_messages`, which applies the same path guard.
    pub transcript_path: Option<String>,
    /// Whether that file exists. `None` when it was not checked -- the
    /// parse had no transcript path to resolve against -- which is not
    /// the same fact as "it is missing".
    pub transcript_found: Option<bool>,
}

/// A tool's output.
///
/// Appears in two places: inside the [`TranscriptBlock::ToolCall`] it
/// answers once both are loaded (merged by `tool_use_id`, never replaced),
/// or on its own as a [`TranscriptBlock::ToolResult`] when its call is
/// not in the loaded messages -- above the window, or in a page not yet
/// fetched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptToolOutput {
    /// The record this output was READ from. Distinct from the call's
    /// message once merged, and it is the address for a full-text fetch.
    pub message_id: String,
    /// This block's position in that record, for the same fetch.
    pub index: usize,
    pub tool_use_id: Option<String>,
    pub text: String,
    pub clip: Option<TranscriptClip>,
    /// `None` when the record carried no `is_error`, which is not `false`.
    pub is_error: Option<bool>,
    /// The diff the record's `toolUseResult` recorded. See
    /// `preview::FileChange` for where this lives and why.
    pub change: Option<FileChange>,
    /// Images the tool returned, as placeholders.
    pub images: Vec<TranscriptImage>,
    /// Set when this output is a subagent's.
    pub subagent: Option<TranscriptSubagent>,
}

/// One content block.
///
/// `index` is the block's position in the record it was read from, and
/// with the message id it is the address the full-text fetch takes. It
/// is not the position in [`TranscriptMessage::blocks`], which merging
/// can change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptBlock {
    Text {
        index: usize,
        text: String,
        clip: Option<TranscriptClip>,
    },
    /// Reasoning. `recorded: false` when the record kept only the
    /// signature -- the common case on current models -- which renders
    /// as "thinking (not recorded)", never as empty thought.
    Thinking {
        index: usize,
        text: String,
        clip: Option<TranscriptClip>,
        recorded: bool,
    },
    /// A tool call, with its result merged in once both are loaded.
    ToolCall {
        index: usize,
        name: String,
        id: Option<String>,
        args: ToolArgs,
        /// `None` means no result is in the LOADED messages. Whether that
        /// is "still running" or "never came back" is liveness's
        /// question, not this module's -- `preview::Pairing::Unanswered`
        /// argues it.
        result: Option<TranscriptToolOutput>,
    },
    /// A result whose call is not in the loaded messages.
    ToolResult(TranscriptToolOutput),
    Image {
        index: usize,
        image: TranscriptImage,
    },
    /// A block type this build does not know. Reported, not dropped.
    Other { index: usize, block_type: String },
}

/// What a message IS, which is not the same as who wrote the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageKind {
    /// Something the user typed. `origin` is `origin.kind` verbatim
    /// (`human`, `peer`, ...) or `None` when unrecorded. Starts a turn.
    UserPrompt { origin: Option<String> },
    /// A slash command, e.g. `/review`, with its arguments as the first
    /// text block when it had any. Starts a turn.
    SlashCommand { name: String },
    /// A `!` shell command the user ran. Starts a turn.
    ShellInput { command: String },
    /// Output of a local command (`/cost`, `!ls`, ...). `command` is the
    /// command's name when the record says.
    CommandOutput { command: Option<String> },
    /// Model output: text, thinking, tool calls.
    Assistant,
    /// A user record carrying only tool results. Absorbed into the calls
    /// once they are loaded; left standing when they are not.
    ToolResults,
    /// A background agent reporting back (`<task-notification>`). NOT the
    /// user speaking, which is how it rendered before.
    AgentNotification {
        task_id: Option<String>,
        status: Option<String>,
    },
    /// Text the harness inserted on the user's behalf (`isMeta`): skill
    /// bodies, caveats, auto-continuations. NOT the user speaking.
    Injected { origin: Option<String> },
    /// A prompt typed while a turn was running and queued behind it.
    /// `mode` is `commandMode` verbatim.
    QueuedPrompt { mode: Option<String> },
    /// "[Request interrupted by user]".
    Interruption { during_tool_use: bool },
    /// Where a compaction replaced the conversation so far.
    CompactionBoundary {
        trigger: Option<String>,
        pre_tokens: Option<u64>,
        post_tokens: Option<u64>,
    },
    /// The summary a compaction left in the conversation's place.
    CompactionSummary,
    /// A legacy `summary` record: a session summary pointing at the leaf
    /// message it describes.
    Summary { leaf_uuid: Option<String> },
    /// An API error, and whether Claude Code was retrying it.
    ///
    /// From a `system/api_error` record (a retry in progress, with its
    /// attempt counts) or an assistant record with `isApiErrorMessage`
    /// (the error the user saw). Every field is as recorded.
    ApiError {
        status: Option<u64>,
        error_type: Option<String>,
        retry_attempt: Option<u64>,
        max_retries: Option<u64>,
        retry_in_ms: Option<u64>,
    },
    /// A hook ran. `outcome` is the record's own type verbatim
    /// (`hook_success`, `hook_cancelled`, `stop_hook_summary`, ...).
    HookOutput {
        event: Option<String>,
        name: Option<String>,
        outcome: String,
        exit_code: Option<i64>,
        prevented_continuation: Option<bool>,
    },
    /// How long a turn took (`system/turn_duration`); the figure is
    /// [`TranscriptMessage::duration_ms`].
    TurnDuration { message_count: Option<u64> },
    /// A `system` or `attachment` notice the harness showed, with its
    /// subtype verbatim (`informational`, `away_summary`, ...).
    Notice {
        subtype: String,
        level: Option<String>,
    },
    /// The assistant's model differs from the previous assistant
    /// message's. Derived, not read: see [`IdSource::Derived`].
    ModelChange { from: String, to: String },
    /// A `permission-mode` record: the mode, verbatim.
    PermissionModeChange { mode: String },
    /// A record on no list. Rendered as a thin divider naming the type,
    /// never dropped. `record_type` is `type`, or `system/<subtype>` /
    /// `attachment/<subtype>` for those envelopes.
    Unrecognised { record_type: String },
}

/// One message, ready to render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptMessage {
    /// Stable id. See [`IdSource`] for the rule and its fallback.
    pub id: String,
    pub id_source: IdSource,
    /// The id of the message that opened this message's turn -- a
    /// [`MessageKind::UserPrompt`], [`MessageKind::SlashCommand`] or
    /// [`MessageKind::ShellInput`]. Equal to `id` on the opener itself.
    ///
    /// `None` for messages before the first opener in the loaded set:
    /// their turn began above what was read. Not a turn of its own.
    pub turn_id: Option<String>,
    pub kind: MessageKind,
    /// RFC 3339 as recorded, or `None`. Never substituted.
    pub timestamp: Option<String>,
    /// `message.model`, verbatim (including Claude Code's `<synthetic>`).
    pub model: Option<String>,
    /// `message.id`: the API response this record came from. Several
    /// records share one when a response is split per block.
    pub api_message_id: Option<String>,
    pub usage: Option<TranscriptUsage>,
    /// Where the record states a duration: a turn's, a hook's.
    pub duration_ms: Option<u64>,
    pub is_meta: bool,
    pub is_sidechain: bool,
    pub blocks: Vec<TranscriptBlock>,
}

/// A window of a transcript as messages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPage {
    /// Oldest first.
    pub messages: Vec<TranscriptMessage>,
    /// Whether anything before these messages was not read -- the window
    /// started mid-file, or [`MAX_MESSAGES`] dropped older ones.
    pub truncated: bool,
    pub bytes_read: u64,
    pub file_bytes: u64,
    /// Bookkeeping records in the window, by type, commonest first.
    /// Counted so the renderer can say where they went.
    pub machinery_records: Vec<(String, usize)>,
    /// Lines that would not parse as JSON. Not the same as unrecognised.
    pub unparseable_records: usize,
    /// Records whose uuid was already seen in the window; the first
    /// occurrence was kept. See the module docs.
    pub duplicate_records: usize,
}

/// One block's full text, fetched by address (#1475).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptBlockText {
    pub message_id: String,
    pub index: usize,
    pub text: String,
    /// Set when even [`FULL_TEXT_CHARS`] bound: the fetch is bounded too.
    pub clip: Option<TranscriptClip>,
}

// ---------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------

/// Where the parsed text starts in its file, which decides how a
/// uuid-less record at the head of the window is keyed. See [`IdSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStart {
    /// Byte 0: `^` anchors the leading uuid-less records.
    FileStart,
    /// Somewhere later: records before the first uuid are unanchored.
    MidFile,
}

/// What a single record became.
enum Parsed {
    Message(Box<TranscriptMessage>),
    Machinery(String),
}

/// Context a record parse needs beyond the record itself.
struct Ctx<'a> {
    /// The transcript being read, to resolve subagent links against.
    transcript: Option<&'a Path>,
    /// The per-block character cap: [`preview::MAX_TEXT_CHARS`] for a
    /// page, [`FULL_TEXT_CHARS`] for a by-id fetch. ONE parser serves
    /// both, so the full text of block `n` is by construction the text
    /// block `n` was clipped from.
    limit: usize,
}

/// Parse a window of JSONL into messages, then [`settle`] them.
///
/// `transcript` is the file the body came from, used only to resolve
/// subagent links; `None` leaves them unresolved and says so.
pub fn parse(body: &str, start: WindowStart, transcript: Option<&Path>) -> TranscriptPage {
    let ctx = Ctx {
        transcript,
        limit: preview::MAX_TEXT_CHARS,
    };
    let mut page = TranscriptPage::default();
    let mut machinery: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut anchor: Option<String> = match start {
        WindowStart::FileStart => Some("^".to_owned()),
        WindowStart::MidFile => None,
    };
    let mut since_anchor = 0usize;
    let mut unanchored: HashMap<String, usize> = HashMap::new();

    for raw in body.lines() {
        // `lines()` already strips a trailing `\r`, so a CRLF file parses
        // the same as an LF one.
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            page.unparseable_records += 1;
            continue;
        };
        let uuid = str_of(&rec, "uuid");
        if let Some(u) = &uuid {
            if !seen.insert(u.clone()) {
                page.duplicate_records += 1;
                continue;
            }
            anchor = Some(u.clone());
            since_anchor = 0;
        }
        let (id, id_source) = match (&uuid, &anchor) {
            (Some(u), _) => (u.clone(), IdSource::Uuid),
            (None, Some(a)) => {
                since_anchor += 1;
                (format!("{a}+{since_anchor}"), IdSource::Anchored)
            }
            (None, None) => {
                let h = short_hash(line);
                let n = unanchored.entry(h.clone()).or_default();
                *n += 1;
                let id = if *n == 1 {
                    format!("~{h}")
                } else {
                    format!("~{h}.{n}")
                };
                (id, IdSource::Unanchored)
            }
        };
        match record(&rec, id, id_source, &ctx) {
            Parsed::Message(m) => page.messages.push(*m),
            Parsed::Machinery(t) => *machinery.entry(t).or_default() += 1,
        }
    }

    let mut counts: Vec<(String, usize)> = machinery.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    page.machinery_records = counts;

    settle(&mut page.messages);
    if page.messages.len() > MAX_MESSAGES {
        page.messages.drain(..page.messages.len() - MAX_MESSAGES);
        page.truncated = true;
        // The drain can cut a turn's opener off; re-derive so no message
        // points at a turn id that is no longer in the page.
        settle(&mut page.messages);
    }
    page
}

/// Pair tool results into their calls, derive model changes and assign
/// turns -- over whatever set of messages is loaded.
///
/// Idempotent, and that is the property paging rests on: two pages
/// parsed separately, concatenated and settled pair a call in the first
/// with its result in the second exactly as one read of both would. A
/// result merged into its call is MOVED there (the call's `result` slot),
/// never overwritten: a second, different result naming an already
/// answered call stays standing on its own.
pub fn settle(messages: &mut Vec<TranscriptMessage>) {
    pair(messages);
    model_changes(messages);
    turns(messages);
}

fn pair(messages: &mut Vec<TranscriptMessage>) {
    // Where each call is: (message, block).
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();
    for (mi, m) in messages.iter().enumerate() {
        for (bi, b) in m.blocks.iter().enumerate() {
            if let TranscriptBlock::ToolCall { id: Some(id), .. } = b {
                calls.entry(id.clone()).or_insert((mi, bi));
            }
        }
    }
    // Which standing results can move, decided before anything moves.
    let mut moves: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut claimed: HashSet<(usize, usize)> = HashSet::new();
    for (mi, m) in messages.iter().enumerate() {
        for (bi, b) in m.blocks.iter().enumerate() {
            let TranscriptBlock::ToolResult(out) = b else {
                continue;
            };
            let Some(tid) = &out.tool_use_id else {
                continue;
            };
            let Some(&(cm, cb)) = calls.get(tid) else {
                continue;
            };
            let filled = matches!(
                &messages[cm].blocks[cb],
                TranscriptBlock::ToolCall {
                    result: Some(_),
                    ..
                }
            );
            if filled || !claimed.insert((cm, cb)) {
                continue;
            }
            moves.push((mi, bi, cm, cb));
        }
    }
    // Take the outputs out, highest block index first per message so the
    // indices still to be taken stay valid.
    moves.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut taken: Vec<(usize, usize, TranscriptToolOutput)> = Vec::new();
    for (mi, bi, cm, cb) in moves {
        if let TranscriptBlock::ToolResult(out) = messages[mi].blocks.remove(bi) {
            taken.push((cm, cb, out));
        }
    }
    for (cm, cb, out) in taken {
        if let TranscriptBlock::ToolCall { result, .. } = &mut messages[cm].blocks[cb] {
            *result = Some(out);
        }
    }
    // A tool-results record whose every result was absorbed has nothing
    // left to show. Any OTHER message left empty stays: an empty
    // assistant message is a fact about the record.
    messages.retain(|m| !(m.kind == MessageKind::ToolResults && m.blocks.is_empty()));
}

fn model_changes(messages: &mut Vec<TranscriptMessage>) {
    messages.retain(|m| !matches!(m.kind, MessageKind::ModelChange { .. }));
    let mut out = Vec::with_capacity(messages.len());
    let mut previous: Option<String> = None;
    for m in messages.drain(..) {
        let real = m.kind == MessageKind::Assistant
            && !m.is_sidechain
            && m.model.as_deref().is_some_and(|x| x != "<synthetic>");
        if real {
            let now = m.model.clone().unwrap_or_default();
            if let Some(prev) = previous.take() {
                if prev != now {
                    out.push(TranscriptMessage {
                        id: format!("{}/model", m.id),
                        id_source: IdSource::Derived,
                        turn_id: None,
                        kind: MessageKind::ModelChange {
                            from: prev,
                            to: now.clone(),
                        },
                        timestamp: m.timestamp.clone(),
                        model: None,
                        api_message_id: None,
                        usage: None,
                        duration_ms: None,
                        is_meta: false,
                        is_sidechain: false,
                        blocks: Vec::new(),
                    });
                }
            }
            previous = Some(now);
        }
        out.push(m);
    }
    *messages = out;
}

fn turns(messages: &mut [TranscriptMessage]) {
    let mut current: Option<String> = None;
    for m in messages.iter_mut() {
        let opens = !m.is_sidechain
            && matches!(
                m.kind,
                MessageKind::UserPrompt { .. }
                    | MessageKind::SlashCommand { .. }
                    | MessageKind::ShellInput { .. }
            );
        if opens {
            current = Some(m.id.clone());
        }
        m.turn_id = current.clone();
    }
}

/// One record, as a message or as bookkeeping.
fn record(rec: &serde_json::Value, id: String, id_source: IdSource, ctx: &Ctx) -> Parsed {
    let kind = rec.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let base = |kind: MessageKind, blocks: Vec<TranscriptBlock>| {
        Parsed::Message(Box::new(TranscriptMessage {
            id: id.clone(),
            id_source,
            turn_id: None,
            kind,
            timestamp: str_of(rec, "timestamp"),
            model: None,
            api_message_id: None,
            usage: None,
            duration_ms: None,
            is_meta: rec.get("isMeta").and_then(|v| v.as_bool()) == Some(true),
            is_sidechain: rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true),
            blocks,
        }))
    };
    match kind {
        "user" => user_message(rec, base, ctx, &id),
        "assistant" => {
            let message = rec.get("message");
            let blocks = content_blocks(message.and_then(|m| m.get("content")), ctx, &id);
            let k = if rec.get("isApiErrorMessage").and_then(|v| v.as_bool()) == Some(true) {
                MessageKind::ApiError {
                    status: u64_of(rec, "apiErrorStatus"),
                    error_type: str_of(rec, "error"),
                    retry_attempt: None,
                    max_retries: None,
                    retry_in_ms: None,
                }
            } else {
                MessageKind::Assistant
            };
            let Parsed::Message(mut m) = base(k, blocks) else {
                unreachable!("base always builds a message")
            };
            if let Some(msg) = message {
                m.model = str_of(msg, "model");
                m.api_message_id = str_of(msg, "id");
                m.usage = msg.get("usage").and_then(usage_of);
            }
            Parsed::Message(m)
        }
        "system" => system_message(rec, base, ctx),
        "attachment" => attachment_message(rec, base, ctx),
        "permission-mode" => match str_of(rec, "permissionMode") {
            Some(mode) => base(MessageKind::PermissionModeChange { mode }, Vec::new()),
            // A mode record with no mode is not a mode change to "".
            None => base(
                MessageKind::Unrecognised {
                    record_type: "permission-mode".to_owned(),
                },
                Vec::new(),
            ),
        },
        "summary" => {
            let blocks = text_blocks([str_of(rec, "summary")], ctx);
            base(
                MessageKind::Summary {
                    leaf_uuid: str_of(rec, "leafUuid"),
                },
                blocks,
            )
        }
        t if MACHINERY_TYPES.contains(&t) => Parsed::Machinery(t.to_owned()),
        t => base(
            MessageKind::Unrecognised {
                record_type: if t.is_empty() {
                    "(no type)".to_owned()
                } else {
                    t.to_owned()
                },
            },
            Vec::new(),
        ),
    }
}

fn user_message(
    rec: &serde_json::Value,
    base: impl Fn(MessageKind, Vec<TranscriptBlock>) -> Parsed,
    ctx: &Ctx,
    id: &str,
) -> Parsed {
    let content = rec.get("message").and_then(|m| m.get("content"));
    let origin = rec
        .get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
        .filter(|k| !k.is_empty())
        .map(str::to_owned);

    if rec.get("isCompactSummary").and_then(|v| v.as_bool()) == Some(true) {
        return base(
            MessageKind::CompactionSummary,
            content_blocks(content, ctx, id),
        );
    }

    let mut blocks = content_blocks(content, ctx, id);
    let only_results = !blocks.is_empty()
        && blocks
            .iter()
            .all(|b| matches!(b, TranscriptBlock::ToolResult(_)));
    if only_results {
        attach_record_result(rec, &mut blocks, ctx);
        return base(MessageKind::ToolResults, blocks);
    }

    // The text the record leads with decides what it is. The tags are
    // Claude Code's own envelope for non-prompt user records.
    let lead: String = match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_owned(),
        _ => String::new(),
    };
    let lead_trim = lead.trim_start();

    if lead_trim.starts_with("[Request interrupted by user") {
        return base(
            MessageKind::Interruption {
                during_tool_use: lead_trim.contains("for tool use"),
            },
            Vec::new(),
        );
    }
    if let Some(name) = tag(&lead, "command-name") {
        let args = tag(&lead, "command-args").filter(|a| !a.trim().is_empty());
        return base(MessageKind::SlashCommand { name }, text_blocks([args], ctx));
    }
    if let Some(command) = tag(&lead, "bash-input") {
        return base(MessageKind::ShellInput { command }, Vec::new());
    }
    let outputs = [
        tag(&lead, "local-command-stdout"),
        tag(&lead, "local-command-stderr"),
        tag(&lead, "bash-stdout"),
        tag(&lead, "bash-stderr"),
    ];
    if outputs.iter().any(Option::is_some) {
        let texts = outputs.into_iter().map(|o| o.filter(|s| !s.is_empty()));
        return base(
            MessageKind::CommandOutput { command: None },
            text_blocks(texts, ctx),
        );
    }
    if lead_trim.starts_with("<task-notification>")
        || origin.as_deref() == Some("task-notification")
    {
        return base(
            MessageKind::AgentNotification {
                task_id: tag(&lead, "task-id"),
                status: tag(&lead, "status"),
            },
            blocks,
        );
    }
    if rec.get("isMeta").and_then(|v| v.as_bool()) == Some(true) {
        return base(MessageKind::Injected { origin }, blocks);
    }
    base(MessageKind::UserPrompt { origin }, blocks)
}

fn system_message(
    rec: &serde_json::Value,
    base: impl Fn(MessageKind, Vec<TranscriptBlock>) -> Parsed,
    ctx: &Ctx,
) -> Parsed {
    let subtype = str_of(rec, "subtype").unwrap_or_default();
    let content = text_blocks([str_of(rec, "content")], ctx);
    let with_duration = |p: Parsed, ms: Option<u64>| match p {
        Parsed::Message(mut m) => {
            m.duration_ms = ms;
            Parsed::Message(m)
        }
        other => other,
    };
    match subtype.as_str() {
        "compact_boundary" => {
            let meta = rec.get("compactMetadata");
            base(
                MessageKind::CompactionBoundary {
                    trigger: meta.and_then(|m| str_of(m, "trigger")),
                    pre_tokens: meta.and_then(|m| u64_of(m, "preTokens")),
                    post_tokens: meta.and_then(|m| u64_of(m, "postTokens")),
                },
                content,
            )
        }
        "api_error" => {
            let err = rec.get("error");
            let formatted = err.and_then(|e| str_of(e, "formatted"));
            base(
                MessageKind::ApiError {
                    status: err.and_then(|e| u64_of(e, "status")),
                    error_type: None,
                    retry_attempt: u64_of(rec, "retryAttempt"),
                    max_retries: u64_of(rec, "maxRetries"),
                    retry_in_ms: u64_of(rec, "retryInMs"),
                },
                text_blocks([formatted], ctx),
            )
        }
        "local_command" => base(
            MessageKind::CommandOutput {
                command: rec.get("commandRun").and_then(|c| str_of(c, "command")),
            },
            content,
        ),
        "stop_hook_summary" => {
            let errors: Vec<Option<String>> = rec
                .get("hookErrors")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().map(|e| e.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            // The stop reason's index follows EVERY error slot, empty or
            // not, so no two blocks can share an address.
            let after = errors.len();
            let mut blocks = text_blocks(errors, ctx);
            blocks.extend(text_blocks_from(after, [str_of(rec, "stopReason")], ctx));
            base(
                MessageKind::HookOutput {
                    event: Some("Stop".to_owned()),
                    name: None,
                    outcome: subtype.clone(),
                    exit_code: None,
                    prevented_continuation: rec
                        .get("preventedContinuation")
                        .and_then(|v| v.as_bool()),
                },
                blocks,
            )
        }
        "turn_duration" => with_duration(
            base(
                MessageKind::TurnDuration {
                    message_count: u64_of(rec, "messageCount"),
                },
                Vec::new(),
            ),
            u64_of(rec, "durationMs"),
        ),
        // A notice with no text of its own and a subtype nobody listed is
        // not a notice anyone can read: it is an unrecognised record.
        s if !content.is_empty() || s == "informational" || s == "away_summary" => base(
            MessageKind::Notice {
                subtype: s.to_owned(),
                level: str_of(rec, "level"),
            },
            content,
        ),
        s => base(
            MessageKind::Unrecognised {
                record_type: format!("system/{s}"),
            },
            Vec::new(),
        ),
    }
}

fn attachment_message(
    rec: &serde_json::Value,
    base: impl Fn(MessageKind, Vec<TranscriptBlock>) -> Parsed,
    ctx: &Ctx,
) -> Parsed {
    let Some(att) = rec.get("attachment") else {
        return base(
            MessageKind::Unrecognised {
                record_type: "attachment".to_owned(),
            },
            Vec::new(),
        );
    };
    let t = str_of(att, "type").unwrap_or_default();
    if t.starts_with("hook_") {
        // `content` is a string on most hook attachments and a list of
        // strings on `hook_additional_context`.
        let content: Vec<Option<String>> = match att.get("content") {
            Some(serde_json::Value::Array(a)) => {
                a.iter().map(|v| v.as_str().map(str::to_owned)).collect()
            }
            Some(serde_json::Value::String(s)) => vec![Some(s.clone())],
            _ => Vec::new(),
        };
        let texts = [str_of(att, "stdout"), str_of(att, "stderr")]
            .into_iter()
            .chain(content);
        let p = base(
            MessageKind::HookOutput {
                event: str_of(att, "hookEvent"),
                name: str_of(att, "hookName"),
                outcome: t.clone(),
                exit_code: att.get("exitCode").and_then(|v| v.as_i64()),
                prevented_continuation: None,
            },
            text_blocks(texts, ctx),
        );
        return match p {
            Parsed::Message(mut m) => {
                m.duration_ms = u64_of(att, "durationMs");
                Parsed::Message(m)
            }
            other => other,
        };
    }
    match t.as_str() {
        "queued_command" => base(
            MessageKind::QueuedPrompt {
                mode: str_of(att, "commandMode"),
            },
            text_blocks([str_of(att, "prompt")], ctx),
        ),
        // Two attachments a reader wants and nothing else renders.
        "max_turns_reached" | "task_status" => base(
            MessageKind::Notice {
                subtype: t.clone(),
                level: None,
            },
            text_blocks([str_of(att, "description")], ctx),
        ),
        s if MACHINERY_ATTACHMENTS.contains(&s) => Parsed::Machinery(format!("attachment/{s}")),
        s => base(
            MessageKind::Unrecognised {
                record_type: format!("attachment/{s}"),
            },
            Vec::new(),
        ),
    }
}

/// The blocks of a `message.content`: a bare string or a block array.
fn content_blocks(
    content: Option<&serde_json::Value>,
    ctx: &Ctx,
    message_id: &str,
) -> Vec<TranscriptBlock> {
    match content {
        Some(serde_json::Value::String(s)) => text_blocks([Some(s.clone())], ctx),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, b)| block(i, b, ctx, message_id))
            .collect(),
        _ => Vec::new(),
    }
}

fn block(index: usize, v: &serde_json::Value, ctx: &Ctx, message_id: &str) -> TranscriptBlock {
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let field = |k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or("");
    match kind {
        "text" => {
            let (text, clip) = clip(field("text"), ctx.limit);
            TranscriptBlock::Text { index, text, clip }
        }
        "thinking" => {
            let (text, clip) = clip(field("thinking"), ctx.limit);
            TranscriptBlock::Thinking {
                index,
                recorded: !text.is_empty(),
                text,
                clip,
            }
        }
        "redacted_thinking" => TranscriptBlock::Thinking {
            index,
            text: String::new(),
            clip: None,
            recorded: false,
        },
        "tool_use" => {
            let name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("a tool")
                .to_owned();
            TranscriptBlock::ToolCall {
                index,
                args: preview::tool_args(&name, v.get("input")),
                name,
                id: str_of(v, "id"),
                result: None,
            }
        }
        "tool_result" => {
            let mut texts = Vec::new();
            let mut images = Vec::new();
            match v.get("content") {
                Some(serde_json::Value::String(s)) => texts.push(s.as_str()),
                Some(serde_json::Value::Array(items)) => {
                    for i in items {
                        match i.get("type").and_then(|t| t.as_str()) {
                            Some("image") => images.push(image_of(i)),
                            _ => {
                                if let Some(t) = i.get("text").and_then(|t| t.as_str()) {
                                    texts.push(t);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            let (text, clip) = clip(&texts.join("\n"), ctx.limit);
            TranscriptBlock::ToolResult(TranscriptToolOutput {
                message_id: message_id.to_owned(),
                index,
                tool_use_id: str_of(v, "tool_use_id"),
                text,
                clip,
                is_error: v.get("is_error").and_then(|e| e.as_bool()),
                change: None,
                images,
                subagent: None,
            })
        }
        "image" => TranscriptBlock::Image {
            index,
            image: image_of(v),
        },
        other => TranscriptBlock::Other {
            index,
            block_type: if other.is_empty() {
                "unknown".to_owned()
            } else {
                other.to_owned()
            },
        },
    }
}

/// Attach what the RECORD's `toolUseResult` says to the first result
/// block: the diff, the subagent, image dimensions.
///
/// First only, for `preview.rs`'s reason: a record carries one
/// `toolUseResult`, and attaching it to every result in a batched record
/// would claim one diff came back from several calls.
fn attach_record_result(rec: &serde_json::Value, blocks: &mut [TranscriptBlock], ctx: &Ctx) {
    let Some(TranscriptBlock::ToolResult(out)) = blocks
        .iter_mut()
        .find(|b| matches!(b, TranscriptBlock::ToolResult(_)))
    else {
        return;
    };
    out.change = preview::file_change(rec);
    let Some(tur) = rec.get("toolUseResult") else {
        return;
    };
    if let Some(agent_id) = str_of(tur, "agentId") {
        let path = ctx
            .transcript
            .and_then(|t| subagent_transcript(t, &agent_id));
        out.subagent = Some(TranscriptSubagent {
            transcript_found: path.as_ref().map(|p| p.is_file()),
            transcript_path: path.map(|p| p.display().to_string()),
            agent_id,
            status: str_of(tur, "status"),
            agent_type: str_of(tur, "agentType"),
        });
    }
    if let Some(dims) = tur.get("file").and_then(|f| f.get("dimensions")) {
        if let Some(img) = out.images.first_mut() {
            img.width = u64_of(dims, "originalWidth");
            img.height = u64_of(dims, "originalHeight");
        }
    }
}

/// Where a subagent's transcript lives, relative to its parent's.
///
/// `<dir>/<session>.jsonl` spawns `<dir>/<session>/subagents/agent-<id>.jsonl`.
/// `None` for an agent id that is not a plain token -- it is joined into
/// a path, and an id carrying a separator must not walk out of the
/// directory.
pub fn subagent_transcript(parent: &Path, agent_id: &str) -> Option<PathBuf> {
    if agent_id.is_empty()
        || !agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let dir = parent.parent()?;
    let stem = parent.file_stem()?;
    Some(
        dir.join(stem)
            .join("subagents")
            .join(format!("agent-{agent_id}.jsonl")),
    )
}

fn image_of(v: &serde_json::Value) -> TranscriptImage {
    let source = v.get("source");
    TranscriptImage {
        media_type: source.and_then(|s| str_of(s, "media_type")),
        approx_bytes: source
            .and_then(|s| s.get("data"))
            .and_then(|d| d.as_str())
            .map(|d| (d.len() as u64) * 3 / 4),
        width: None,
        height: None,
    }
}

fn usage_of(u: &serde_json::Value) -> Option<TranscriptUsage> {
    if !u.is_object() {
        return None;
    }
    Some(TranscriptUsage {
        input_tokens: u64_of(u, "input_tokens"),
        output_tokens: u64_of(u, "output_tokens"),
        cache_creation_input_tokens: u64_of(u, "cache_creation_input_tokens"),
        cache_read_input_tokens: u64_of(u, "cache_read_input_tokens"),
    })
}

/// Text blocks from a sequence of optional strings, indexed by position
/// in that sequence. An absent or empty string makes no block but still
/// takes its index, so the full-text fetch addresses the same block.
fn text_blocks(texts: impl IntoIterator<Item = Option<String>>, ctx: &Ctx) -> Vec<TranscriptBlock> {
    text_blocks_from(0, texts, ctx)
}

fn text_blocks_from(
    first: usize,
    texts: impl IntoIterator<Item = Option<String>>,
    ctx: &Ctx,
) -> Vec<TranscriptBlock> {
    texts
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let t = t.filter(|t| !t.is_empty())?;
            let (text, clip) = clip(&t, ctx.limit);
            Some(TranscriptBlock::Text {
                index: first + i,
                text,
                clip,
            })
        })
        .collect()
}

/// Bound one block's text by characters, on a char boundary, reporting
/// how much was kept against how much there was.
fn clip(s: &str, limit: usize) -> (String, Option<TranscriptClip>) {
    match s.char_indices().nth(limit) {
        None => (s.to_owned(), None),
        Some((byte, _)) => (
            s[..byte].to_owned(),
            Some(TranscriptClip {
                shown_chars: limit,
                total_chars: s.chars().count(),
            }),
        ),
    }
}

/// The text between `<name>` and `</name>`, trimmed, when both are there.
fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let from = s.find(&open)? + open.len();
    let to = s[from..].find(&close)? + from;
    Some(s[from..to].trim().to_owned())
}

fn str_of(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn u64_of(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key).and_then(serde_json::Value::as_u64)
}

fn short_hash(line: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(line.as_bytes())
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

// ---------------------------------------------------------------------
// Reading files
// ---------------------------------------------------------------------

/// The tail of `path` as messages, bounded like `preview::tail`: a
/// [`preview::TAIL_BYTES`] window, at most [`MAX_MESSAGES`], each block
/// clipped with its clip stated.
///
/// # Errors
///
/// Only when the file cannot be opened, sized, sought or read. A window
/// with no messages is an answer, and its counts say why.
pub fn tail(path: &Path) -> Result<TranscriptPage, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let file_bytes = file
        .metadata()
        .map_err(|e| format!("{}: could not read its size: {e}", path.display()))?
        .len();
    let start = file_bytes.saturating_sub(preview::TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("{}: could not seek in it: {e}", path.display()))?;
    let limit = file_bytes - start;
    let mut buf = Vec::with_capacity(limit as usize);
    let mut bounded = std::io::Read::take(&mut file, limit);
    bounded
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}: could not read it: {e}", path.display()))?;
    let bytes_read = limit - bounded.limit();

    let text = String::from_utf8_lossy(&buf);
    let mut body: &str = &text;
    let window = if start > 0 {
        // Landed mid-record: drop to the first newline, `preview::tail`'s
        // rule and for its reason.
        body = match body.find('\n') {
            Some(nl) => &body[nl + 1..],
            None => "",
        };
        WindowStart::MidFile
    } else {
        WindowStart::FileStart
    };
    let mut page = parse(body, window, Some(path));
    page.bytes_read = bytes_read;
    page.file_bytes = file_bytes;
    page.truncated |= start > 0;
    Ok(page)
}

/// One block's full text, found by the record's uuid (#1475).
///
/// # The design
///
/// - **Addressed by `(message_id, index)`**, the pair every text-bearing
///   block and every tool output carries. Not by position in a page:
///   pages move, record ids do not.
/// - **Uuid ids only.** A fallback id ([`IdSource`]) names a position,
///   not a record, and the records that get one (`permission-mode`)
///   carry no clipped text anyway. Asking for one is refused by name.
/// - **The same parser** re-reads the record with [`FULL_TEXT_CHARS`] in
///   place of the per-block cap, so block `index` of the fetch is by
///   construction the block that was clipped.
/// - **Bounded server-side**: the response is at most [`FULL_TEXT_CHARS`]
///   characters and says when that bound bit. The scan reads the file
///   line by line, parsing only lines that contain the id; the FIRST
///   record with that uuid wins, matching [`parse`]'s duplicate rule.
///
/// # Errors
///
/// The file cannot be read, the id is not a uuid-shaped id, no record
/// carries it, or the record has no text at that index -- each named, so
/// the UI can say which.
pub fn block_text(
    path: &Path,
    message_id: &str,
    index: usize,
) -> Result<TranscriptBlockText, String> {
    if message_id.is_empty()
        || message_id.len() > 64
        || !message_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "{message_id:?} is not a record id; only messages keyed by their record's uuid have fetchable text"
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let ctx = Ctx {
        transcript: Some(path),
        limit: FULL_TEXT_CHARS,
    };
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("{}: could not read it: {e}", path.display()))?;
        if n == 0 {
            return Err(format!("no record in this transcript has id {message_id}"));
        }
        if !line.contains(message_id) {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if str_of(&rec, "uuid").as_deref() != Some(message_id) {
            continue;
        }
        let Parsed::Message(m) = record(&rec, message_id.to_owned(), IdSource::Uuid, &ctx) else {
            return Err(format!("record {message_id} carries no text"));
        };
        return text_at(&m.blocks, index)
            .map(|(text, clip)| TranscriptBlockText {
                message_id: message_id.to_owned(),
                index,
                text,
                clip,
            })
            .ok_or_else(|| format!("record {message_id} has no text at block {index}"));
    }
}

fn text_at(blocks: &[TranscriptBlock], index: usize) -> Option<(String, Option<TranscriptClip>)> {
    blocks.iter().find_map(|b| match b {
        TranscriptBlock::Text {
            index: i,
            text,
            clip,
        }
        | TranscriptBlock::Thinking {
            index: i,
            text,
            clip,
            ..
        } if *i == index => Some((text.clone(), *clip)),
        TranscriptBlock::ToolResult(o) if o.index == index => Some((o.text.clone(), o.clip)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a JSONL body from records.
    fn body(records: &[serde_json::Value]) -> String {
        records
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn one(rec: serde_json::Value) -> TranscriptMessage {
        let page = parse(&body(&[rec]), WindowStart::FileStart, None);
        assert_eq!(page.messages.len(), 1, "{page:#?}");
        page.messages.into_iter().next().unwrap()
    }

    fn user(uuid: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-01-01T00:00:00Z",
            "isSidechain": false, "message": {"role": "user", "content": content}
        })
    }

    fn assistant(uuid: &str, model: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "type": "assistant", "uuid": uuid, "timestamp": "2026-01-01T00:00:01Z",
            "isSidechain": false,
            "message": {"id": "msg_1", "model": model, "role": "assistant", "content": content,
                "usage": {"input_tokens": 3, "output_tokens": 5, "cache_read_input_tokens": 7}}
        })
    }

    fn call(uuid: &str, tool_id: &str) -> serde_json::Value {
        assistant(
            uuid,
            "model-a",
            serde_json::json!([{"type": "tool_use", "id": tool_id, "name": "Bash",
                "input": {"command": "ls", "description": "list"}}]),
        )
    }

    fn result(uuid: &str, tool_id: &str) -> serde_json::Value {
        user(
            uuid,
            serde_json::json!([{"type": "tool_result", "tool_use_id": tool_id, "content": "out"}]),
        )
    }

    // ---- one fixture per record kind ---------------------------------

    /// A typed prompt is a user prompt and opens its own turn.
    #[test]
    fn a_typed_prompt_is_a_user_prompt_that_opens_a_turn() {
        let mut r = user("u1", serde_json::json!("hello there"));
        r["origin"] = serde_json::json!({"kind": "human"});
        let m = one(r);
        assert_eq!(
            m.kind,
            MessageKind::UserPrompt {
                origin: Some("human".into())
            }
        );
        assert_eq!(m.id, "u1");
        assert_eq!(m.id_source, IdSource::Uuid);
        assert_eq!(m.turn_id.as_deref(), Some("u1"));
        assert_eq!(m.timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    /// Assistant text and thinking, with model, response id and usage.
    #[test]
    fn assistant_text_and_thinking_carry_model_and_usage() {
        let m = one(assistant(
            "a1",
            "model-a",
            serde_json::json!([
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "thinking", "thinking": "considering"},
                {"type": "text", "text": "done"}
            ]),
        ));
        assert_eq!(m.kind, MessageKind::Assistant);
        assert_eq!(m.model.as_deref(), Some("model-a"));
        assert_eq!(m.api_message_id.as_deref(), Some("msg_1"));
        let u = m.usage.unwrap();
        assert_eq!(u.input_tokens, Some(3));
        assert_eq!(u.output_tokens, Some(5));
        assert_eq!(u.cache_read_input_tokens, Some(7));
        // Absent is not zero: the record carried no cache-creation count.
        assert_eq!(u.cache_creation_input_tokens, None);
        assert!(matches!(
            &m.blocks[0],
            TranscriptBlock::Thinking {
                recorded: false,
                ..
            }
        ));
        assert!(matches!(
            &m.blocks[1],
            TranscriptBlock::Thinking {
                recorded: true,
                index: 1,
                ..
            }
        ));
        assert!(matches!(
            &m.blocks[2],
            TranscriptBlock::Text { index: 2, .. }
        ));
    }

    /// A call and its result merge into one block; the result record is
    /// absorbed and its address is kept on the output.
    #[test]
    fn a_tool_call_and_its_result_merge_by_id() {
        let page = parse(
            &body(&[call("a1", "t1"), result("r1", "t1")]),
            WindowStart::FileStart,
            None,
        );
        assert_eq!(page.messages.len(), 1, "{:#?}", page.messages);
        let TranscriptBlock::ToolCall {
            result: Some(out),
            args,
            ..
        } = &page.messages[0].blocks[0]
        else {
            panic!("{:#?}", page.messages[0].blocks)
        };
        assert_eq!(out.text, "out");
        assert_eq!(out.message_id, "r1");
        assert!(matches!(args, ToolArgs::Bash { .. }));
    }

    /// A result whose call is not loaded stands on its own.
    #[test]
    fn a_result_without_its_call_stands_alone() {
        let page = parse(&body(&[result("r1", "t9")]), WindowStart::FileStart, None);
        assert_eq!(page.messages[0].kind, MessageKind::ToolResults);
        assert!(matches!(
            &page.messages[0].blocks[0],
            TranscriptBlock::ToolResult(_)
        ));
    }

    /// A compaction boundary and the summary that follows it.
    #[test]
    fn compaction_boundary_and_summary() {
        let b = one(serde_json::json!({
            "type": "system", "subtype": "compact_boundary", "uuid": "c1",
            "content": "Conversation compacted", "level": "info",
            "compactMetadata": {"trigger": "auto", "preTokens": 900, "postTokens": 100}
        }));
        assert_eq!(
            b.kind,
            MessageKind::CompactionBoundary {
                trigger: Some("auto".into()),
                pre_tokens: Some(900),
                post_tokens: Some(100)
            }
        );
        let mut s = user(
            "c2",
            serde_json::json!("This session is being continued..."),
        );
        s["isCompactSummary"] = serde_json::json!(true);
        assert_eq!(one(s).kind, MessageKind::CompactionSummary);
    }

    /// A slash command carries its name, and its arguments as text.
    #[test]
    fn a_slash_command_with_arguments() {
        let m = one(user(
            "s1",
            serde_json::json!(
                "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>42</command-args>"
            ),
        ));
        assert_eq!(
            m.kind,
            MessageKind::SlashCommand {
                name: "/review".into()
            }
        );
        assert!(matches!(&m.blocks[..], [TranscriptBlock::Text { text, .. }] if text == "42"));
        assert_eq!(m.turn_id.as_deref(), Some("s1"));
    }

    /// Local command output, from both of its record shapes.
    #[test]
    fn command_output_from_user_and_system_records() {
        let m = one(user(
            "o1",
            serde_json::json!("<local-command-stdout>Total cost: 1</local-command-stdout>"),
        ));
        assert_eq!(m.kind, MessageKind::CommandOutput { command: None });
        let s = one(serde_json::json!({
            "type": "system", "subtype": "local_command", "uuid": "o2",
            "content": "Reloaded", "commandRun": {"command": "reload", "args": ""}
        }));
        assert_eq!(
            s.kind,
            MessageKind::CommandOutput {
                command: Some("reload".into())
            }
        );
    }

    /// `!` shell input opens a turn; its output is command output.
    #[test]
    fn shell_input() {
        let m = one(user("b1", serde_json::json!("<bash-input>ls</bash-input>")));
        assert_eq!(
            m.kind,
            MessageKind::ShellInput {
                command: "ls".into()
            }
        );
    }

    /// Hook output from an attachment and from a stop summary.
    #[test]
    fn hook_output() {
        let m = one(serde_json::json!({
            "type": "attachment", "uuid": "h1",
            "attachment": {"type": "hook_success", "hookName": "PostToolUse:Bash",
                "hookEvent": "PostToolUse", "stdout": "ok", "stderr": "", "exitCode": 0,
                "durationMs": 12, "content": ""}
        }));
        assert_eq!(
            m.kind,
            MessageKind::HookOutput {
                event: Some("PostToolUse".into()),
                name: Some("PostToolUse:Bash".into()),
                outcome: "hook_success".into(),
                exit_code: Some(0),
                prevented_continuation: None,
            }
        );
        assert_eq!(m.duration_ms, Some(12));
        assert_eq!(m.blocks.len(), 1);
        let s = one(serde_json::json!({
            "type": "system", "subtype": "stop_hook_summary", "uuid": "h2",
            "hookCount": 1, "hookErrors": [], "preventedContinuation": false, "stopReason": ""
        }));
        assert!(matches!(
            s.kind,
            MessageKind::HookOutput {
                prevented_continuation: Some(false),
                ..
            }
        ));
    }

    /// An interruption is not the user speaking.
    #[test]
    fn interruption() {
        let m = one(user(
            "i1",
            serde_json::json!([{"type": "text", "text": "[Request interrupted by user for tool use]"}]),
        ));
        assert_eq!(
            m.kind,
            MessageKind::Interruption {
                during_tool_use: true
            }
        );
        assert_eq!(m.turn_id, None);
    }

    /// An API error, both as a retry and as the error the user saw.
    #[test]
    fn api_error_and_retry() {
        let r = one(serde_json::json!({
            "type": "system", "subtype": "api_error", "uuid": "e1", "level": "error",
            "error": {"status": 529, "formatted": "529 Overloaded"},
            "retryInMs": 500, "retryAttempt": 2, "maxRetries": 10
        }));
        assert_eq!(
            r.kind,
            MessageKind::ApiError {
                status: Some(529),
                error_type: None,
                retry_attempt: Some(2),
                max_retries: Some(10),
                retry_in_ms: Some(500)
            }
        );
        let mut a = assistant(
            "e2",
            "<synthetic>",
            serde_json::json!([{"type": "text", "text": "Rate limited"}]),
        );
        a["isApiErrorMessage"] = serde_json::json!(true);
        a["apiErrorStatus"] = serde_json::json!(429);
        a["error"] = serde_json::json!("rate_limit");
        assert!(matches!(
            one(a).kind,
            MessageKind::ApiError {
                status: Some(429),
                ..
            }
        ));
    }

    /// A model change is derived between two assistant messages, and
    /// the synthetic model does not count as one.
    #[test]
    fn model_change_is_derived() {
        let page = parse(
            &body(&[
                assistant("a1", "model-a", serde_json::json!([])),
                assistant("a2", "<synthetic>", serde_json::json!([])),
                assistant("a3", "model-b", serde_json::json!([])),
            ]),
            WindowStart::FileStart,
            None,
        );
        let changes: Vec<_> = page
            .messages
            .iter()
            .filter(|m| matches!(m.kind, MessageKind::ModelChange { .. }))
            .collect();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].id, "a3/model");
        assert_eq!(changes[0].id_source, IdSource::Derived);
        assert_eq!(
            changes[0].kind,
            MessageKind::ModelChange {
                from: "model-a".into(),
                to: "model-b".into()
            }
        );
    }

    /// A permission-mode record has no uuid and gets an anchored id.
    #[test]
    fn permission_mode_change_gets_an_anchored_id() {
        let page = parse(
            &body(&[
                serde_json::json!({"type": "permission-mode", "permissionMode": "plan"}),
                user("u1", serde_json::json!("hi")),
                serde_json::json!({"type": "permission-mode", "permissionMode": "auto"}),
                serde_json::json!({"type": "permission-mode", "permissionMode": "auto"}),
            ]),
            WindowStart::FileStart,
            None,
        );
        let ids: Vec<(&str, IdSource)> = page
            .messages
            .iter()
            .map(|m| (m.id.as_str(), m.id_source))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("^+1", IdSource::Anchored),
                ("u1", IdSource::Uuid),
                ("u1+1", IdSource::Anchored),
                ("u1+2", IdSource::Anchored),
            ]
        );
        assert_eq!(
            page.messages[0].kind,
            MessageKind::PermissionModeChange {
                mode: "plan".into()
            }
        );
    }

    /// Mid-file, a uuid-less record before any anchor is unanchored, and
    /// identical lines still get distinct ids.
    #[test]
    fn unanchored_ids_are_distinct_and_marked() {
        let pm = serde_json::json!({"type": "permission-mode", "permissionMode": "auto"});
        let page = parse(&body(&[pm.clone(), pm]), WindowStart::MidFile, None);
        assert_eq!(page.messages.len(), 2);
        assert!(page
            .messages
            .iter()
            .all(|m| m.id_source == IdSource::Unanchored && m.id.starts_with('~')));
        assert_ne!(page.messages[0].id, page.messages[1].id);
    }

    /// An image is a placeholder with its size estimated, never bytes.
    #[test]
    fn image_is_a_placeholder() {
        let m = one(user(
            "p1",
            serde_json::json!([{"type": "image", "source":
                {"type": "base64", "media_type": "image/png", "data": "AAAAAAAA"}}]),
        ));
        let TranscriptBlock::Image { image, .. } = &m.blocks[0] else {
            panic!("{:#?}", m.blocks)
        };
        assert_eq!(image.media_type.as_deref(), Some("image/png"));
        assert_eq!(image.approx_bytes, Some(6));
        assert_eq!(image.width, None);
        let wire = serde_json::to_string(&m).unwrap();
        assert!(
            !wire.contains("AAAAAAAA"),
            "image bytes on the wire: {wire}"
        );
    }

    /// A subagent call links to its transcript under `subagents/`.
    #[test]
    fn subagent_call_links_to_its_transcript() {
        let dir = std::env::temp_dir().join(format!(
            "headstate-tmodel-sub-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let parent = dir.join("session-1.jsonl");
        let sub = dir.join("session-1").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("agent-abc123.jsonl"), "").unwrap();

        let c = assistant(
            "a1",
            "model-a",
            serde_json::json!([{"type": "tool_use", "id": "t1", "name": "Agent",
                "input": {"description": "d", "prompt": "p", "subagent_type": "general"}}]),
        );
        let mut r = result("r1", "t1");
        r["toolUseResult"] = serde_json::json!({"agentId": "abc123", "status": "completed"});
        let mut missing = result("r2", "t2");
        missing["toolUseResult"] = serde_json::json!({"agentId": "fff999"});
        let page = parse(
            &body(&[c, r, missing]),
            WindowStart::FileStart,
            Some(&parent),
        );
        let TranscriptBlock::ToolCall {
            result: Some(out),
            args,
            ..
        } = &page.messages[0].blocks[0]
        else {
            panic!("{:#?}", page.messages)
        };
        assert!(matches!(args, ToolArgs::Task { .. }));
        let s = out.subagent.as_ref().unwrap();
        assert_eq!(s.agent_id, "abc123");
        assert_eq!(s.status.as_deref(), Some("completed"));
        assert_eq!(s.transcript_found, Some(true));
        assert_eq!(
            s.transcript_path.as_deref(),
            Some(
                sub.join("agent-abc123.jsonl")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        let TranscriptBlock::ToolResult(o2) = &page.messages[1].blocks[0] else {
            panic!()
        };
        assert_eq!(o2.subagent.as_ref().unwrap().transcript_found, Some(false));

        // Without a transcript path the link is unchecked, not missing.
        let page = parse(&body(&[result("r3", "t3")]), WindowStart::FileStart, None);
        assert!(
            matches!(&page.messages[0].blocks[0], TranscriptBlock::ToolResult(o) if o.subagent.is_none())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent id that could walk out of the directory is refused.
    #[test]
    fn a_hostile_agent_id_makes_no_path() {
        let p = Path::new("/x/s.jsonl");
        assert!(subagent_transcript(p, "../../etc").is_none());
        assert!(subagent_transcript(p, "").is_none());
        assert!(subagent_transcript(p, "a1b2").is_some());
    }

    /// A background agent reporting back, and harness-injected text, are
    /// not the user speaking -- and neither opens a turn.
    #[test]
    fn notifications_and_injected_text_are_not_user_prompts() {
        let n = one(user(
            "n1",
            serde_json::json!("<task-notification>\n<task-id>x1</task-id>\n<status>completed</status>\n</task-notification>"),
        ));
        assert_eq!(
            n.kind,
            MessageKind::AgentNotification {
                task_id: Some("x1".into()),
                status: Some("completed".into())
            }
        );
        assert_eq!(n.turn_id, None);
        let mut m = user("m1", serde_json::json!("Base directory for this skill"));
        m["isMeta"] = serde_json::json!(true);
        let m = one(m);
        assert_eq!(m.kind, MessageKind::Injected { origin: None });
        assert!(m.is_meta);
    }

    /// Notices and turn durations from `system` records.
    #[test]
    fn notices_and_turn_duration() {
        let n = one(serde_json::json!({
            "type": "system", "subtype": "away_summary", "uuid": "n1", "content": "While away"
        }));
        assert!(
            matches!(n.kind, MessageKind::Notice { ref subtype, .. } if subtype == "away_summary")
        );
        let d = one(serde_json::json!({
            "type": "system", "subtype": "turn_duration", "uuid": "d1",
            "durationMs": 1500, "messageCount": 4
        }));
        assert_eq!(d.duration_ms, Some(1500));
        assert_eq!(
            d.kind,
            MessageKind::TurnDuration {
                message_count: Some(4)
            }
        );
    }

    /// A queued prompt, and a legacy summary record.
    #[test]
    fn queued_prompt_and_legacy_summary() {
        let q = one(serde_json::json!({
            "type": "attachment", "uuid": "q1",
            "attachment": {"type": "queued_command", "prompt": "next", "commandMode": "prompt"}
        }));
        assert_eq!(
            q.kind,
            MessageKind::QueuedPrompt {
                mode: Some("prompt".into())
            }
        );
        let s = one(serde_json::json!({"type": "summary", "summary": "A title", "leafUuid": "l1"}));
        assert_eq!(
            s.kind,
            MessageKind::Summary {
                leaf_uuid: Some("l1".into())
            }
        );
    }

    // ---- unknown kinds -----------------------------------------------

    /// An unknown record type, system subtype or attachment subtype
    /// becomes an explicit Unrecognised message, never nothing.
    #[test]
    fn unknown_kinds_become_unrecognised() {
        let page = parse(
            &body(&[
                serde_json::json!({"type": "brand-new-thing", "uuid": "x1"}),
                serde_json::json!({"type": "system", "subtype": "agents_killed", "uuid": "x2"}),
                serde_json::json!({"type": "attachment", "uuid": "x3",
                    "attachment": {"type": "novel_attachment"}}),
            ]),
            WindowStart::FileStart,
            None,
        );
        let types: Vec<String> = page
            .messages
            .iter()
            .map(|m| match &m.kind {
                MessageKind::Unrecognised { record_type } => record_type.clone(),
                k => panic!("{k:?}"),
            })
            .collect();
        assert_eq!(
            types,
            vec![
                "brand-new-thing",
                "system/agents_killed",
                "attachment/novel_attachment"
            ]
        );
    }

    /// Known bookkeeping is counted by type and not rendered.
    #[test]
    fn machinery_is_counted_not_rendered() {
        let page = parse(
            &body(&[
                serde_json::json!({"type": "ai-title", "aiTitle": "t"}),
                serde_json::json!({"type": "ai-title", "aiTitle": "t"}),
                serde_json::json!({"type": "attachment", "uuid": "k1",
                    "attachment": {"type": "skill_listing"}}),
            ]),
            WindowStart::FileStart,
            None,
        );
        assert!(page.messages.is_empty(), "{:#?}", page.messages);
        assert_eq!(
            page.machinery_records,
            vec![
                ("ai-title".to_owned(), 2),
                ("attachment/skill_listing".to_owned(), 1)
            ]
        );
        assert_eq!(page.unparseable_records, 0);
    }

    // ---- ids, turns, pairing across a split --------------------------

    /// Ids survive a re-read, and a window that starts later keeps the
    /// uuid ids of the records it shares.
    #[test]
    fn ids_are_stable_across_a_reread() {
        let recs = vec![
            user("u1", serde_json::json!("one")),
            call("a1", "t1"),
            result("r1", "t1"),
            serde_json::json!({"type": "permission-mode", "permissionMode": "auto"}),
            user("u2", serde_json::json!("two")),
            assistant(
                "a2",
                "model-a",
                serde_json::json!([{"type": "text", "text": "ok"}]),
            ),
        ];
        let b = body(&recs);
        let first = parse(&b, WindowStart::FileStart, None);
        let again = parse(&b, WindowStart::FileStart, None);
        assert_eq!(first, again);

        let later = parse(&body(&recs[2..]), WindowStart::MidFile, None);
        // Every record id a page holds: its messages', and the records
        // whose tool output was merged into a call.
        let ids = |p: &TranscriptPage| -> Vec<String> {
            let mut out = Vec::new();
            for m in p.messages.iter().filter(|m| m.id_source == IdSource::Uuid) {
                out.push(m.id.clone());
                for b in &m.blocks {
                    if let TranscriptBlock::ToolCall {
                        result: Some(o), ..
                    } = b
                    {
                        out.push(o.message_id.clone());
                    }
                }
            }
            out
        };
        for id in ids(&later) {
            assert!(ids(&first).contains(&id), "{id} not in the full read");
        }
        // The anchored record keeps its id in any window holding its anchor.
        assert!(first.messages.iter().any(|m| m.id == "r1+1"));
        assert!(later.messages.iter().any(|m| m.id == "r1+1"));
    }

    /// Every message is grouped under the prompt that opened its turn.
    #[test]
    fn turns_group_under_their_opener() {
        let page = parse(
            &body(&[
                assistant(
                    "a0",
                    "model-a",
                    serde_json::json!([{"type": "text", "text": "tail"}]),
                ),
                user("u1", serde_json::json!("one")),
                call("a1", "t1"),
                result("r1", "t1"),
                user("u2", serde_json::json!("two")),
                assistant(
                    "a2",
                    "model-a",
                    serde_json::json!([{"type": "text", "text": "ok"}]),
                ),
            ]),
            WindowStart::MidFile,
            None,
        );
        let turns: Vec<(&str, Option<&str>)> = page
            .messages
            .iter()
            .map(|m| (m.id.as_str(), m.turn_id.as_deref()))
            .collect();
        assert_eq!(
            turns,
            vec![
                ("a0", None),
                ("u1", Some("u1")),
                ("a1", Some("u1")),
                ("u2", Some("u2")),
                ("a2", Some("u2")),
            ]
        );
    }

    /// A call in one page and its result in the next pair once both are
    /// loaded -- identically to one read of both.
    #[test]
    fn pairing_holds_across_a_split() {
        let recs = vec![
            user("u1", serde_json::json!("go")),
            call("a1", "t1"),
            call("a2", "t2"),
            result("r1", "t1"),
            result("r2", "t2"),
        ];
        let whole = parse(&body(&recs), WindowStart::FileStart, None).messages;
        // The reference itself must be paired, or "equal to the whole
        // read" would hold just as well with pairing switched off.
        let answered: Vec<&str> = whole
            .iter()
            .flat_map(|m| &m.blocks)
            .filter_map(|b| match b {
                TranscriptBlock::ToolCall {
                    result: Some(o), ..
                } => Some(o.message_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(answered, vec!["r1", "r2"], "{whole:#?}");
        assert!(whole.iter().all(|m| m.kind != MessageKind::ToolResults));
        for cut in 1..recs.len() {
            let mut older = parse(&body(&recs[..cut]), WindowStart::FileStart, None).messages;
            let newer = parse(&body(&recs[cut..]), WindowStart::MidFile, None).messages;
            older.extend(newer);
            settle(&mut older);
            assert_eq!(older, whole, "split at {cut}");
            // Settling again changes nothing.
            let again = older.clone();
            settle(&mut older);
            assert_eq!(older, again, "settle is not idempotent at {cut}");
        }
    }

    /// A second, different result for an answered call does not replace
    /// the first.
    #[test]
    fn a_second_result_does_not_replace_the_first() {
        let page = parse(
            &body(&[call("a1", "t1"), result("r1", "t1"), result("r2", "t1")]),
            WindowStart::FileStart,
            None,
        );
        let TranscriptBlock::ToolCall {
            result: Some(out), ..
        } = &page.messages[0].blocks[0]
        else {
            panic!()
        };
        assert_eq!(out.message_id, "r1");
        assert_eq!(page.messages[1].id, "r2");
    }

    /// A duplicated uuid keeps the first record and counts the rest.
    #[test]
    fn a_duplicate_uuid_keeps_the_first() {
        let mut dup = user("u1", serde_json::json!("hello"));
        dup["gitBranch"] = serde_json::json!("other");
        let page = parse(
            &body(&[user("u1", serde_json::json!("hello")), dup]),
            WindowStart::FileStart,
            None,
        );
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.duplicate_records, 1);
    }

    // ---- truncation and full text ------------------------------------

    /// A clipped block says how much of how much, per block.
    #[test]
    fn clipped_blocks_state_their_clip() {
        let long = "é".repeat(preview::MAX_TEXT_CHARS + 10);
        let m = one(user(
            "u1",
            serde_json::json!([{"type": "text", "text": long}]),
        ));
        let TranscriptBlock::Text { text, clip, .. } = &m.blocks[0] else {
            panic!()
        };
        assert_eq!(text.chars().count(), preview::MAX_TEXT_CHARS);
        assert_eq!(
            *clip,
            Some(TranscriptClip {
                shown_chars: preview::MAX_TEXT_CHARS,
                total_chars: preview::MAX_TEXT_CHARS + 10
            })
        );
    }

    fn tmp_file(tag: &str, contents: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "headstate-tmodel-{tag}-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&p, contents).unwrap();
        p
    }

    /// The full text of a clipped block comes back by address, and is
    /// the text the block was clipped from.
    #[test]
    fn full_text_is_fetched_by_id_and_index() {
        let long = "x".repeat(preview::MAX_TEXT_CHARS * 3);
        let recs = [
            call("a1", "t1"),
            user(
                "r1",
                serde_json::json!([{"type": "tool_result", "tool_use_id": "t1", "content": long}]),
            ),
        ];
        // CRLF, so the fetch's line handling is exercised on Windows endings.
        let p = tmp_file("full", &body(&recs).replace('\n', "\r\n"));
        let page = tail(&p).unwrap();
        let TranscriptBlock::ToolCall {
            result: Some(out), ..
        } = &page.messages[0].blocks[0]
        else {
            panic!()
        };
        assert!(out.clip.is_some());
        let full = block_text(&p, &out.message_id, out.index).unwrap();
        assert_eq!(full.text, long);
        assert_eq!(full.clip, None);

        assert!(block_text(&p, "r1", 5).is_err());
        assert!(block_text(&p, "missing", 0).is_err());
        assert!(block_text(&p, "^+1", 0).is_err());
        assert!(block_text(&p, "a1/model", 0).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// The fetch is bounded too, and says so.
    #[test]
    fn full_text_is_bounded() {
        let huge = "y".repeat(FULL_TEXT_CHARS + 5);
        let p = tmp_file(
            "bound",
            &body(&[user(
                "u1",
                serde_json::json!([{"type": "text", "text": huge}]),
            )]),
        );
        let full = block_text(&p, "u1", 0).unwrap();
        assert_eq!(full.text.len(), FULL_TEXT_CHARS);
        assert_eq!(
            full.clip,
            Some(TranscriptClip {
                shown_chars: FULL_TEXT_CHARS,
                total_chars: FULL_TEXT_CHARS + 5
            })
        );
        let _ = std::fs::remove_file(&p);
    }

    /// A tail that starts mid-file drops the partial first line, says it
    /// is truncated, and keys leading uuid-less records as unanchored.
    #[test]
    fn a_mid_file_tail_is_truncated() {
        let pad = user(
            "u0",
            serde_json::json!("z".repeat(preview::TAIL_BYTES as usize)),
        );
        let recs = [
            pad,
            serde_json::json!({"type": "permission-mode", "permissionMode": "auto"}),
            user("u1", serde_json::json!("hi")),
        ];
        let p = tmp_file("mid", &body(&recs));
        let page = tail(&p).unwrap();
        assert!(page.truncated);
        assert!(page.bytes_read <= preview::TAIL_BYTES);
        assert_eq!(page.messages.len(), 2, "{:#?}", page.messages);
        assert_eq!(page.messages[0].id_source, IdSource::Unanchored);
        assert_eq!(page.messages[1].id, "u1");
        let _ = std::fs::remove_file(&p);
    }
}
