/// Each message's accessible name (#1489): who is speaking and when, the
/// way a screen reader should announce the row before reading it --
/// "You, 12:03", "Claude, 12:04", "Session summary, 12:05".
///
/// Both renderers use this, so a row is announced the same whichever
/// device draws it. The name is the speaker and the time only: the
/// row's text follows it in the reading order, and repeating it in the
/// name would read everything twice.
///
/// A record with no timestamp, or one that does not parse, is named
/// without a time -- never with a made-up one.

import type { TranscriptMessage } from "../../types/transcript";
import { dividerLabel } from "./transcriptCopy";

/// Who a record is from, in words.
function speaker(m: TranscriptMessage): string {
  switch (m.kind.kind) {
    case "user_prompt":
      return m.is_meta ? dividerLabel(m) : "You";
    case "slash_command":
    case "shell_input":
      return "You";
    case "queued_prompt":
      return "You, queued";
    case "assistant":
      return m.is_meta ? "Added by Claude Code" : "Claude";
    default:
      return dividerLabel(m);
  }
}

/// "12:03" in the reader's locale, or `null` when there is no usable time.
export function shortTime(ts: string | null): string | null {
  if (!ts) return null;
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return null;
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function messageName(m: TranscriptMessage): string {
  const time = shortTime(m.timestamp);
  return time === null ? speaker(m) : `${speaker(m)}, ${time}`;
}

/// A message this app is sending that the transcript does not hold yet
/// (#1491): it has no recorded time to name.
export const PENDING_NAME = "You, not in the transcript yet";
