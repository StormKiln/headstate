import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import { everyRecord, output } from "../components/transcript/fixtures";
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
  it("accepts old-peer review gates and checks additive independent native lifetimes", () => {
    const old = { rules: { state: "read", require_last_push_approval: true, required_review_thread_resolution: false }, last_pusher: { state: "not_needed" } };
    expect(() => assertRemoteReply("get_review_gates", old)).not.toThrow();
    expect(() => assertRemoteReply("get_review_gates", { ...old, rules_valid_for_ms: 545000, pusher_valid_for_ms: 0 })).not.toThrow();
    expect(() => assertRemoteReply("get_review_gates", { ...old, rules_valid_for_ms: "new" })).toThrow(/rules_valid_for_ms/);
  });

  it("checks every existing transcript record/tool union and nested failure", () => {
    const value = window();
    expect(() => assertRemoteReply("claude_transcript_page", value)).not.toThrow();
    const malformed = { ...value, page: { ...value.page, messages: [{ ...value.page.messages[0], blocks: [{ kind: "future-kind", secret: "DO-NOT-LOG" }] }] } };
    expect(() => assertRemoteReply("claude_transcript_page", malformed)).toThrow(/claude_transcript_page.*page.messages\[\].blocks\[\]/);
    try { assertRemoteReply("claude_transcript_page", malformed); }
    catch (e) { expect(String(e)).not.toContain("DO-NOT-LOG"); }
  });

  it("accepts older task results and validates optional snapshot metadata", () => {
    const old = output({task:{task_id:"1",success:null,status_from:null,status_to:null}});
    const row = {...everyRecord()[0], kind:{kind:"tool_results" as const}, blocks:[{kind:"tool_result" as const,...old}]};
    expect(() => assertRemoteReply("claude_transcript_page", window([row]))).not.toThrow();
    const snapshots = {items:[{task_id:"1",subject:"Fixture",status:null}],omitted:0,truncated:false};
    row.blocks[0].task = {...old.task!,snapshots};
    expect(() => assertRemoteReply("claude_transcript_page", window([row]))).not.toThrow();
    const malformed = {...row,blocks:[{...row.blocks[0],task:{...old.task,snapshots:{...snapshots,items:"DO-NOT-LOG"}}}]};
    expect(() => assertRemoteReply("claude_transcript_page", window([malformed as unknown as typeof row]))).toThrow(/snapshots/);
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

it("accepts legacy and qualified inventory events but rejects invented readiness evidence", () => {
  expect(remoteEventError("reviewing-updated", PR_FIXTURES)).toBeNull();
  const row = { ...PR_FIXTURES[0], observation: { state: "retained", last_observed_at: null, unknown_fields: [], retained_fields: ["ci"], confirmed_review: { head_oid: "fixture-head", review: "approved", confirmed_at: "2026-10-01T00:00:00Z" } } };
  expect(remoteEventError("reviewing-updated", [row])).toBeNull();
  expect(remoteEventError("reviewing-updated", [{ ...row, observation: { ...row.observation, unknown_fields: ["invented"] } }])).not.toBeNull();
});

it("accepts legacy absent detail qualification and validates positive detail groups", () => {
  const row = { ...PR_FIXTURES[0], observation: { state: "observed", last_observed_at: null, unknown_fields: [], retained_fields: [] } };
  expect(remoteEventError("reviewing-updated", [row])).toBeNull();
  const qualified = { ...row, observation: { ...row.observation, detail_fields: ["base", "comments", "threads", "reviewers", "reviews"] } };
  expect(remoteEventError("reviewing-updated", [qualified])).toBeNull();
  expect(() => assertRemoteReply("get_reviewing", [qualified])).not.toThrow();
  expect(remoteEventError("reviewing-updated", [{ ...qualified, observation: { ...qualified.observation, detail_fields: ["invented"] } }])).not.toBeNull();
});

it("accepts additive possible-process detail and compact groups with old-host omissions", () => {
  const liveness = { state: "unknown", why: 0 };
  const row = { session_id: "s", name: null, opening_prompt: null, cwd: null, git_branch: null,
    last_activity_at: null, liveness, cwd_state: { state: "not-recorded" }, kind: { kind: "own" },
    subagents: 0, waiting: { state: "no", reason: "never-observed" }, context_pressure: null };
  const base = { sessions: [row], reasons: ["uncertain"], registry_failure: null, registry_unreadable: [], registry_unnamed: [] };
  expect(() => assertRemoteReply("claude_sessions", base)).not.toThrow();
  const candidate = { pid: 42, cwd: null, cwd_truncated: false };
  const modern = { ...base, possible_process_groups: [[candidate]], sessions: [{ ...row,
    liveness: { ...liveness, possible_processes: { group: 0, total: 1 } } }] };
  expect(() => assertRemoteReply("claude_sessions", modern)).not.toThrow();
  const detail = { session_id: "s", claude_version: null, transcript_path: null, first_seen_at: "fixture-time",
    liveness: { state: "unknown", why: "uncertain", possible_processes: { candidates: [candidate], total: 1 } },
    transcript_state: { state: "not-recorded" }, resume: { command: "fixture", caveat: null, anchored: false }, runs: 0,
    registry_failure: null, kind: { kind: "own" }, subagents: [], parent: null, unattributed: null,
    compactions: null, agent_types: null, waiting: { state: "no", reason: "never-observed" } };
  expect(() => assertRemoteReply("claude_session_detail", detail)).not.toThrow();
  expect(() => assertRemoteReply("claude_sessions", { ...modern, possible_process_groups: [[{ ...candidate, pid: "private-sentinel" }]] })).toThrow();
});
