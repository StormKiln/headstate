/// Local pending messages (#1491, #1490 constraint 2): a user message
/// this app is sending, shown before the transcript holds it.
///
/// Nothing in 7.9 creates one -- there is no send path, and the composer
/// is behind `COMPOSER_ENABLED` -- but the model, both renderers and the
/// reconciliation exist so that 7.10 adds a command, not a redesign.
/// `docs/transcript-send-readiness.md` has the whole design.
///
/// # States
///
/// | state | means | set by |
/// |---|---|---|
/// | `pending` | the send is in flight | creation |
/// | `delivered` | the desktop said it handed the text to the session | the send's success |
/// | `failed` | the desktop said it did NOT hand the text over | a definite refusal only |
/// | `unconfirmed` | no answer either way: it may or may not have arrived | a timeout, a dropped connection |
///
/// A timeout is `unconfirmed`, never `failed` (#1466): the command may
/// have run and only its answer been lost. Offering "send again" on a
/// timeout is how a message gets sent twice; `unconfirmed` says to
/// check the session first.
///
/// A pending message leaves the list when the transcript's own record of
/// it appears -- whatever its state, because the transcript is what
/// happened. Nothing here expires one: "not seen yet" is only a fact
/// while the transcript is still being read, and deciding when to give
/// up belongs to 7.10's send path, which knows whether it is.
///
/// # The reconciliation rule
///
/// `reconcilePending` pairs each pending message with at most one
/// transcript record, and each record with at most one pending message.
/// A record can answer a pending message only if ALL of these hold:
///
/// 1. It is the user speaking: a `user_prompt` that is not `is_meta`, or
///    a `queued_prompt` (a message sent while Claude is busy is recorded
///    as queued). Never a sidechain record.
/// 2. It comes after `after` -- the newest message held when the send
///    began -- when that message is still held. The transcript is
///    append-only, so this rules out an older, identical prompt. When
///    `after` is no longer held (the window moved), only the time rule
///    places it.
/// 3. Its recorded timestamp is within `[createdAt - MATCH_SKEW_MS,
///    createdAt + MATCH_WINDOW_MS]`. A record with no timestamp, or one
///    that does not parse, cannot be placed in time and never matches:
///    a wrong match would hide a message that was never delivered.
/// 4. Its text is the sent text. Both sides have `\r\n` normalised and
///    outer whitespace trimmed; the record's text blocks are joined with
///    `\n`. A span the desktop masked before the text reached a phone
///    (#1488) matches any non-empty run, since the phone cannot see what
///    it hid; a clipped block (`clip` set) matches as a prefix.
///
/// Pending messages are paired oldest first, each to the EARLIEST record
/// that qualifies, so sending "yes" twice pairs the first send with the
/// first "yes" and the second with the second.

import { MARKER_OPEN, splitMasked } from "@/lib/masked";
import type { TranscriptMessage } from "../../types/transcript";

export type PendingState = "pending" | "delivered" | "failed" | "unconfirmed";

export interface PendingMessage {
  /// Made on this device when the message is created, and sent with it
  /// as the idempotency key: a retry carries the same id, so the desktop
  /// can refuse to deliver it twice.
  clientId: string;
  /// Exactly what was sent. Never masked: masking (#1488) applies to
  /// text leaving the desktop for display, not to what the user types.
  text: string;
  /// Milliseconds since the epoch, on this device's clock.
  createdAt: number;
  state: PendingState;
  /// The id of the newest transcript message held when the send began;
  /// `null` when none was held. See rule 2.
  after: string | null;
  /// Why it failed, or why it is unconfirmed, as the send path says it.
  /// `null` when there is nothing to add.
  reason: string | null;
}

/// How far BEFORE the send a record's timestamp may be and still match:
/// the phone's and the desktop's clocks are not the same clock.
export const MATCH_SKEW_MS = 30_000;
/// How far AFTER the send a record's timestamp may be and still match. A
/// message sent while Claude is busy is written when it is taken up.
export const MATCH_WINDOW_MS = 10 * 60_000;

/// The item id a pending message is scrolled and anchored by. Cannot
/// collide with a record's: record ids are uuids or derived from them.
export function pendingItemId(clientId: string): string {
  return `pending:${clientId}`;
}

export function newPendingMessage(
  text: string,
  after: string | null,
  now: number = Date.now(),
  clientId: string = crypto.randomUUID(),
): PendingMessage {
  return { clientId, text, createdAt: now, state: "pending", after, reason: null };
}

/// What both renderers say under a pending message, and in what tone.
///
/// `unconfirmed` tells the reader what to do BEFORE retrying: the
/// message may already be in the session, and sending it again could
/// send it twice.
export function pendingStatus(p: PendingMessage): { text: string; tone: "muted" | "warn" | "error" } {
  switch (p.state) {
    case "pending":
      return { text: "Sending…", tone: "muted" };
    case "delivered":
      return { text: "Sent. Not in the transcript yet.", tone: "muted" };
    case "unconfirmed":
      return {
        text: `Not confirmed: it may or may not have reached the session${p.reason ? ` (${p.reason})` : ""}. Check the session before sending it again.`,
        tone: "warn",
      };
    case "failed":
      return { text: `Not sent${p.reason ? `: ${p.reason}` : ""}.`, tone: "error" };
  }
}

export interface Reconciled {
  /// Still waiting for their record, in the order given.
  unmatched: PendingMessage[];
  /// `clientId` to the id of the record that answered it.
  matched: Map<string, string>;
}

export function reconcilePending(
  pending: readonly PendingMessage[],
  messages: readonly TranscriptMessage[],
): Reconciled {
  const matched = new Map<string, string>();
  if (pending.length === 0) return { unmatched: [], matched };

  const index = new Map<string, number>();
  messages.forEach((m, i) => index.set(m.id, i));
  const claimed = new Set<number>();

  const byAge = [...pending].sort((a, b) => a.createdAt - b.createdAt);
  for (const p of byAge) {
    const from = p.after === null ? 0 : (index.get(p.after) ?? -1) + 1;
    const want = normalise(p.text);
    for (let i = from; i < messages.length; i++) {
      if (claimed.has(i)) continue;
      const m = messages[i];
      if (!isUserSpeaking(m) || !inWindow(m.timestamp, p.createdAt)) continue;
      if (!textMatches(m, want)) continue;
      claimed.add(i);
      matched.set(p.clientId, m.id);
      break;
    }
  }
  return { unmatched: pending.filter((p) => !matched.has(p.clientId)), matched };
}

function isUserSpeaking(m: TranscriptMessage): boolean {
  if (m.is_sidechain) return false;
  return (m.kind.kind === "user_prompt" && !m.is_meta) || m.kind.kind === "queued_prompt";
}

function inWindow(timestamp: string | null, createdAt: number): boolean {
  if (timestamp === null) return false;
  const at = Date.parse(timestamp);
  if (Number.isNaN(at)) return false;
  return at >= createdAt - MATCH_SKEW_MS && at <= createdAt + MATCH_WINDOW_MS;
}

function normalise(text: string): string {
  return text.replace(/\r\n/g, "\n").trim();
}

function textMatches(m: TranscriptMessage, want: string): boolean {
  const texts = m.blocks.flatMap((b) => (b.kind === "text" ? [b] : []));
  if (texts.length === 0) return false;
  const got = normalise(texts.map((b) => b.text).join("\n"));
  const clipped = texts.some((b) => b.clip !== null);
  if (!got.includes(MARKER_OPEN)) {
    return clipped ? got.length > 0 && want.startsWith(got) : got === want;
  }
  // Masked: literal runs must match exactly and in order; each hidden
  // span stands for at least one character.
  const pattern = splitMasked(got)
    .map((part) => ("text" in part ? escapeRegExp(part.text) : "[\\s\\S]+?"))
    .join("");
  return new RegExp(`^${pattern}${clipped ? "" : "$"}`).test(want);
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
