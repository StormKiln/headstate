import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import { everyRecord } from "../components/transcript/fixtures";
import type { Worktree } from "../types/pr";
import type { TranscriptWindow } from "../types/transcript";
import { wireGraph } from "./wireContract.generated";
import { assertRemoteReply, remoteEventError } from "./wireContract";
import { validateWire } from "./wireValidator";

const tree: Worktree = {
  path: "/tmp/demo", branch: "feature", head: "abc", size_bytes: null,
  safety: { kind: "pending" }, is_main: false, merged_at: null, upstream: null, last_commit: null,
};
function window(messages = everyRecord()): TranscriptWindow {
  return {
    page: { messages, truncated: false, bytes_read: 1000, file_bytes: 1000, machinery_records: [], unparseable_records: 0, duplicate_records: 0 },
    start: { offset: 0, behind_digest: "start" }, end: { offset: 1000, behind_digest: "end" },
    at_start: true, at_end: true, rewritten: false,
    position: { first: 1, last: messages.length, total: messages.length, exact: true, basis: "whole_file" },
    seam: { first_model: null, last_model: null }, bytes_scanned: 0,
  };
}

describe("generated remote contracts", () => {
  it("checks every existing transcript record/tool union and nested failure", () => {
    const value = window();
    expect(() => assertRemoteReply("claude_transcript_page", value)).not.toThrow();
    const malformed = { ...value, page: { ...value.page, messages: [{ ...value.page.messages[0], blocks: [{ kind: "future-kind", secret: "DO-NOT-LOG" }] }] } };
    expect(() => assertRemoteReply("claude_transcript_page", malformed)).toThrow(/claude_transcript_page.*page.messages\[\].blocks\[\]/);
    try { assertRemoteReply("claude_transcript_page", malformed); }
    catch (e) { expect(String(e)).not.toContain("DO-NOT-LOG"); }
  });

  it("checks tuple length and finite numbers, retaining optional older worktree data", () => {
    expect(remoteEventError("worktree-safety", tree)).toBeNull();
    expect(remoteEventError("worktree-safety", { ...tree, submodules: "wrong" })).toContain("submodules");
    expect(remoteEventError("worktree-size", ["/tmp/demo", null])).toBeNull();
    for (const tuple of [["/tmp/demo"], ["/tmp/demo", 2, 3], ["/tmp/demo", NaN]]) {
      expect(remoteEventError("worktree-size", tuple)).not.toBeNull();
    }
    expect(remoteEventError("worktree-removal-progress", { run: 1, removed: true, done: 1, total: 2 })).toBeNull();
  });

  it("rejects inherited required properties and prototype names as commands", () => {
    expect(remoteEventError("claude-session-activity", Object.create({ session_id: "a", size: 2, seq: 1 }))).not.toBeNull();
    for (const command of ["__proto__", "constructor", "toString"]) {
      expect(() => assertRemoteReply(command, {})).toThrow(/no remote response contract/);
    }
  });
});

// Measured validation only: construction/stringification are outside timing.
// Independent objects approximate JSON.parse output; repeated shared references
// would give the cache an unrealistic advantage. No wall-clock threshold: CI
// varies and deterministic work/depth budgets provide the actual safety guard.
it("measures large synthetic replies and leaves operation-budget headroom", () => {
  const records = everyRecord();
  const cases = [
    ["get_cached", Array.from({ length: 10_000 }, (_, i) => structuredClone(PR_FIXTURES[i % PR_FIXTURES.length]))],
    ["classify_worktrees", Array.from({ length: 10_000 }, () => structuredClone(tree))],
    ["claude_transcript_page", window(Array.from({ length: 10_000 }, (_, i) => structuredClone(records[i % records.length])))],
  ] as const;
  for (const [name, value] of cases) {
    const bytes = new TextEncoder().encode(JSON.stringify(value)).length;
    const times: number[] = [];
    let operations = 0;
    for (let i = 0; i < 4; i++) {
      const started = performance.now();
      const result = validateWire(wireGraph.nodes, wireGraph.commands[name], value);
      const ms = performance.now() - started;
      expect(result.field).toBeNull();
      operations = result.operations;
      if (i > 0) times.push(ms);
    }
    expect(operations).toBeLessThan(2_500_000); // At least 4x budget headroom.
    console.info(`wire-benchmark ${name}: ${bytes} bytes, ${operations} operations, median ${times.sort((a, b) => a - b)[1].toFixed(2)} ms (synthetic host, 10000 rows/messages)`);
  }
});


it("preserves legacy waiting payloads and validates optional permission context", () => {
  const base = {
    session_id: "fixture", claude_version: null, transcript_path: null, first_seen_at: "fixture-time",
    liveness: { state: "unknown", why: "fixture" }, transcript_state: { state: "exists" },
    resume: { command: "fixture", caveat: null, anchored: true }, runs: 0,
    registry_failure: null, kind: { kind: "own" }, subagents: [], parent: null, unattributed: null,
    compactions: null, agent_types: null,
  };
  for (const state of ["now", "last-seen"]) {
    const waiting = { state, kind: "permission_prompt", at: "fixture-time", ...(state === "last-seen" ? { why: "fixture" } : {}) };
    for (const context of [{}, { tool: null, summary: null }, { tool: "Bash", summary: "echo safe" }]) {
      expect(() => assertRemoteReply("claude_session_detail", { ...base, waiting: { ...waiting, ...context } })).not.toThrow();
    }
    for (const context of [{ tool: 123 }, { summary: [] }]) {
      expect(() => assertRemoteReply("claude_session_detail", { ...base, waiting: { ...waiting, ...context } })).toThrow();
    }
  }
});
