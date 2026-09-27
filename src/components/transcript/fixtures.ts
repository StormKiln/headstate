/// Generic fixtures for the tool renderer tests (#1483). No real repo,
/// user or path names -- the privacy guard scans these.

import type { ClaudeFileChange, ClaudeToolArgs, Liveness } from "../../types/pr";
import type { TranscriptLive } from "../../api/hooks";
import type { TranscriptPage, TranscriptToolOutput } from "../../types/transcript";
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
