/// The session's task list, derived across messages (#1504, epic #1473).
///
/// Current Claude Code tracks its work with `TaskCreate` / `TaskUpdate`
/// calls, not `TodoWrite`. A `TaskUpdate` names a task by id and changes
/// it, so the checklist is STATE folded over the loaded messages, never
/// something one call can show on its own.
///
/// # What the records say (measured, this machine's corpus)
///
/// - A create's INPUT carries no id. The id it was given is written only
///   in its result (`toolUseResult.task.id`, surfaced as
///   `TranscriptToolOutput.task`); the result's text says the same thing
///   ("Task #N created successfully: ..."), and is read as a fallback.
///   A create whose id is recorded nowhere loaded is listed but cannot be
///   matched to any update.
/// - Every first status change recorded `from: "pending"`, so a created
///   task starts as `pending`.
/// - A refused update ("Task not found") was recorded with
///   `success: false` and no `is_error`. It is not applied.
///
/// # Partial windows
///
/// The loaded messages may start mid-session (`TranscriptPage.truncated`)
/// and an update may name a task whose create is above them. Such a task
/// is listed with its id and what the update said, its name unknown, and
/// the checklist is marked `partial`: every count it gives is then a
/// floor, and the renderers say so ("at least").

import type { TranscriptMessage, TranscriptToolOutput } from "../../types/transcript";
import type { ToolCallBlock } from "./types";

/// One task as the loaded messages leave it.
export interface ChecklistTask {
  /// Stable React key: the id, or the create call's own id when the task
  /// was given no recorded id.
  key: string;
  /// `null` when no id was recorded for its create.
  id: string | null;
  /// `null` when its create is not in the loaded messages and no update
  /// renamed it -- unknown, not empty.
  subject: string | null;
  active_form: string | null;
  /// Verbatim (`pending`, `in_progress`, `completed`, `deleted`, or a
  /// value this build does not know). `null` when nothing loaded said.
  status: string | null;
  /// Whether its `TaskCreate` is in the loaded messages.
  created: boolean;
  /// `false` when the change that set `status` has no result loaded yet,
  /// so it is what was ASKED for, not yet what happened.
  confirmed: boolean;
}

export interface TaskListState {
  /// Tasks known only from an update come first (they were created
  /// earlier); the rest in the order they were created.
  tasks: ChecklistTask[];
  /// Whether tasks may be missing: the messages start mid-session, or an
  /// update named a task whose create is not loaded.
  partial: boolean;
  /// The loaded messages start mid-session.
  truncated: boolean;
  /// A create whose id was recorded nowhere loaded. While one exists AND
  /// a task is known only by id, the two may be the same task, so a
  /// total would be possibly wrong -- `taskSummary` suppresses it.
  unlinked: number;
}

const CREATED_TEXT = /^Task #(\S+) created\b/;

/// The id a create was given: the recorded `task.id`, else the result
/// text's "Task #N created". `null` when neither was recorded.
function createdId(result: TranscriptToolOutput | null): string | null {
  if (!result) return null;
  if (result.task?.task_id) return result.task.task_id;
  return CREATED_TEXT.exec(result.text)?.[1] ?? null;
}

/// Whether a result says the call was REFUSED. A missing result is not a
/// refusal; see `confirmed`.
function refused(result: TranscriptToolOutput | null): boolean {
  return result !== null && (result.is_error === true || result.task?.success === false);
}

/// Fold every task call in `messages` into one checklist.
///
/// `truncated` is the page's own flag: whether anything before these
/// messages was not read. Sidechain messages are skipped -- a subagent's
/// task list is its own. A call seen twice (the same call id) counts
/// once.
export function deriveTaskChecklist(
  messages: readonly TranscriptMessage[],
  { truncated }: { truncated: boolean },
): TaskListState {
  const byId = new Map<string, ChecklistTask>();
  const order: ChecklistTask[] = [];
  const seenCalls = new Set<string>();
  let unlinked = 0;
  let orphaned = false;

  for (const m of messages) {
    if (m.is_sidechain) continue;
    for (const b of m.blocks) {
      if (b.kind !== "tool_call") continue;
      const callKey = b.id ?? `${m.id}#${b.index}`;
      if (seenCalls.has(callKey)) continue;
      seenCalls.add(callKey);
      const args = b.args;
      if (args.tool === "task_create") {
        if (refused(b.result)) continue;
        const id = createdId(b.result);
        const task: ChecklistTask = {
          key: id ?? `call:${callKey}`,
          id,
          subject: args.subject,
          active_form: args.active_form,
          status: "pending",
          created: true,
          confirmed: b.result !== null,
        };
        if (id === null) {
          unlinked += 1;
        } else {
          const old = byId.get(id);
          // The same id created again: the later create is the task
          // now, in its own place in the order.
          if (old) order.splice(order.indexOf(old), 1);
          byId.set(id, task);
        }
        order.push(task);
      } else if (args.tool === "task_update") {
        if (args.task_id === null || refused(b.result)) continue;
        let task = byId.get(args.task_id);
        if (!task) {
          orphaned = true;
          task = {
            key: args.task_id,
            id: args.task_id,
            subject: null,
            active_form: null,
            status: null,
            created: false,
            confirmed: true,
          };
          byId.set(args.task_id, task);
          order.push(task);
        }
        if (args.subject !== null) task.subject = args.subject;
        if (args.active_form !== null) task.active_form = args.active_form;
        const status = b.result?.task?.status_to ?? args.status;
        if (status !== null) {
          task.status = status;
          task.confirmed = b.result !== null;
        }
      }
    }
  }

  const tasks = [...order.filter((t) => !t.created), ...order.filter((t) => t.created)];
  return { tasks, partial: truncated || orphaned, truncated, unlinked };
}

/// "2 of 5 tasks done", qualified by what the checklist knows.
export interface TaskSummary {
  done: number;
  /// `null` when a total would be possibly wrong (see `unlinked`), and
  /// then suppressed rather than printed.
  total: number | null;
  /// Both counts are floors.
  atLeast: boolean;
  text: string;
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}

/// The one-line summary for #1484's "while you were away" card and the
/// panel header. `null` when no task is in the loaded messages -- there
/// is nothing to summarise, which is not "0 of 0 done". Deleted tasks
/// are not counted.
export function taskSummary(c: TaskListState): TaskSummary | null {
  const live = c.tasks.filter((t) => t.status !== "deleted");
  if (live.length === 0) return null;
  const done = live.filter((t) => t.status === "completed").length;
  const orphans = live.some((t) => !t.created);
  if (c.unlinked > 0 && orphans) {
    // A create with no id and a task known only by id may be one task
    // counted twice: the total is possibly wrong, so it is not printed.
    return { done, total: null, atLeast: true, text: `at least ${plural(done, "task")} done` };
  }
  const total = live.length;
  if (c.partial) {
    return {
      done,
      total,
      atLeast: true,
      text: `at least ${done.toLocaleString()} of at least ${plural(total, "task")} done`,
    };
  }
  return { done, total, atLeast: false, text: `${done.toLocaleString()} of ${plural(total, "task")} done` };
}

/// Words for a status, for screen readers and the compact row.
export function taskStatusWords(status: string | null): string {
  if (status === null) return "status not recorded";
  if (status === "completed") return "done";
  return status.replace(/_/g, " ");
}

/// The task a call is about, for the inline row: a create's recorded id,
/// or the id an update, get names. `null` when none is known.
export function taskIdOfCall(call: ToolCallBlock): string | null {
  switch (call.args.tool) {
    case "task_create":
      return createdId(call.result);
    case "task_update":
    case "task_get":
      return call.args.task_id;
    default:
      return null;
  }
}

/// Whether a task call's result says it was refused.
export function taskCallRefused(call: ToolCallBlock): boolean {
  return refused(call.result);
}
