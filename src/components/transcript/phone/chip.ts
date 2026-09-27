/// The one line a tool call shows as a chip on the phone (#1481). Pure.
///
/// "Run the tests · failed", "Edited testing.rs +23 −0": what the Claude
/// iOS app shows for a tool call, and all the phone shows until the chip
/// is tapped. The full call -- output, diff, arguments -- is
/// `ToolCall`'s, in the sheet the chip opens.
///
/// The rules the tool renderers keep (#1483) hold here too, because a
/// chip is the same claim in fewer words:
///
/// - a call with no result says WHICH no-result it is (`callState`),
///   and "running" only of a session that is running;
/// - a count from clipped or partial data is a floor ("at least");
/// - a failure says so in words, never by colour alone.

import type { ClaudeFileChange, Liveness } from "../../../types/pr";
import { changeFromArgs, diffStat, linesOf } from "../diff";
import { bashOutcome, callState, countLabel, isClipped, mcpName, searchSummary } from "../summary";
import { taskCallRefused, taskIdOfCall, taskStatusWords, type TaskListState } from "../tasks";
import type { ToolCallBlock } from "../types";

export type ChipTone = "ok" | "error" | "warn" | "muted";

export interface Chip {
  /// What the call did, as a sentence fragment: "Run the tests".
  text: string;
  /// How it ended, in words, or `null` when there is nothing to add to
  /// a call that completed.
  status: string | null;
  tone: ChipTone;
}

/// The last path segment. A phone line is too short for the directory,
/// and the sheet shows the whole path.
export function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const at = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return at === -1 ? trimmed : trimmed.slice(at + 1);
}

function firstLine(s: string): string {
  return linesOf(s)[0] ?? "";
}

/// "+23 −0", as a floor when the change says lines were not recorded.
function changeStat(change: ClaudeFileChange, countsKnown = true): string {
  const { added, removed } = diffStat(change);
  const partial =
    !countsKnown || change.hunks_omitted > 0 || change.hunks.some((h) => h.lines_omitted > 0);
  return `${partial ? "at least " : ""}+${added} −${removed}`;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/// The chip for one call, given whether its session is live and, for a
/// task call that names its task only by id, the session's task list.
export function toolChip(call: ToolCallBlock, liveness: Liveness, tasks?: TaskListState): Chip {
  const state = callState(call, liveness);
  const r = state.state === "paired" ? state.result : null;
  const failed = r?.is_error === true;
  const args = call.args;

  let text: string;
  // What a paired, successful call adds after its text. Failure and the
  // no-result states are decided below, once, for every tool.
  let done: string | null = null;
  switch (args.tool) {
    case "bash":
      // The description Claude gave the command already names the action
      // ("Run the tests"); only a bare command needs the verb.
      text = args.description ?? `Ran ${firstLine(args.command)}`;
      if (r) {
        const o = bashOutcome(r);
        if (o.kind === "error") {
          return {
            text,
            status: o.code !== null ? `failed, exit ${o.code}` : "failed",
            tone: "error",
          };
        }
      }
      break;
    case "read":
      text = `Read ${basename(args.file_path)}`;
      break;
    case "edit":
    case "multi_edit": {
      text = `Edited ${basename(args.file_path)}`;
      const change = r?.change ?? changeFromArgs(args);
      if (change && !failed) text += ` ${changeStat(change)}`;
      break;
    }
    case "write": {
      const created = r?.change?.created === true;
      text = `${created ? "Created" : "Wrote"} ${basename(args.file_path)}`;
      done = countLabel(linesOf(args.content).length, "line", args.truncated);
      break;
    }
    case "grep":
    case "glob":
      text = `Searched for ${args.pattern}`;
      if (r && !failed) {
        done = searchSummary(
          args.tool,
          args.tool === "grep" ? args.output_mode : null,
          r.text,
          isClipped(r.clip),
        ).label;
      }
      break;
    case "task":
      text = `Agent: ${args.description ?? args.subagent_type ?? "subagent"}`;
      done = r?.subagent?.status ? r.subagent.status.replace(/_/g, " ") : null;
      break;
    case "todo_write": {
      const total = args.todos.length + args.todos_omitted;
      const doneCount = args.todos.filter((t) => t.status === "completed").length;
      text = "Updated the todo list";
      done = `${args.todos_omitted > 0 ? "at least " : ""}${doneCount} of ${total} done`;
      break;
    }
    case "web_fetch":
      text = `Fetched ${hostOf(args.url)}`;
      break;
    case "web_search":
      text = `Searched the web for ${args.query}`;
      break;
    case "task_create":
    case "task_update":
    case "task_get":
    case "task_list": {
      const id = taskIdOfCall(call);
      const ref = id !== null ? `#${id}` : "a task";
      const known = id !== null ? tasks?.tasks.find((t) => t.id === id) : undefined;
      if (args.tool === "task_create") {
        text = `Added task ${id !== null ? `#${id} ` : ""}· ${args.subject}`;
      } else if (args.tool === "task_update") {
        const name = args.subject ?? known?.subject ?? null;
        const change =
          args.status !== null
            ? `→ ${taskStatusWords(args.status)}`
            : args.subject !== null
              ? "renamed"
              : "updated";
        text = `Task ${ref} ${change}${name !== null ? ` · ${name}` : ""}`;
      } else if (args.tool === "task_get") {
        text = `Looked up task ${ref}${known?.subject ? ` · ${known.subject}` : ""}`;
      } else {
        text = "Listed the tasks";
      }
      // Refused with `success: false` and no error flag (#1504): said as
      // "not applied", which is what was recorded, not as an error.
      if (taskCallRefused(call) && r?.is_error !== true) {
        return { text, status: "not applied", tone: "error" };
      }
      break;
    }
    case "other":
    case "none": {
      const mcp = mcpName(call.name);
      text = mcp ? `${mcp.server} › ${mcp.tool}` : call.name;
      break;
    }
  }

  switch (state.state) {
    case "paired":
      return failed
        ? { text, status: "failed", tone: "error" }
        : { text, status: done, tone: "ok" };
    case "running":
      return { text, status: "running…", tone: "muted" };
    case "not_recorded":
      // The crash signature, as a fact about the recording.
      return { text, status: "no result recorded", tone: "warn" };
    case "unknown":
      return { text, status: "result not known", tone: "muted" };
    case "unkeyed":
      return { text, status: "result cannot be matched", tone: "muted" };
  }
}
