/// Generic fixtures for the tool renderer tests (#1483). No real repo,
/// user or path names -- the privacy guard scans these.

import type { ClaudeFileChange, ClaudeToolArgs, Liveness } from "../../types/pr";
import type { TranscriptLive } from "../../api/hooks";
import type {
  TranscriptBlock,
  TranscriptMessage,
  TranscriptPage,
  TranscriptToolOutput,
} from "../../types/transcript";
import type { PendingMessage } from "./pending";
import type { ToolCallBlock } from "./types";

export const LIVE: Liveness = { state: "running", pid: 42, status: "busy" };

/// A message being sent (#1491), in any state the renderers draw.
export function pendingMessage(over: Partial<PendingMessage> = {}): PendingMessage {
  return {
    clientId: "c1",
    text: "please run the tests",
    createdAt: Date.parse("2026-01-01T00:00:00Z"),
    state: "pending",
    after: null,
    reason: null,
    ...over,
  };
}

export const PENDING_STATES: PendingMessage["state"][] = [
  "pending",
  "delivered",
  "unconfirmed",
  "failed",
];
export const DEAD: Liveness = { state: "dead", why: "exited" };
export const UNKNOWN: Liveness = { state: "unknown", why: "unreadable" };

export function output(over: Partial<TranscriptToolOutput> = {}): TranscriptToolOutput {
  return {
    message_id: "m-result",
    index: 0,
    timestamp: null,
    offset: null,
    tool_use_id: "toolu_1",
    text: "",
    clip: null,
    is_error: false,
    change: null,
    images: [],
    subagent: null,
    task: null,
    oversized_bytes: null,
    ...over,
  };
}

export function call(
  name: string,
  args: ClaudeToolArgs,
  result: TranscriptToolOutput | null = null,
  id: string | null = "toolu_1",
): ToolCallBlock {
  return { kind: "tool_call", index: 1, name, id, args, result };
}

export const RECORDED_CHANGE: ClaudeFileChange = {
  file_path: "src/example.rs",
  source: "recorded",
  hunks: [
    {
      old_start: 10,
      new_start: 10,
      lines: [
        { op: "context", text: "fn main() {" },
        { op: "removed", text: "    let total = add(1, 2);" },
        { op: "added", text: "    let total = add(1, 3);" },
        { op: "added", text: "    println!(\"{total}\");" },
        { op: "context", text: "}" },
      ],
      lines_omitted: 0,
    },
  ],
  hunks_omitted: 0,
  created: false,
};

/// What `useClaudeTranscriptLive` returns (#1476), for a host test that
/// mocks the hook: one whole page read, a read not answered yet
/// (`undefined`), or a first read that failed (`failed`).
export function liveOf(
  page: TranscriptPage | undefined,
  failed: unknown = undefined,
  over: Partial<TranscriptLive> = {},
): TranscriptLive {
  const n = page?.messages.length ?? 0;
  const read = failed === undefined ? page : undefined;
  return {
    messages: read?.messages,
    status: failed !== undefined ? "could-not-read" : page === undefined ? "loading" : "stopped",
    error: failed ?? null,
    lastReadAt: read === undefined ? null : Date.UTC(2026, 0, 1, 12, 4, 31),
    hasOlder: read?.truncated ?? false,
    atLiveEdge: true,
    older: { state: "idle" },
    fileBytes: read?.file_bytes ?? null,
    masking: undefined,
    replacements: 0,
    position:
      read === undefined
        ? null
        : {
            first: n === 0 ? null : 1,
            last: n === 0 ? null : n,
            total: read.truncated ? null : n,
            exact: !read.truncated,
            basis: read.truncated ? "bytes" : "whole_file",
          },
    loadOlder: () => undefined,
    loadNewer: () => undefined,
    jumpToLatest: () => undefined,
    refresh: () => Promise.resolve(),
    setViewport: () => undefined,
    seek: () => Promise.resolve(true),
    loadOlderUntil: () => Promise.resolve(false),
    ...over,
  };
}

// ---------------------------------------------------------------------
// Every record kind and every tool (#1489), for the renderers' axe and
// separation checks. A `Record` over the kinds, so a kind added to the
// read model fails to compile here until it has a fixture.
// ---------------------------------------------------------------------

type Kind = TranscriptMessage["kind"]["kind"];

function record(
  id: string,
  turn: string,
  kind: TranscriptMessage["kind"],
  blocks: TranscriptBlock[],
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: turn,
    kind,
    timestamp: "2026-01-01T12:03:00Z",
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks,
    ...over,
  };
}

const said = (text: string, index = 0): TranscriptBlock => ({ kind: "text", index, text, clip: null });

/// One call of every tool this build draws, each with a result but the
/// last, whose input was not recorded and which has none.
export function everyToolCall(): ToolCallBlock[] {
  const calls: [string, ClaudeToolArgs, TranscriptToolOutput | null][] = [
    ["Bash", { tool: "bash", command: "make test", description: "Run the tests", truncated: false }, output({ text: "ok\nall passed" })],
    ["Bash", { tool: "bash", command: "make lint", description: null, truncated: false }, output({ text: "exit code 2\nfailed", is_error: true })],
    ["Read", { tool: "read", file_path: "src/example.rs", offset: null, limit: null }, output({ text: "     1\tfn main() {}\n     2\t" })],
    ["Edit", { tool: "edit", file_path: "src/example.rs", old_string: "a", new_string: "b", replace_all: false, truncated: false }, output({ change: RECORDED_CHANGE })],
    ["MultiEdit", { tool: "multi_edit", file_path: "src/example.rs", edits: [{ old_string: "a", new_string: "b", replace_all: false, truncated: false }], edits_omitted: 0 }, output({ text: "ok" })],
    ["Write", { tool: "write", file_path: "src/new.rs", content: "fn new() {}\n", truncated: false }, output({ text: "ok" })],
    ["Grep", { tool: "grep", pattern: "todo", path: "src", output_mode: null }, output({ text: "src/a.rs\nsrc/b.rs" })],
    ["Glob", { tool: "glob", pattern: "**/*.rs", path: null }, output({ text: "src/a.rs" })],
    ["Task", { tool: "task", description: "Explore", subagent_type: "explorer", prompt: "Look around\nand report", truncated: false }, output({ text: "Report", subagent: { agent_id: "a1", status: "completed", agent_type: "explorer", transcript_path: "/tmp/projects/p/subagents/agent-1.jsonl", transcript_found: true } })],
    ["TodoWrite", { tool: "todo_write", todos: [{ content: "Write the parser", status: "in_progress", active_form: null, truncated: false }, { content: "Test it", status: "pending", active_form: null, truncated: false }], todos_omitted: 0 }, output({ text: "ok" })],
    ["WebFetch", { tool: "web_fetch", url: "https://example.com/doc", prompt: "Summarise", truncated: false }, output({ text: "A summary" })],
    ["WebSearch", { tool: "web_search", query: "example query", truncated: false }, output({ text: "Results" })],
    ["TaskCreate", { tool: "task_create", subject: "Write the parser", description: "Parse it", active_form: null, truncated: false }, output({ text: "Task #3 created", task: { task_id: "3", success: true, status_from: null, status_to: null } })],
    ["TaskUpdate", { tool: "task_update", task_id: "3", status: "completed", subject: null, active_form: null, fields: ["status"], truncated: false }, output({ text: "ok", task: { task_id: "3", success: true, status_from: "in_progress", status_to: "completed" } })],
    ["TaskGet", { tool: "task_get", task_id: "3" }, output({ text: "Task 3" })],
    ["TaskList", { tool: "task_list" }, output({ text: "1 task" })],
    ["Mystery", { tool: "other", keys: ["alpha", "beta"] }, output({ text: "done" })],
    ["Unrecorded", { tool: "none" }, null],
  ];
  return calls.map(([name, args, result], i) => ({
    kind: "tool_call",
    index: i + 10,
    name,
    id: `toolu_${i}`,
    args,
    result: result === null ? null : { ...result, tool_use_id: `toolu_${i}`, index: i + 10 },
  }));
}

/// One record of every kind the read model has, in a plausible order.
export function everyRecord(): TranscriptMessage[] {
  const byKind: Record<Kind, TranscriptMessage> = {
    user_prompt: record("u1", "u1", { kind: "user_prompt", origin: null }, [said("Please fix the parser")]),
    slash_command: record("sc1", "sc1", { kind: "slash_command", name: "review" }, [said("the diff")]),
    shell_input: record("sh1", "sh1", { kind: "shell_input", command: "ls -la" }, []),
    command_output: record("co1", "sh1", { kind: "command_output", command: "ls" }, [said("a.rs\nb.rs")]),
    assistant: record(
      "a1",
      "u1",
      { kind: "assistant" },
      [
        { kind: "thinking", index: 0, text: "Consider the parser first.", clip: null, recorded: true },
        { kind: "thinking", index: 1, text: "", clip: null, recorded: false },
        said("I will **fix** it. See `parse()` and ⟦hidden:api-key⟧ here.", 2),
        { kind: "image", index: 3, image: { media_type: "image/png", approx_bytes: 2048, width: 10, height: 10 } },
        { kind: "other", index: 4, block_type: "server_tool_use" },
        ...everyToolCall(),
      ],
      { timestamp: "2026-01-01T12:04:00Z" },
    ),
    tool_results: record("tr1", "u1", { kind: "tool_results" }, [
      { kind: "tool_result", ...output({ text: "late output\nsecond line", tool_use_id: "toolu_gone" }) },
    ]),
    agent_notification: record("an1", "u1", { kind: "agent_notification", task_id: "t1", status: "completed" }, [said("Agent done")]),
    task_status: record("ts1", "u1", { kind: "task_status", task_id: "t1", task_type: "local_bash", status: "running" }, [said("Watching the build")]),
    injected: record("in1", "u1", { kind: "injected", origin: "hook" }, [said("Context line one\nline two")]),
    queued_prompt: record("q1", "q1", { kind: "queued_prompt", mode: null }, [said("And then the docs")]),
    interruption: record("int1", "q1", { kind: "interruption", during_tool_use: true }, [said("[Request interrupted by user]")]),
    compaction_boundary: record("cb1", "q1", { kind: "compaction_boundary", trigger: "auto", pre_tokens: 1000, post_tokens: 100 }, []),
    compaction_summary: record("cs1", "q1", { kind: "compaction_summary" }, [said("Summary\nof what\nhappened")]),
    summary: record("s1", "q1", { kind: "summary", leaf_uuid: null }, [said("Fixed the parser")]),
    api_error: record("e1", "q1", { kind: "api_error", status: 529, error_type: "overloaded_error", retry_attempt: 1, max_retries: 3, retry_in_ms: 2000 }, [said("Overloaded")]),
    hook_output: record("h1", "q1", { kind: "hook_output", event: "PreToolUse", name: "check", outcome: "error", exit_code: 1, prevented_continuation: true }, [said("hook said no\nsecond line")]),
    turn_duration: record("td1", "q1", { kind: "turn_duration", message_count: 4 }, [], { duration_ms: 12000 }),
    notice: record("n1", "q1", { kind: "notice", subtype: "local_command", level: "warning" }, [said("A notice")]),
    model_change: record("mc1", "q1", { kind: "model_change", from: "model-a", to: "model-b" }, []),
    permission_mode_change: record("pm1", "q1", { kind: "permission_mode_change", mode: "plan" }, []),
    unrecognised: record("ur1", "q1", { kind: "unrecognised", record_type: "future_thing" }, []),
  };
  return Object.values(byKind);
}
