/// Shared props for the transcript tool renderers (#1483, epic #1473).
///
/// Both renderers -- the desktop terminal (#1480) and the phone bubbles
/// (#1481) -- draw tool calls with these components and choose a
/// `variant`. The components are presentational: the only data they
/// fetch is through the callbacks passed in here.

import type {
  TranscriptBlock,
  TranscriptBlockText,
  TranscriptSubagent,
} from "../../types/transcript";

/// `terminal` is the desktop, Claude-Code-in-iTerm2 look: dense, output
/// expands in place. `compact` is the phone: output opens in a sheet
/// and the diff drops its second line-number gutter.
export type ToolVariant = "terminal" | "compact";

/// Where one block's full text lives: the record it was read from and
/// its position there. `claudeTranscriptBlockText` takes exactly this.
export interface BlockAddress {
  messageId: string;
  index: number;
  /// Where that record starts in the file (#1220): with it the fetch
  /// reads one record instead of scanning (#1476). `null` when unknown.
  offset: number | null;
}

/// Fetch one block's full text. Injected by the renderer, which knows
/// the transcript path; the components only call it.
export type LoadFullText = (address: BlockAddress) => Promise<TranscriptBlockText>;

/// Open a subagent's transcript in the same viewer.
export type OpenSubagent = (subagent: TranscriptSubagent) => void;

export type ToolCallBlock = Extract<TranscriptBlock, { kind: "tool_call" }>;
export type ToolResultBlock = Extract<TranscriptBlock, { kind: "tool_result" }>;
export type ThinkingBlock = Extract<TranscriptBlock, { kind: "thinking" }>;
