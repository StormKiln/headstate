/// The transcript read model (#1475, epic #1473).
///
/// Rust side: `src-tauri/src/claude/transcript_model.rs`, which measures
/// the record shapes this is cut against and argues the id rule, the
/// allowlist and the clip metadata. Both renderers (desktop terminal,
/// phone bubbles), live follow, paging and 7.10's sent-message echo
/// consume this one model.
///
/// Wire keys are snake_case, as every Rust wire type is.
///
/// The nested interfaces are EXPORTED and tagged `@public` for `yarn
/// knip`, where `pr.ts` would keep them private until a consumer
/// appears. The reason is the mirrored-type invariant
/// (`every_mirrored_type_agrees_with_its_rust_wire_spelling`): it checks
/// only `export interface` declarations, so a private mirror is an
/// unchecked one. Until the renderers (#1479-#1481) import them, `@public`
/// is what lets the guard see them.

import type { ClaudeFileChange, ClaudeToolArgs, TranscriptMasking } from "./pr";

/// Where a message's id came from. The variant says which promise the id
/// makes:
///
/// - `uuid`: the record's own uuid. Survives re-reads, other windows and
///   compaction. What the renderers and 7.10's echo key on.
/// - `anchored`: a uuid-less record, keyed `"<previous uuid>+<n>"` (`^`
///   at the start of the file). Stable in any read holding the anchor.
/// - `unanchored`: a uuid-less record read before any anchor in a window
///   that starts mid-file. Stable for the same bytes; may change if the
///   window moves.
/// - `derived`: produced, not read (a model change), keyed
///   `"<assistant id>/model"`.
type TranscriptIdSource = "uuid" | "anchored" | "unanchored" | "derived";

/** @public */
/// How much of one block's text was kept. Mirrors
/// `claude::transcript_model::TranscriptClip`.
///
/// Per block, so the renderer says "showing the first 4,000 of 38,210
/// characters" beside the block it clipped. `null` on a block means
/// nothing was clipped.
export interface TranscriptClip {
  shown_chars: number;
  total_chars: number;
}

/** @public */
/// Token usage as recorded. Mirrors
/// `claude::transcript_model::TranscriptUsage`.
///
/// Every counter is nullable: `null` is "the record did not say", never
/// zero. One API response is written as several records that repeat its
/// usage, so sum once per `api_message_id`, never per message.
export interface TranscriptUsage {
  input_tokens: number | null;
  output_tokens: number | null;
  cache_creation_input_tokens: number | null;
  cache_read_input_tokens: number | null;
}

/** @public */
/// An image as a placeholder, never its bytes. Mirrors
/// `claude::transcript_model::TranscriptImage`.
///
/// Dimensions are recorded only for tool-produced images; `null` is "not
/// recorded", not zero.
export interface TranscriptImage {
  media_type: string | null;
  approx_bytes: number | null;
  width: number | null;
  height: number | null;
}

/** @public */
/// A subagent call's link to its own transcript. Mirrors
/// `claude::transcript_model::TranscriptSubagent`.
export interface TranscriptSubagent {
  agent_id: string;
  /// `async_launched`, `completed`, ... verbatim.
  status: string | null;
  agent_type: string | null;
  /// Openable with `claudeTranscriptPage`, which applies the same
  /// path guard as the parent's.
  transcript_path: string | null;
  /// `null` when it was not checked, which is not "missing".
  transcript_found: boolean | null;
}

/** @public */
/// A tool's output. Mirrors
/// `claude::transcript_model::TranscriptToolOutput`.
///
/// Merged into the call it answers once both are loaded; standing alone
/// as a `tool_result` block while its call is not.
export interface TranscriptToolOutput {
  /// The record this was read from, and with `index` the address for
  /// `claudeTranscriptBlockText`.
  message_id: string;
  index: number;
  /// That record's timestamp, as recorded. Kept because merging absorbs
  /// the record, and without it a paired call's duration could not be
  /// measured. `null` when the record carried none.
  timestamp: string | null;
  /// Where that record starts in the file: pass it to
  /// `claudeTranscriptBlockText` so the fetch reads one record instead of
  /// scanning (#1220). `null` when parsed without a file position.
  offset: number | null;
  tool_use_id: string | null;
  text: string;
  clip: TranscriptClip | null;
  /// `null` when the record carried no `is_error`, which is not `false`.
  is_error: boolean | null;
  change: ClaudeFileChange | null;
  images: TranscriptImage[];
  subagent: TranscriptSubagent | null;
  /// Set when this is a `TaskCreate`'s or `TaskUpdate`'s output (#1504).
  task: TranscriptTaskResult | null;
  /// The record's `oversized_bytes`, carried with the output so it
  /// survives being merged into its call (#1476). `null` when the record
  /// was held whole.
  oversized_bytes: number | null;
}

/** @public */
/// What a task tool's result recorded (#1504). Mirrors
/// `claude::transcript_model::TranscriptTaskResult`.
///
/// The only place a created task's id is written. `success: false` can
/// arrive with `is_error: null`, so a refused update is read from
/// `success`, not only from `is_error`. `null` everywhere is "the record
/// did not say".
export interface TranscriptTaskResult {
  task_id: string | null;
  success: boolean | null;
  status_from: string | null;
  status_to: string | null;
}

/// One content block. `index` is the block's position in the record it
/// came from -- with the message id, the full-text fetch address.
export type TranscriptBlock =
  | { kind: "text"; index: number; text: string; clip: TranscriptClip | null }
  /// `recorded: false` when only the signature was kept: render
  /// "thinking (not recorded)", never empty thought.
  | {
      kind: "thinking";
      index: number;
      text: string;
      clip: TranscriptClip | null;
      recorded: boolean;
    }
  /// `result: null` means no result is in the LOADED messages. Whether
  /// that is "still running" or "never came back" is liveness's question.
  | {
      kind: "tool_call";
      index: number;
      name: string;
      id: string | null;
      args: ClaudeToolArgs;
      result: TranscriptToolOutput | null;
    }
  | ({ kind: "tool_result" } & TranscriptToolOutput)
  | { kind: "image"; index: number; image: TranscriptImage }
  | { kind: "other"; index: number; block_type: string };

/// What a message IS. Mirrors the Rust `MessageKind`, variant for variant.
///
/// `unrecognised` renders as a thin divider naming `record_type`, never
/// as nothing. `agent_notification` and `injected` are NOT the user
/// speaking, though Claude Code writes them as user records.
type TranscriptMessageKind =
  | { kind: "user_prompt"; origin: string | null }
  | { kind: "slash_command"; name: string }
  | { kind: "shell_input"; command: string }
  | { kind: "command_output"; command: string | null }
  | { kind: "assistant" }
  | { kind: "tool_results" }
  | { kind: "agent_notification"; task_id: string | null; status: string | null }
  /// A background task's recorded state (#1483); the description is the
  /// first text block.
  | {
      kind: "task_status";
      task_id: string | null;
      task_type: string | null;
      status: string | null;
    }
  | { kind: "injected"; origin: string | null }
  | { kind: "queued_prompt"; mode: string | null }
  | { kind: "interruption"; during_tool_use: boolean }
  | {
      kind: "compaction_boundary";
      trigger: string | null;
      pre_tokens: number | null;
      post_tokens: number | null;
    }
  | { kind: "compaction_summary" }
  | { kind: "summary"; leaf_uuid: string | null }
  | {
      kind: "api_error";
      status: number | null;
      error_type: string | null;
      retry_attempt: number | null;
      max_retries: number | null;
      retry_in_ms: number | null;
    }
  | {
      kind: "hook_output";
      event: string | null;
      name: string | null;
      outcome: string;
      exit_code: number | null;
      prevented_continuation: boolean | null;
    }
  | { kind: "turn_duration"; message_count: number | null }
  | { kind: "notice"; subtype: string; level: string | null }
  | { kind: "model_change"; from: string; to: string }
  | { kind: "permission_mode_change"; mode: string }
  | { kind: "unrecognised"; record_type: string };

/** @public */
/// One message, ready to render. Mirrors
/// `claude::transcript_model::TranscriptMessage`.
export interface TranscriptMessage {
  /// Stable id; see `TranscriptIdSource` for the rule and its fallback.
  id: string;
  id_source: TranscriptIdSource;
  /// The id of the prompt, slash command or shell input that opened this
  /// message's turn; equal to `id` on the opener. `null` when the turn
  /// began above what was read.
  turn_id: string | null;
  kind: TranscriptMessageKind;
  /// RFC 3339 as recorded. Never substituted.
  timestamp: string | null;
  model: string | null;
  api_message_id: string | null;
  usage: TranscriptUsage | null;
  duration_ms: number | null;
  is_meta: boolean;
  is_sidechain: boolean;
  blocks: TranscriptBlock[];
  /// Where the record starts in the file (#1220): the full-text fetch's
  /// hint. `null` for a derived message.
  offset: number | null;
  /// Set when the record was too large for a page to hold: its size in
  /// bytes. Its blocks are clipped, each clip stating the true length,
  /// and the message is still here -- never dropped (#1220).
  oversized_bytes: number | null;
}

/// A window of a transcript as messages. Mirrors
/// `claude::transcript_model::TranscriptPage`.
export interface TranscriptPage {
  /// Oldest first.
  messages: TranscriptMessage[];
  /// Whether anything before these messages was not read. The viewer
  /// must say so.
  truncated: boolean;
  bytes_read: number;
  file_bytes: number;
  /// Bookkeeping records in the window by type, commonest first.
  machinery_records: [string, number][];
  unparseable_records: number;
  /// Repeated uuids; the first occurrence was kept.
  duplicate_records: number;
}

/// A page as it reaches the webview: on a paired phone, with the masking
/// summary the desktop's remote boundary attaches (#1488).
///
/// An alias, not a field on `TranscriptPage`: the read model does not
/// emit `masking` -- `remote::privacy::Plan::finish` adds the key to
/// whatever a transcript command returned -- so declaring it on the
/// mirror would be a key the mirrored Rust type never serialises.
/// Absent on the desktop's own answers, which are never masked.
export type RemoteTranscriptPage = TranscriptPage & { masking?: TranscriptMasking };

/// A paged read (#1220) as it reaches the webview: the same masking
/// summary, attached at the top level by the remote boundary, for the
/// same reason `RemoteTranscriptPage` is an alias.
export type RemoteTranscriptWindow = TranscriptWindow & { masking?: TranscriptMasking };

/// One block's full text, fetched by address. Mirrors
/// `claude::transcript_model::TranscriptBlockText`.
export interface TranscriptBlockText {
  message_id: string;
  index: number;
  text: string;
  /// Set when even the fetch's own bound bit.
  clip: TranscriptClip | null;
}

/// Where a page is read from (#1220). Mirrors the Rust `PageAnchor`.
///
/// `cursor` is a `PageCursor` a previous page returned, handed back
/// unread: `{ kind: "cursor", ...window.start }`.
export type TranscriptPageAnchor =
  | { kind: "start" }
  | { kind: "end" }
  | { kind: "cursor"; offset: number; behind_digest: string };

/// Which way from the anchor: `before` is older messages.
export type TranscriptPageDirection = "before" | "after";

/** @public */
/// A record boundary and a digest of the bytes behind it. Mirrors
/// `claude::transcript_page::PageCursor`. Opaque except for `offset`,
/// which orders pages: they never overlap.
export interface PageCursor {
  offset: number;
  behind_digest: string;
}

/// What a position figure rests on. Only `whole_file` is a count.
type TranscriptPositionBasis = "whole_file" | "index" | "partial_index" | "bytes";

/** @public */
/// Where a page sits in the whole transcript. Mirrors
/// `claude::transcript_page::TranscriptPosition`.
///
/// Render with `positionLabel` (`src/lib/transcriptPages.ts`), which says
/// "estimate" whenever `exact` is false. `null` is "nothing to place" or
/// "nothing to estimate from", never 0.
export interface TranscriptPosition {
  first: number | null;
  last: number | null;
  total: number | null;
  exact: boolean;
  basis: TranscriptPositionBasis;
}

/** @public */
/// A page's first real assistant message. Mirrors
/// `claude::transcript_page::SeamModel`.
export interface SeamModel {
  message_id: string;
  model: string;
  timestamp: string | null;
}

/** @public */
/// What joins a page to the one before it. Mirrors
/// `claude::transcript_page::PageSeam`. The server decides which
/// assistant messages count; the merge only compares strings.
export interface PageSeam {
  first_model: SeamModel | null;
  last_model: string | null;
}

/** @public */
/// One bounded page of a transcript (#1220). Mirrors
/// `claude::transcript_page::TranscriptWindow`.
///
/// Messages are settled WITHIN the page. Join pages with `mergeWindows`
/// (`src/lib/transcriptPages.ts`), which pairs results into calls across
/// pages by id and fills the seams -- never by concatenating.
export interface TranscriptWindow {
  page: TranscriptPage;
  /// The page is exactly the records in `[start.offset, end.offset)`.
  start: PageCursor;
  end: PageCursor;
  at_start: boolean;
  at_end: boolean;
  /// The anchor no longer described the file, so this page came from the
  /// end instead: every page held before it is stale. Replace, do not
  /// merge.
  rewritten: boolean;
  position: TranscriptPosition;
  seam: PageSeam;
  /// Bytes streamed past without being held (oversized records).
  bytes_scanned: number;
}

/// A nudge that a RUNNING session's transcript changed (#1477): the
/// payload of the `claude-session-activity` event. Mirrors
/// `claude::activity::SessionActivity`.
///
/// Content-free by design: an id the session list already carries, the
/// file's byte size (the same measure as a page's `file_bytes`), and a
/// sequence number that resets when the desktop restarts. Never order on
/// `seq`, and never treat a nudge as data: a lost one costs latency only,
/// because the follow keeps its own poll.
export interface SessionActivity {
  session_id: string;
  size: number;
  seq: number;
}

/** @public */
/// One message a find located (#1484). Mirrors
/// `claude::transcript_page::FindHit`.
///
/// `cursor` reads the page starting at the message:
/// `{ kind: "cursor", ...hit.cursor }`, `"after"`. A result record's id
/// is its own, although the merge absorbs it into its call.
export interface FindHit {
  message_id: string;
  cursor: PageCursor;
  timestamp: string | null;
  /// The text around the match, or an opener's first text.
  snippet: string;
  /// A prompt, slash command or shell input: a turn opener.
  opener: boolean;
}

/** @public */
/// What a find over one whole transcript located (#1484). Mirrors
/// `claude::transcript_page::TranscriptFind`.
///
/// Both bounds say so: `more` when the hit limit was reached, and
/// `complete: false` when the scan's deadline stopped it at
/// `scanned_to`. Hits already found are kept either way.
export interface TranscriptFind {
  /// Oldest first.
  hits: FindHit[];
  more: boolean;
  complete: boolean;
  scanned_to: number;
  file_bytes: number;
  /// Records too large to hold, searched only in part.
  skimmed_records: number;
}

/// A find as it reaches the webview: on a paired phone, with the masking
/// summary the remote boundary attaches (#1488).
export type RemoteTranscriptFind = TranscriptFind & { masking?: TranscriptMasking };
