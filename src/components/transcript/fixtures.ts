/// Generic fixtures for the tool renderer tests (#1483). No real repo,
/// user or path names -- the privacy guard scans these.

import type { ClaudeFileChange, ClaudeToolArgs, Liveness } from "../../types/pr";
import type { TranscriptToolOutput } from "../../types/transcript";
import type { ToolCallBlock } from "./types";

export const LIVE: Liveness = { state: "running", pid: 42, status: "busy" };
export const DEAD: Liveness = { state: "dead", why: "exited" };
export const UNKNOWN: Liveness = { state: "unknown", why: "unreadable" };

export function output(over: Partial<TranscriptToolOutput> = {}): TranscriptToolOutput {
  return {
    message_id: "m-result",
    index: 0,
    timestamp: null,
    tool_use_id: "toolu_1",
    text: "",
    clip: null,
    is_error: false,
    change: null,
    images: [],
    subagent: null,
    task: null,
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
