/// #1484: the "since you left" marker and the "while you were away"
/// summary. Generic fixtures only.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ClaudeToolArgs } from "../../types/pr";
import type { TranscriptMessage } from "../../types/transcript";
import { call, output, RECORDED_CHANGE } from "./fixtures";
import { advanceMarker, awaySummary, isTestCommand, readMarker, unreadFrom } from "./sinceYouLeft";
import { deriveTaskChecklist } from "./tasks";

const PATH = "/tmp/projects/p/session.jsonl";

function msg(
  id: string,
  offset: number | null,
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: id,
    kind: { kind: "assistant" },
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    blocks: [],
    offset,
    oversized_bytes: null,
    ...over,
  };
}

const prompt = (id: string, offset: number) =>
  msg(id, offset, { turn_id: id, kind: { kind: "user_prompt", origin: null } });
const reply = (id: string, offset: number, turn: string, blocks: TranscriptMessage["blocks"] = []) =>
  msg(id, offset, { turn_id: turn, blocks });
const bash = (command: string, is_error: boolean | null) =>
  call("Bash", { tool: "bash", command, description: null, truncated: false }, output({ is_error }));

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("the marker", () => {
  it("persists and restores per transcript", () => {
    expect(readMarker(PATH)).toBeNull();
    expect(advanceMarker(PATH, reply("a1", 500, "u1"))).toBe(true);
    expect(readMarker(PATH)).toEqual({ id: "a1", offset: 500 });
    expect(readMarker("/tmp/projects/p/other.jsonl")).toBeNull();
  });

  it("only moves forward, and never on a message without an offset", () => {
    advanceMarker(PATH, reply("a2", 900, "u1"));
    expect(advanceMarker(PATH, reply("a1", 500, "u1"))).toBe(false);
    expect(advanceMarker(PATH, msg("x/model", null))).toBe(false);
    expect(readMarker(PATH)?.id).toBe("a2");
  });

  it("storage that throws costs the marker and nothing else", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(readMarker(PATH)).toBeNull();
    expect(advanceMarker(PATH, reply("a1", 500, "u1"))).toBe(false);
  });

  it("a stored value that is not a marker reads as none", () => {
    localStorage.setItem(`headstate.transcript.read:${PATH}`, "{not json");
    expect(readMarker(PATH)).toBeNull();
    localStorage.setItem(`headstate.transcript.read:${PATH}`, JSON.stringify({ id: 3 }));
    expect(readMarker(PATH)).toBeNull();
  });
});

describe("where the unread messages start", () => {
  const held = [prompt("u1", 0), reply("a1", 100, "u1"), prompt("u2", 200), reply("a2", 300, "u2")];

  it("just after the marker's own message when it is held", () => {
    expect(unreadFrom(held, { id: "a1", offset: 100 }, true)).toEqual({ index: 2, placed: true });
  });

  it("by offset when the marker's message is not held but its place is", () => {
    expect(unreadFrom(held, { id: "gone", offset: 150 }, true)).toEqual({ index: 2, placed: true });
  });

  it("unplaced when the marker is older than everything held and more is older still", () => {
    expect(unreadFrom(held.slice(2), { id: "a1", offset: 100 }, true).placed).toBe(false);
    expect(unreadFrom(held.slice(2), { id: "a1", offset: 100 }, false).placed).toBe(true);
  });
});

describe("the summary", () => {
  const edit: ClaudeToolArgs = {
    tool: "edit",
    file_path: "src/example.rs",
    old_string: "a",
    new_string: "b",
    replace_all: false,
  } as ClaudeToolArgs;
  const held = [
    prompt("u1", 0),
    reply("a1", 100, "u1"),
    // Unread from here.
    prompt("u2", 200),
    reply("a2", 300, "u2", [
      call("Edit", edit, output({ change: RECORDED_CHANGE })),
      bash("yarn vitest run", true),
    ]),
    prompt("u3", 400),
    reply("a3", 500, "u3", [
      call("Edit", edit, output({ change: RECORDED_CHANGE, tool_use_id: "toolu_2" }), "toolu_2"),
      call("Read", { tool: "read", file_path: "src/x.rs", offset: null, limit: null }, output()),
      bash("cargo test --lib", false),
    ]),
  ];

  it("counts turns, calls, files, lines and test runs after the marker", () => {
    const s = awaySummary(held, { id: "a1", offset: 100 }, { hasOlder: false });
    expect(s).not.toBeNull();
    expect(s!.turns).toBe(2);
    expect(s!.toolCalls).toBe(5);
    // RECORDED_CHANGE is +2 −1; the same file twice is one file.
    expect(s!.files).toEqual({ count: 1, added: 4, removed: 2, partial: false });
    expect(s!.tests).toEqual({ runs: 2, last: "last passed" });
    expect(s!.atLeast).toBe(false);
    expect(s!.text).toBe(
      "2 turns · 5 tool calls · 1 file edited (+4 −2) · tests: 2 runs, last passed",
    );
  });

  it("says 'at least' when the marker is in pages that are not loaded", () => {
    const s = awaySummary(held.slice(2), { id: "a1", offset: 100 }, { hasOlder: true });
    expect(s!.atLeast).toBe(true);
    expect(s!.text).toMatch(/^at least 2 turns · at least 5 tool calls · at least 1 file edited/);
    expect(s!.text).toContain("tests: at least 2 runs, last passed");
  });

  it("adds the task checklist's summary and a live wait", () => {
    const tasks = deriveTaskChecklist(held, { truncated: false });
    const s = awaySummary(held, { id: "a1", offset: 100 }, {
      hasOlder: false,
      tasks,
      waiting: { state: "now", kind: "idle_prompt", at: "2026-01-01T00:00:00Z" },
    });
    expect(s!.text.endsWith(" · now waiting for your input")).toBe(true);
    // A past wait is not "now".
    const past = awaySummary(held, { id: "a1", offset: 100 }, {
      hasOlder: false,
      waiting: { state: "last-seen", kind: "idle_prompt", at: "2026-01-01T00:00:00Z", why: "x" },
    });
    expect(past!.text).not.toContain("waiting");
  });

  it("is nothing when nothing is new", () => {
    expect(awaySummary(held, { id: "a3", offset: 500 }, { hasOlder: false })).toBeNull();
  });

  it("a refused edit is not an edit, and an unanswered test run says so", () => {
    const s = awaySummary(
      [
        prompt("u1", 0),
        reply("a1", 100, "u1", [
          call("Edit", edit, output({ is_error: true, change: RECORDED_CHANGE })),
          call("Bash", { tool: "bash", command: "make test", description: null, truncated: false }, null),
        ]),
      ],
      { id: "zero", offset: -1 },
      { hasOlder: false },
    );
    expect(s!.files).toBeNull();
    expect(s!.tests).toEqual({ runs: 1, last: "last has no result yet" });
  });
});

describe("what counts as a test run", () => {
  it.each([
    ["yarn vitest run src", true],
    ["cd x && cargo test --lib -- --test-threads=8", true],
    ["npm test", true],
    ["make test-mobile", true],
    ["go test ./...", true],
    ["python -m pytest -q", true],
    ["ls tests/", false],
    ["cat latest.txt", false],
    ["git log --grep test", false],
  ])("%s -> %s", (cmd, want) => {
    expect(isTestCommand(cmd)).toBe(want);
  });
});
