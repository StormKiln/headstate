/// #1480: the per-turn footer's figures. Generic fixtures only.

import { describe, expect, it } from "vitest";
import type { TranscriptMessage, TranscriptUsage } from "../../types/transcript";
import { turnFooters } from "./turnFooter";

function msg(
  id: string,
  turn: string | null,
  kind: TranscriptMessage["kind"],
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: turn,
    kind,
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks: [],
    ...over,
  };
}

const usage = (output: number | null): TranscriptUsage => ({
  input_tokens: 10,
  output_tokens: output,
  cache_creation_input_tokens: null,
  cache_read_input_tokens: null,
});

const PROMPT = { kind: "user_prompt", origin: null } as const;
const ASSISTANT = { kind: "assistant" } as const;

describe("turnFooters", () => {
  it("sums output tokens once per API response, and hangs the footer on the turn's last reply", () => {
    const f = turnFooters(
      [
        msg("u1", "u1", PROMPT, { timestamp: "2026-01-01T00:00:00Z" }),
        // One API response written as two records repeating its usage.
        msg("a1", "u1", ASSISTANT, { api_message_id: "r1", usage: usage(40), model: "model-a" }),
        msg("a2", "u1", ASSISTANT, { api_message_id: "r1", usage: usage(40), model: "model-a" }),
        msg("a3", "u1", ASSISTANT, {
          api_message_id: "r2",
          usage: usage(2),
          model: "model-a",
          timestamp: "2026-01-01T00:00:05Z",
        }),
      ],
      false,
    );
    expect([...f.keys()]).toEqual(["a3"]);
    expect(f.get("a3")).toEqual({
      outputTokens: 42,
      tokensPartial: false,
      durationMs: 5000,
      durationSource: "timestamps",
      models: ["model-a"],
      qualifier: null,
    });
  });

  it("prefers the recorded turn duration, and the record hosts the footer", () => {
    const f = turnFooters(
      [
        msg("u1", "u1", PROMPT, { timestamp: "2026-01-01T00:00:00Z" }),
        msg("a1", "u1", ASSISTANT, { usage: usage(1), timestamp: "2026-01-01T00:00:09Z" }),
        msg("d1", "u1", { kind: "turn_duration", message_count: 2 }, { duration_ms: 63_000 }),
      ],
      false,
    );
    expect(f.has("a1")).toBe(false);
    expect(f.get("d1")).toMatchObject({ durationMs: 63_000, durationSource: "recorded" });
  });

  /// Absent is not zero: a response that did not record its tokens makes
  /// the sum a floor, and a turn where none did has no token figure.
  it("qualifies a sum missing a response's figure, and gives none when no response had one", () => {
    const partial = turnFooters(
      [
        msg("u1", "u1", PROMPT),
        msg("a1", "u1", ASSISTANT, { api_message_id: "r1", usage: usage(5) }),
        msg("a2", "u1", ASSISTANT, { api_message_id: "r2", usage: null }),
      ],
      false,
    ).get("a2");
    expect(partial).toMatchObject({ outputTokens: 5, tokensPartial: true });

    const none = turnFooters(
      [msg("u1", "u1", PROMPT), msg("a1", "u1", ASSISTANT)],
      false,
    ).get("a1");
    expect(none?.outputTokens).toBeNull();
    // And no timestamps: no duration, never "0 ms".
    expect(none?.durationMs).toBeNull();
  });

  it("marks a turn that began above the read, and does not time it from its first loaded message", () => {
    const f = turnFooters(
      [
        msg("a0", null, ASSISTANT, { usage: usage(3), timestamp: "2026-01-01T00:00:00Z" }),
        msg("a1", null, ASSISTANT, { usage: usage(4), timestamp: "2026-01-01T00:01:00Z" }),
        msg("u2", "u2", PROMPT),
      ],
      false,
    );
    expect(f.get("a1")).toMatchObject({
      outputTokens: 7,
      tokensPartial: true,
      durationMs: null,
      qualifier: "began_above",
    });
  });

  it("marks only the newest turn of a live session as still being written", () => {
    const f = turnFooters(
      [
        msg("u1", "u1", PROMPT),
        msg("a1", "u1", ASSISTANT, { usage: usage(1) }),
        msg("u2", "u2", PROMPT),
        msg("a2", "u2", ASSISTANT, { usage: usage(1) }),
      ],
      true,
    );
    expect(f.get("a1")?.qualifier).toBeNull();
    expect(f.get("a2")).toMatchObject({ qualifier: "in_progress", tokensPartial: true });
  });

  it("lists every model that answered, and skips Claude Code's own synthetic records", () => {
    const f = turnFooters(
      [
        msg("u1", "u1", PROMPT),
        msg("a1", "u1", ASSISTANT, { model: "model-a", usage: usage(1) }),
        msg("a2", "u1", ASSISTANT, { model: "<synthetic>", usage: usage(99) }),
        msg("a3", "u1", ASSISTANT, { model: "model-b", usage: usage(1), api_message_id: "x" }),
      ],
      false,
    );
    expect(f.get("a3")).toMatchObject({ models: ["model-a", "model-b"], outputTokens: 2 });
  });

  it("gives a turn with no reply no footer", () => {
    expect(turnFooters([msg("u1", "u1", PROMPT)], false).size).toBe(0);
  });
});
