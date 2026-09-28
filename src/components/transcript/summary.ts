/// The one-line summaries the tool renderers show (#1483). Pure.
///
/// Every count here is taken from a result's TEXT, which may be clipped
/// (`clip` non-null). A count from clipped text is a floor and says so
/// ("at least 40 lines"); a count that cannot be read at all is not
/// shown rather than shown as zero.

import type { Liveness } from "../../types/pr";
import type { TranscriptClip, TranscriptToolOutput } from "../../types/transcript";
import { linesOf } from "./diff";
import type { ToolCallBlock } from "./types";

/// "12 lines", "1 line", or "at least 12 lines" when the text was clipped.
export function countLabel(
  n: number,
  unit: string,
  clipped: boolean,
  plural = `${unit}s`,
): string {
  const noun = n === 1 ? unit : plural;
  return `${clipped ? "at least " : ""}${n.toLocaleString()} ${noun}`;
}

export function isClipped(clip: TranscriptClip | null): boolean {
  return clip !== null && clip.shown_chars < clip.total_chars;
}

/// How a Bash call ended, from what its result recorded.
///
/// Claude Code writes a non-zero exit as an error result whose text
/// begins `Exit code N`. A non-error result records no exit code at
/// all (a backgrounded command is not an error either), so it reads as
/// `completed`, never as "exit 0". `null` `is_error` was not recorded.
export type BashOutcome =
  | { kind: "error"; code: number | null }
  | { kind: "completed" }
  | { kind: "not_recorded" };

export function bashOutcome(result: TranscriptToolOutput): BashOutcome {
  if (result.is_error === true) {
    const m = /^Exit code (\d+)/.exec(result.text);
    return { kind: "error", code: m ? Number(m[1]) : null };
  }
  if (result.is_error === false) return { kind: "completed" };
  return { kind: "not_recorded" };
}

/// `mcp__server__tool` split into its server and tool, or `null` for a
/// name that is not an MCP tool's.
export function mcpName(name: string): { server: string; tool: string } | null {
  const m = /^mcp__(.+?)__(.+)$/.exec(name);
  return m ? { server: m[1], tool: m[2] } : null;
}

/// A Read result's numbered lines: how many, and the first and last
/// line numbers, from Claude Code's `   N\t` prefix. `null` when the text
/// carries no numbered lines (an error, an image).
export function readSpan(
  text: string,
): { count: number; first: number; last: number } | null {
  let count = 0;
  let first = 0;
  let last = 0;
  for (const line of linesOf(text)) {
    const m = /^\s*(\d+)\t/.exec(line);
    if (!m) continue;
    const n = Number(m[1]);
    if (count === 0) first = n;
    last = n;
    count++;
  }
  return count === 0 ? null : { count, first, last };
}

/// What a Grep or Glob found, read from its result text.
///
/// - Glob, and Grep's default `files_with_matches`: a file list, which
///   Grep heads with `Found N files`.
/// - Grep `count`: `path:N` lines, summed.
/// - Grep `content`: matching lines; context lines may be among them, so
///   this says "lines", not "matches".
export interface SearchSummary {
  label: string;
  /// The files or lines, for the expanded list.
  items: string[];
}

export function searchSummary(
  tool: "grep" | "glob",
  outputMode: string | null,
  text: string,
  clipped: boolean,
): SearchSummary {
  const lines = linesOf(text).filter((l) => l.trim() !== "");
  if (lines.length === 0 || /^No (files|matches) found/.test(lines[0])) {
    // Measured, and it was nothing -- unless clipped, which cannot
    // happen to an empty text.
    return { label: tool === "glob" ? "0 files" : "no matches", items: [] };
  }
  const mode = tool === "glob" ? "files" : (outputMode ?? "files_with_matches");
  if (mode === "count") {
    let total = 0;
    let files = 0;
    for (const l of lines) {
      const m = /:(\d+)$/.exec(l);
      if (m) {
        total += Number(m[1]);
        files++;
      }
    }
    if (files > 0) {
      return {
        label: `${countLabel(total, "match", clipped, "matches")} in ${countLabel(files, "file", clipped)}`,
        items: lines,
      };
    }
  }
  if (mode === "content") {
    return { label: countLabel(lines.length, "line", clipped), items: lines };
  }
  // A file list. Grep heads it with its own total, which is the true
  // count even when the text was clipped.
  const head = /^Found (\d+) files?/.exec(lines[0]);
  const items = (head ? lines.slice(1) : lines).filter((l) => !/^\(Results are truncated/.test(l));
  if (head) return { label: countLabel(Number(head[1]), "file", false), items };
  return { label: countLabel(items.length, "file", clipped), items };
}

/// Where a tool call stands, as the renderer has to say it.
///
/// Four states the issue names, kept apart, plus the two ways the
/// question cannot be answered:
///
/// - `paired`: its result is loaded.
/// - `running`: no result yet, and the session IS live. Only then.
/// - `not_recorded`: no result, and the session is not running -- the
///   call never came back.
/// - `unknown`: no result, and whether the session is live could not be
///   determined. NOT a shade of `not_recorded`.
/// - `unkeyed`: the call carries no id, so no result can be matched.
///
/// The fourth issue state -- a result whose call is in an older page --
/// belongs to the RESULT, and `ToolResultOrphan` renders it.
export type CallState =
  | { state: "paired"; result: TranscriptToolOutput }
  | { state: "running" }
  | { state: "not_recorded" }
  | { state: "unknown" }
  | { state: "unkeyed" };

export function callState(call: ToolCallBlock, liveness: Liveness): CallState {
  if (call.result) return { state: "paired", result: call.result };
  if (call.id === null) return { state: "unkeyed" };
  switch (liveness.state) {
    case "running":
      return { state: "running" };
    case "dead":
      return { state: "not_recorded" };
    case "unknown":
      return { state: "unknown" };
  }
}

/// Milliseconds between two recorded timestamps, or `null` when either
/// is missing or unparseable, or the order is backwards. Never 0 for
/// "could not tell".
export function durationBetween(start: string | null, end: string | null): number | null {
  if (!start || !end) return null;
  const a = Date.parse(start);
  const b = Date.parse(end);
  if (Number.isNaN(a) || Number.isNaN(b) || b < a) return null;
  return b - a;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const total = Math.round(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m} m ${String(s).padStart(2, "0")} s`;
}
