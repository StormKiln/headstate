/// How long things took, from the timestamps the transcript recorded
/// (#1481). Pure.
///
/// Nothing here is measured by Headstate: every figure is the gap
/// between two recorded timestamps, via `durationBetween`, and a gap
/// that cannot be taken is `null` -- not shown -- never zero.

import type { TranscriptMessage } from "../../../types/transcript";
import { durationBetween } from "../summary";
import type { ToolCallBlock } from "../types";

/// How long a paired call took: its message's timestamp to its result
/// record's. `null` when either was not recorded, or it has no result.
export function callDuration(call: ToolCallBlock, callAt: string | null): number | null {
  return durationBetween(callAt, call.result?.timestamp ?? null);
}

function later(a: string | null, b: string | null): string | null {
  if (a === null) return b;
  if (b === null) return a;
  const ta = Date.parse(a);
  const tb = Date.parse(b);
  if (Number.isNaN(tb)) return a;
  if (Number.isNaN(ta)) return b;
  return tb > ta ? b : a;
}

/// The latest thing recorded in one message: its own timestamp, or a
/// result merged into one of its calls, whichever came last. The result
/// records are absorbed by pairing, so without this a message's calls
/// would look as if they finished the moment they were made.
function lastEventIn(m: TranscriptMessage): string | null {
  let at = m.timestamp;
  for (const b of m.blocks) {
    if (b.kind === "tool_call" && b.result) at = later(at, b.result.timestamp);
  }
  return at;
}

/// For every message that holds a recorded thinking block, when the
/// thing before it was recorded -- the reply's earliest possible start.
///
/// Only messages with thinking are keyed; a message whose predecessor is
/// not loaded (the first in the window) or left no timestamp is absent,
/// and its thinking is shown without a duration.
export function thinkingStarts(messages: readonly TranscriptMessage[]): Map<string, string> {
  const out = new Map<string, string>();
  let before: string | null = null;
  for (const m of messages) {
    if (before !== null && m.blocks.some((b) => b.kind === "thinking" && b.recorded)) {
      out.set(m.id, before);
    }
    before = later(before, lastEventIn(m));
  }
  return out;
}

/// "Thought for about 14 s".
///
/// ABOUT, because the gap runs from the previous record to the thinking
/// record, so it also holds the request's own latency: it is close to
/// the thinking time, and never under it. `null` when the gap cannot be
/// taken; the row then says only "Thought".
export function thoughtLabel(ms: number | null): string {
  if (ms === null) return "Thought";
  if (ms < 1000) return "Thought for less than a second";
  const total = Math.round(ms / 1000);
  if (total < 60) return `Thought for about ${total} s`;
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `Thought for about ${m} m ${String(s).padStart(2, "0")} s`;
}
