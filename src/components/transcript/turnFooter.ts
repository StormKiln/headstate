/// The muted footer under each assistant turn on the desktop (#1480):
/// tokens, duration and model, as Claude Code prints after a turn.
///
/// Computed over the messages the viewer HOLDS, once per change, and
/// looked up by the renderer per message: a renderer sees one message at
/// a time and a turn's figures need all of them.
///
/// # What each figure promises
///
/// - **Tokens** are OUTPUT tokens, summed once per `api_message_id`: one
///   API response is written as several records that repeat its usage
///   (`TranscriptUsage`'s own rule). A response whose usage did not say
///   is not counted as zero; the sum is then qualified "at least".
/// - **Duration** is the turn's recorded `turn_duration` when Claude Code
///   wrote one, and otherwise the span between the opener's timestamp and
///   the turn's last. A turn that began above what was read has no
///   opener, so its span is not measured -- not shown, never guessed.
/// - **Model** is every model that answered in the turn, in order.
///
/// A turn that began above the read, or that is still being written, has
/// figures that are only floors; `qualifier` says which.

import type { TranscriptMessage } from "../../types/transcript";
import { durationBetween } from "./summary";

export interface TurnFooter {
  /// Summed output tokens, or `null` when no response in the turn
  /// recorded any.
  outputTokens: number | null;
  /// Some response in the turn did not record its output tokens, or the
  /// turn began above what was read: the sum is a floor.
  tokensPartial: boolean;
  durationMs: number | null;
  /// Where `durationMs` came from, for its title.
  durationSource: "recorded" | "timestamps" | null;
  models: string[];
  /// Why the figures are floors, if they are.
  qualifier: "began_above" | "in_progress" | null;
}

/// The model Claude Code writes on records it made itself, not an API
/// response.
const SYNTHETIC = "<synthetic>";

/// Footers keyed by the id of the message each one is drawn under.
///
/// The footer goes under the turn's LAST assistant message, or under its
/// `turn_duration` record when that comes after it (the record is then
/// drawn as the footer and nothing else). A turn with neither has nothing
/// to summarise and gets no footer.
///
/// `live` marks the newest turn as still being written, when the caller
/// knows it is.
export function turnFooters(
  messages: readonly TranscriptMessage[],
  live: boolean,
): Map<string, TurnFooter> {
  // Group by turn, in order. `null` is "began above the read"; the read
  // model only ever leaves that at the top, before the first opener.
  const turns: { id: string | null; messages: TranscriptMessage[] }[] = [];
  for (const m of messages) {
    const last = turns[turns.length - 1];
    if (last && last.id === m.turn_id) last.messages.push(m);
    else turns.push({ id: m.turn_id, messages: [m] });
  }

  const out = new Map<string, TurnFooter>();
  turns.forEach((turn, i) => {
    const host = footerHost(turn.messages);
    if (!host) return;
    const newest = i === turns.length - 1;
    out.set(host.id, summarise(turn.id, turn.messages, live && newest));
  });
  return out;
}

function footerHost(messages: readonly TranscriptMessage[]): TranscriptMessage | null {
  let host: TranscriptMessage | null = null;
  for (const m of messages) {
    if (m.kind.kind === "assistant" && !m.is_sidechain) host = m;
    else if (m.kind.kind === "turn_duration") host = m;
  }
  return host;
}

function summarise(
  turnId: string | null,
  messages: readonly TranscriptMessage[],
  inProgress: boolean,
): TurnFooter {
  const beganAbove = turnId === null;

  // Once per API response.
  const seen = new Set<string>();
  let tokens: number | null = null;
  let unrecorded = false;
  const models: string[] = [];
  for (const m of messages) {
    if (m.kind.kind !== "assistant") continue;
    if (m.model && m.model !== SYNTHETIC && !models.includes(m.model)) models.push(m.model);
    if (m.model === SYNTHETIC) continue;
    const key = m.api_message_id ?? `message:${m.id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const n = m.usage?.output_tokens ?? null;
    if (n === null) unrecorded = true;
    else tokens = (tokens ?? 0) + n;
  }

  let durationMs: number | null = null;
  let durationSource: TurnFooter["durationSource"] = null;
  const recorded = messages.find((m) => m.kind.kind === "turn_duration" && m.duration_ms !== null);
  if (recorded) {
    durationMs = recorded.duration_ms;
    durationSource = "recorded";
  } else if (!beganAbove) {
    const opener = messages.find((m) => m.id === turnId) ?? null;
    const last = [...messages].reverse().find((m) => m.timestamp !== null) ?? null;
    const span = durationBetween(opener?.timestamp ?? null, last?.timestamp ?? null);
    if (span !== null && last !== opener) {
      durationMs = span;
      durationSource = "timestamps";
    }
  }

  return {
    outputTokens: tokens,
    tokensPartial: tokens !== null && (unrecorded || beganAbove || inProgress),
    durationMs,
    durationSource,
    models,
    qualifier: beganAbove ? "began_above" : inProgress ? "in_progress" : null,
  };
}
