/// #1491: the pending-message model and its reconciliation rule. Generic
/// fixtures only: the privacy guard scans this file.

import { describe, expect, it } from "vitest";
import type { TranscriptMessage } from "../../types/transcript";
import {
  MATCH_SKEW_MS,
  MATCH_WINDOW_MS,
  newPendingMessage,
  type PendingMessage,
  pendingStatus,
  reconcilePending,
} from "./pending";

const SENT_AT = Date.parse("2026-01-01T00:00:00Z");
const at = (ms: number) => new Date(SENT_AT + ms).toISOString();

function record(
  id: string,
  text: string | string[],
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  const texts = Array.isArray(text) ? text : [text];
  return {
    id,
    id_source: "uuid",
    turn_id: id,
    kind: { kind: "user_prompt", origin: null },
    timestamp: at(2_000),
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks: texts.map((t, index) => ({ kind: "text", index, text: t, clip: null })),
    ...over,
  };
}

function sent(text: string, over: Partial<PendingMessage> = {}): PendingMessage {
  return newPendingMessage(text, over.after ?? null, over.createdAt ?? SENT_AT, over.clientId ?? "c1");
}

const reply = (id: string): TranscriptMessage =>
  record(id, "an answer", { kind: { kind: "assistant" }, turn_id: null });

describe("reconcilePending: what matches", () => {
  it("pairs a pending message with the user record that holds its text", () => {
    const p = sent("run the tests");
    const r = reconcilePending([p], [reply("a0"), record("u1", "run the tests")]);
    expect(r.unmatched).toEqual([]);
    expect(r.matched.get("c1")).toBe("u1");
  });

  it("keeps it pending while no record holds its text", () => {
    const p = sent("run the tests");
    const r = reconcilePending([p], [record("u1", "something else")]);
    expect(r.unmatched).toEqual([p]);
    expect(r.matched.size).toBe(0);
  });

  it("normalises line endings and outer whitespace on both sides", () => {
    const p = sent("line one\r\nline two\n");
    expect(reconcilePending([p], [record("u1", "  line one\nline two")]).matched.get("c1")).toBe(
      "u1",
    );
  });

  it("joins a record's text blocks with a new line", () => {
    const p = sent("first\nsecond");
    expect(reconcilePending([p], [record("u1", ["first", "second"])]).matched.get("c1")).toBe("u1");
  });

  it("takes a queued prompt: a message sent while Claude is busy", () => {
    const p = sent("and then this");
    const q = record("q1", "and then this", { kind: { kind: "queued_prompt", mode: null } });
    expect(reconcilePending([p], [q]).matched.get("c1")).toBe("q1");
  });

  it("matches whatever the pending message's state: the transcript is what happened", () => {
    for (const state of ["pending", "delivered", "failed", "unconfirmed"] as const) {
      const p = { ...sent("hello"), state };
      expect(reconcilePending([p], [record("u1", "hello")]).unmatched).toEqual([]);
    }
  });
});

describe("reconcilePending: what does not match", () => {
  it("never takes a record that is not the user speaking", () => {
    const p = sent("hello");
    const cases: TranscriptMessage[] = [
      record("a", "hello", { kind: { kind: "assistant" } }),
      record("m", "hello", { is_meta: true }),
      record("s", "hello", { is_sidechain: true }),
      record("i", "hello", { kind: { kind: "injected", origin: null } }),
    ];
    for (const m of cases) expect(reconcilePending([p], [m]).unmatched, m.id).toEqual([p]);
  });

  it("never takes a record from before the send, even with the same text", () => {
    // The user typed "continue" earlier; the phone sends "continue" again.
    const p = sent("continue", { after: "a1" });
    const earlier = record("u0", "continue");
    const r = reconcilePending([p], [earlier, reply("a1")]);
    expect(r.unmatched).toEqual([p]);
    // ...and takes the new one when it arrives after the send.
    const later = record("u2", "continue");
    expect(reconcilePending([p], [earlier, reply("a1"), later]).matched.get("c1")).toBe("u2");
  });

  it("falls back to the time window when the message it was sent after is no longer held", () => {
    const p = sent("continue", { after: "gone" });
    expect(reconcilePending([p], [record("u1", "continue")]).matched.get("c1")).toBe("u1");
  });

  it("holds a record to the time window around the send", () => {
    const p = sent("hello");
    const inside = [-MATCH_SKEW_MS, 0, MATCH_WINDOW_MS];
    const outside = [-MATCH_SKEW_MS - 1, MATCH_WINDOW_MS + 1];
    for (const ms of inside) {
      expect(reconcilePending([p], [record("u", "hello", { timestamp: at(ms) })]).unmatched, `${ms}`).toEqual([]);
    }
    for (const ms of outside) {
      expect(reconcilePending([p], [record("u", "hello", { timestamp: at(ms) })]).unmatched, `${ms}`).toEqual([p]);
    }
  });

  it("never matches a record it cannot place in time", () => {
    const p = sent("hello");
    for (const timestamp of [null, "not a time"]) {
      expect(reconcilePending([p], [record("u", "hello", { timestamp })]).unmatched).toEqual([p]);
    }
  });

  it("never matches a record with no text", () => {
    const p = sent("hello");
    expect(reconcilePending([p], [record("u", [])]).unmatched).toEqual([p]);
  });
});

describe("reconcilePending: pairing", () => {
  it("pairs two identical sends with two records, oldest to oldest, one each", () => {
    const first = sent("yes", { clientId: "c1", createdAt: SENT_AT });
    const second = sent("yes", { clientId: "c2", createdAt: SENT_AT + 5_000 });
    // Given newest first, still paired by age.
    const one = record("u1", "yes", { timestamp: at(1_000) });
    let r = reconcilePending([second, first], [one]);
    expect(r.matched.get("c1")).toBe("u1");
    expect(r.unmatched).toEqual([second]);

    const two = record("u2", "yes", { timestamp: at(6_000) });
    r = reconcilePending([second, first], [one, two]);
    expect(r.matched.get("c1")).toBe("u1");
    expect(r.matched.get("c2")).toBe("u2");
    expect(r.unmatched).toEqual([]);
  });

  it("returns the unmatched in the order it was given them", () => {
    const a = sent("a", { clientId: "a", createdAt: SENT_AT + 1 });
    const b = sent("b", { clientId: "b", createdAt: SENT_AT });
    expect(reconcilePending([a, b], []).unmatched).toEqual([a, b]);
  });
});

describe("reconcilePending: text the phone was not shown whole", () => {
  it("treats a span the desktop masked as any non-empty run", () => {
    const p = sent("use token abc123 please");
    const masked = record("u1", "use token ⟦hidden:token⟧ please");
    expect(reconcilePending([p], [masked]).matched.get("c1")).toBe("u1");
  });

  it("still holds the unmasked text around it exactly", () => {
    const p = sent("use token abc123 now");
    const masked = record("u1", "use token ⟦hidden:token⟧ please");
    expect(reconcilePending([p], [masked]).unmatched).toEqual([p]);
  });

  it("reads the masked text's own characters literally, not as a pattern", () => {
    const p = sent("a.b ⟦x");
    const masked = record("u1", "a+b ⟦hidden:secret⟧");
    expect(reconcilePending([sent("aab xyz")], [masked]).unmatched).toHaveLength(1);
    expect(reconcilePending([p], [masked]).unmatched).toEqual([p]);
  });

  it("matches a clipped block as a prefix, and only as one", () => {
    const long = "x".repeat(50) + " and the rest";
    const clipped = record("u1", "x".repeat(50), {
      blocks: [{ kind: "text", index: 0, text: "x".repeat(50), clip: { shown_chars: 50, total_chars: 63 } }],
    });
    expect(reconcilePending([sent(long)], [clipped]).unmatched).toEqual([]);
    expect(reconcilePending([sent("y".repeat(60))], [clipped]).unmatched).toHaveLength(1);
  });
});

describe("pendingStatus", () => {
  it("says what is known, and never calls a timeout a failure", () => {
    const p = sent("hello");
    expect(pendingStatus(p)).toEqual({ text: "Sending…", tone: "muted" });
    expect(pendingStatus({ ...p, state: "delivered" }).tone).toBe("muted");
    const unconfirmed = pendingStatus({ ...p, state: "unconfirmed", reason: "no answer in 30 s" });
    expect(unconfirmed.tone).toBe("warn");
    expect(unconfirmed.text).toMatch(/^Not confirmed/);
    expect(unconfirmed.text).toContain("no answer in 30 s");
    expect(unconfirmed.text).toContain("before sending it again");
    expect(unconfirmed.text).not.toMatch(/not sent|failed/i);
    expect(pendingStatus({ ...p, state: "failed", reason: "the session has ended" })).toEqual({
      text: "Not sent: the session has ended.",
      tone: "error",
    });
  });
});

describe("newPendingMessage", () => {
  it("starts pending, with a fresh client id each time", () => {
    const a = newPendingMessage("hello", "m1");
    const b = newPendingMessage("hello", "m1");
    expect(a.state).toBe("pending");
    expect(a.after).toBe("m1");
    expect(a.reason).toBeNull();
    expect(a.clientId).not.toBe(b.clientId);
  });
});
