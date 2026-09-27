/// #1484: what the transcript shows, and moving between turns. Generic
/// fixtures only.

import { describe, expect, it } from "vitest";
import type { TranscriptMessage } from "../../types/transcript";
import { call, output } from "./fixtures";
import { applyShow, SHOW_ALL, showFrom, shownAnchor } from "./filters";
import { adjacentOpener, loadedTurns, rowFor } from "./turnNav";

function msg(id: string, over: Partial<TranscriptMessage> = {}): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: "u1",
    kind: { kind: "assistant" },
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    blocks: [{ kind: "text", index: 0, text: id, clip: null }],
    offset: null,
    oversized_bytes: null,
    ...over,
  };
}

const thinking = { kind: "thinking" as const, index: 1, text: "hmm", clip: null, recorded: true };
const ls = call("Bash", { tool: "bash", command: "ls", description: null, truncated: false }, output());

const messages: TranscriptMessage[] = [
  msg("u1", { turn_id: "u1", kind: { kind: "user_prompt", origin: null } }),
  msg("a1", { blocks: [thinking, { kind: "text", index: 2, text: "ok", clip: null }] }),
  msg("a2", { blocks: [thinking] }),
  msg("a3", { blocks: [ls] }),
  msg("h1", { kind: { kind: "hook_output", event: "Stop", name: null, outcome: "success", exit_code: 0, prevented_continuation: null } }),
  msg("m1", { is_meta: true, kind: { kind: "user_prompt", origin: null } }),
  msg("s1", { is_sidechain: true }),
  msg("t1", { kind: { kind: "turn_duration", message_count: null }, blocks: [] }),
];

describe("what is shown", () => {
  it("shows everything by default, as the same list", () => {
    expect(showFrom(undefined)).toEqual(SHOW_ALL);
    expect(showFrom({ thinking: false })).toEqual({ ...SHOW_ALL, thinking: false });
    const out = applyShow(messages, SHOW_ALL);
    expect(out.messages).toBe(messages);
    expect(out.hidden).toBe(0);
  });

  it("hides thinking: the block, and a message that held only thinking", () => {
    const out = applyShow(messages, { ...SHOW_ALL, thinking: false });
    expect(out.messages.map((m) => m.id)).not.toContain("a2");
    const a1 = out.messages.find((m) => m.id === "a1")!;
    expect(a1.blocks.map((b) => b.kind)).toEqual(["text"]);
    expect(out.hidden).toBe(2);
    // The input is not changed.
    expect(messages[1].blocks).toHaveLength(2);
  });

  it("hides tool calls, system and meta records, and sidechains, each on its own", () => {
    const ids = (s: Partial<typeof SHOW_ALL>) =>
      applyShow(messages, { ...SHOW_ALL, ...s }).messages.map((m) => m.id);
    expect(ids({ tools: false })).not.toContain("a3");
    expect(ids({ system: false })).toEqual(["u1", "a1", "a2", "a3", "s1", "t1"]);
    expect(ids({ sidechains: false })).not.toContain("s1");
    // A turn's duration carries its footer: never a "system" record.
    expect(ids({ system: false })).toContain("t1");
  });

  it("lands a jump to a hidden message on the nearest shown one before it", () => {
    const shown = applyShow(messages, { ...SHOW_ALL, thinking: false }).messages;
    expect(shownAnchor(messages, shown, "a1")).toBe("a1");
    expect(shownAnchor(messages, shown, "a2")).toBe("a1");
    expect(shownAnchor(messages, [], "a2")).toBeNull();
  });
});

describe("turns", () => {
  const convo = [
    msg("u1", { turn_id: "u1", kind: { kind: "user_prompt", origin: null } }),
    msg("a1", { turn_id: "u1" }),
    msg("m1", { turn_id: "m1", is_meta: true, kind: { kind: "user_prompt", origin: null } }),
    msg("u2", { turn_id: "u2", kind: { kind: "slash_command", name: "/review" } }),
    msg("a2", {
      turn_id: "u2",
      blocks: [{ ...ls, result: output({ message_id: "r9" }) }],
    }),
    msg("u3", { turn_id: "u3", kind: { kind: "shell_input", command: "ls" } }),
  ];

  it("steps between prompts the reader typed, skipping meta prompts", () => {
    expect(adjacentOpener(convo, "a2", -1)?.id).toBe("u2");
    expect(adjacentOpener(convo, "u2", -1)?.id).toBe("u1");
    expect(adjacentOpener(convo, "a1", 1)?.id).toBe("u2");
    expect(adjacentOpener(convo, "u3", 1)).toBeNull();
    expect(adjacentOpener(convo, null, -1)?.id).toBe("u3");
  });

  it("finds the row of a result the merge absorbed into its call", () => {
    expect(rowFor(convo, "r9")).toBe("a2");
    expect(rowFor(convo, "a1")).toBe("a1");
    expect(rowFor(convo, "nope")).toBeNull();
  });

  it("groups the loaded turns by opener", () => {
    expect(loadedTurns(convo).map((t) => [t.opener.id, t.messages.length])).toEqual([
      ["u1", 3],
      ["u2", 2],
      ["u3", 1],
    ]);
  });
});
