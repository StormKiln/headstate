import { describe, expect, it } from "vitest";
import type { TranscriptBlock, TranscriptMessage } from "../../../types/transcript";
import { call, output } from "../fixtures";
import { callDuration, thinkingStarts, thoughtLabel } from "./timing";

function msg(id: string, timestamp: string | null, blocks: TranscriptBlock[]): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: null,
    kind: { kind: "assistant" },
    timestamp,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks,
  };
}

const THINK: TranscriptBlock = { kind: "thinking", index: 0, text: "hm", clip: null, recorded: true };
const BASH = { tool: "bash", command: "ls", description: null, truncated: false } as const;

describe("callDuration", () => {
  it("is the call's time to its result record's time, or null", () => {
    const c = call("Bash", BASH, output({ timestamp: "2026-01-01T00:00:03Z" }));
    expect(callDuration(c, "2026-01-01T00:00:01Z")).toBe(2000);
    expect(callDuration(c, null)).toBeNull();
    expect(callDuration(call("Bash", BASH, output({ timestamp: null })), "2026-01-01T00:00:01Z")).toBeNull();
    expect(callDuration(call("Bash", BASH, null), "2026-01-01T00:00:01Z")).toBeNull();
  });
});

describe("thinkingStarts", () => {
  it("starts thinking after a merged result, not at the call that made it", () => {
    // The tool ran for 50 s; the result record was absorbed into the
    // call. Timing the thinking from the call would add those 50 s.
    const messages = [
      msg("a", "2026-01-01T00:00:00Z", [call("Bash", BASH, output({ timestamp: "2026-01-01T00:00:50Z" }))]),
      msg("b", "2026-01-01T00:00:56Z", [THINK]),
    ];
    expect(thinkingStarts(messages).get("b")).toBe("2026-01-01T00:00:50Z");
  });

  it("has no start for thinking with nothing recorded before it", () => {
    expect(thinkingStarts([msg("b", "2026-01-01T00:00:56Z", [THINK])]).has("b")).toBe(false);
    expect(
      thinkingStarts([msg("a", null, []), msg("b", "2026-01-01T00:00:56Z", [THINK])]).has("b"),
    ).toBe(false);
  });
});

describe("thoughtLabel", () => {
  it("qualifies a measured gap, and says only 'Thought' without one", () => {
    expect(thoughtLabel(14_200)).toBe("Thought for about 14 s");
    expect(thoughtLabel(75_000)).toBe("Thought for about 1 m 15 s");
    expect(thoughtLabel(400)).toBe("Thought for less than a second");
    expect(thoughtLabel(null)).toBe("Thought");
  });
});
