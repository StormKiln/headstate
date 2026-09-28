/// #1504: the task checklist folded across messages. Generic fixtures cut
/// to the measured record shapes -- no real task text.

import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ClaudeToolArgs } from "../../types/pr";
import type {
  TranscriptMessage,
  TranscriptTaskResult,
  TranscriptToolOutput,
} from "../../types/transcript";
import { call, DEAD, LIVE, output } from "./fixtures";
import { TaskChecklist } from "./TaskChecklist";
import { deriveTaskChecklist, taskSummary } from "./tasks";
import { ToolCall } from "./ToolCall";
import type { ToolCallBlock } from "./types";

afterEach(cleanup);

let seq = 0;

function message(blocks: ToolCallBlock[], over: Partial<TranscriptMessage> = {}): TranscriptMessage {
  seq += 1;
  return {
    id: `m${seq}`,
    id_source: "uuid",
    turn_id: null,
    kind: { kind: "assistant" },
    timestamp: null,
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

function taskResult(over: Partial<TranscriptTaskResult>): TranscriptTaskResult {
  return { task_id: null, success: null, status_from: null, status_to: null, ...over };
}

/// A create, answered the way the records answer it: the id in
/// `task`, and "Task #N created successfully" in the text.
function create(
  subject: string,
  id: string | null,
  result: TranscriptToolOutput | null | "default" = "default",
): ToolCallBlock {
  seq += 1;
  const args: ClaudeToolArgs = {
    tool: "task_create",
    subject,
    description: "Details.",
    active_form: `Working on ${subject}`,
    truncated: false,
  };
  const r =
    result === "default"
      ? output({
          tool_use_id: `c${seq}`,
          text: `Task #${id} created successfully: ${subject}`,
          task: taskResult({ task_id: id }),
          is_error: null,
        })
      : result;
  return call("TaskCreate", args, r, `c${seq}`);
}

function update(
  taskId: string,
  status: string | null,
  result: TranscriptToolOutput | null | "default" = "default",
  extra: Partial<Extract<ClaudeToolArgs, { tool: "task_update" }>> = {},
): ToolCallBlock {
  seq += 1;
  const args: ClaudeToolArgs = {
    tool: "task_update",
    task_id: taskId,
    status,
    subject: null,
    active_form: null,
    fields: status !== null ? ["status"] : [],
    truncated: false,
    ...extra,
  };
  const r =
    result === "default"
      ? output({
          tool_use_id: `u${seq}`,
          text: `Updated task #${taskId} status`,
          task: taskResult({ task_id: taskId, success: true, status_to: status }),
          is_error: null,
        })
      : result;
  return call("TaskUpdate", args, r, `u${seq}`);
}

describe("deriveTaskChecklist", () => {
  it("folds create, update, update into one task's final state", () => {
    const c = deriveTaskChecklist(
      [
        message([create("Write the parser", "1"), create("Test it", "2")]),
        message([update("1", "in_progress")]),
        message([update("1", "completed"), update("2", "in_progress")]),
      ],
      { truncated: false },
    );
    expect(c.partial).toBe(false);
    expect(c.tasks.map((t) => [t.id, t.subject, t.status, t.confirmed])).toEqual([
      ["1", "Write the parser", "completed", true],
      ["2", "Test it", "in_progress", true],
    ]);
    expect(taskSummary(c)?.text).toBe("1 of 2 tasks done");
  });

  it("walks every status transition, pending first", () => {
    const steps: TranscriptMessage[] = [message([create("Only", "4")])];
    const seen: (string | null)[] = [];
    for (const s of [null, "in_progress", "completed"]) {
      if (s !== null) steps.push(message([update("4", s)]));
      seen.push(deriveTaskChecklist(steps, { truncated: false }).tasks[0].status);
    }
    expect(seen).toEqual(["pending", "in_progress", "completed"]);
  });

  it("qualifies a partial window: a task known only from an update, and a truncated start", () => {
    const messages = [message([update("7", "completed")]), message([create("New", "8")])];
    const c = deriveTaskChecklist(messages, { truncated: true });
    expect(c.partial).toBe(true);
    expect(c.tasks[0]).toMatchObject({ id: "7", subject: null, status: "completed", created: false });
    const s = taskSummary(c)!;
    expect(s.atLeast).toBe(true);
    expect(s.text).toBe("at least 1 of at least 2 tasks done");

    // Not truncated, but an update names a task no loaded create made:
    // still partial -- the evidence says something is missing.
    expect(deriveTaskChecklist(messages, { truncated: false }).partial).toBe(true);
  });

  it("does not apply a refused update, whether it said is_error or only success: false", () => {
    const notFound = output({
      text: "Task not found",
      is_error: null,
      task: taskResult({ task_id: "1", success: false }),
    });
    const invalid = output({ text: "<tool_use_error>bad input</tool_use_error>", is_error: true });
    const c = deriveTaskChecklist(
      [message([create("A", "1"), update("1", "completed", notFound), update("1", "deleted", invalid)])],
      { truncated: false },
    );
    expect(c.tasks.map((t) => t.status)).toEqual(["pending"]);
  });

  it("marks a change with no result yet as requested, not confirmed", () => {
    const c = deriveTaskChecklist([message([create("A", "1"), update("1", "in_progress", null)])], {
      truncated: false,
    });
    expect(c.tasks[0]).toMatchObject({ status: "in_progress", confirmed: false });
  });

  it("tolerates unknown task fields and an unknown status, kept verbatim", () => {
    const c = deriveTaskChecklist(
      [
        message([
          create("A", "1"),
          update("1", "blocked", "default", { fields: ["owner", "status", "zzz"] }),
          update("1", null, "default", { subject: "A, renamed", fields: ["subject"] }),
        ]),
      ],
      { truncated: false },
    );
    expect(c.tasks[0]).toMatchObject({ subject: "A, renamed", status: "blocked" });
    expect(taskSummary(c)?.text).toBe("0 of 1 task done");
  });

  it("reads a create's id from the result text when the structured id is absent", () => {
    const textOnly = output({ text: "Task #12 created successfully: A", task: null });
    const c = deriveTaskChecklist(
      [message([create("A", null, textOnly)]), message([update("12", "completed")])],
      { truncated: false },
    );
    expect(c.tasks).toHaveLength(1);
    expect(c.tasks[0]).toMatchObject({ id: "12", status: "completed", created: true });
  });

  it("suppresses the total when an id-less create and an id-only task may be one task", () => {
    const c = deriveTaskChecklist(
      [message([create("A", null, null)]), message([update("3", "completed")])],
      { truncated: false },
    );
    const s = taskSummary(c)!;
    expect(s.total).toBeNull();
    expect(s.text).toBe("at least 1 task done");
  });

  it("counts a call seen twice once, skips sidechains, and leaves deleted tasks out of the counts", () => {
    const dup = update("1", "completed");
    const c = deriveTaskChecklist(
      [
        message([create("A", "1"), create("B", "2")]),
        message([dup]),
        message([dup]),
        message([update("2", "deleted")]),
        message([create("Sub", "1")], { is_sidechain: true }),
      ],
      { truncated: false },
    );
    expect(c.tasks.map((t) => [t.subject, t.status])).toEqual([
      ["A", "completed"],
      ["B", "deleted"],
    ]);
    expect(taskSummary(c)?.text).toBe("1 of 1 task done");
  });

  it("has nothing to summarise with no tasks, which is not 0 of 0", () => {
    expect(taskSummary(deriveTaskChecklist([], { truncated: false }))).toBeNull();
  });
});

describe("TaskChecklist panel", () => {
  it("lists each task with its status in words and a partial window labelled", () => {
    const c = deriveTaskChecklist(
      [
        message([update("1", "completed")]),
        message([create("Write the parser", "2"), create("Test it", "3")]),
        message([update("2", "in_progress")]),
      ],
      { truncated: true },
    );
    render(<TaskChecklist checklist={c} variant="compact" />);
    const panel = screen.getByRole("region", { name: "Tasks" });
    expect(within(panel).getByText("at least 1 of at least 3 tasks done")).toBeTruthy();
    const items = within(panel).getAllByRole("listitem").map((i) => i.textContent);
    expect(items).toEqual([
      "☑done: #1 (name not in the messages shown)",
      "◐in progress: #2 Working on Write the parser",
      "☐pending: #3 Test it",
    ]);
    expect(
      within(panel).getByText("Tasks created before the earliest message shown are not listed."),
    ).toBeTruthy();
  });

  it("says there are no tasks rather than drawing an empty list", () => {
    render(<TaskChecklist checklist={deriveTaskChecklist([], { truncated: false })} variant="terminal" />);
    expect(screen.getByText("No tasks in the messages shown.")).toBeTruthy();
    expect(screen.queryByRole("list")).toBeNull();
  });
});

describe("task calls in the transcript", () => {
  it("renders a create with its recorded id and an update as 'task N → status' with the name", () => {
    const c1 = create("Write the parser", "3");
    const u1 = update("3", "in_progress");
    const tasks = deriveTaskChecklist([message([c1]), message([u1])], { truncated: false });
    render(
      <>
        <ToolCall call={c1} variant="compact" liveness={DEAD} tasks={tasks} />
        <ToolCall call={u1} variant="compact" liveness={DEAD} tasks={tasks} />
      </>,
    );
    const [g1, g2] = screen.getAllByRole("group");
    expect(g1.getAttribute("aria-label")).toBe("TaskCreate: #3 Write the parser");
    expect(g2.getAttribute("aria-label")).toBe("TaskUpdate: #3 → in progress · Write the parser");
  });

  it("names a refused update as not applied, and fields it does not show", () => {
    const refused = update(
      "9",
      "completed",
      output({ text: "Task not found", is_error: null, task: taskResult({ success: false }) }),
      { fields: ["owner", "status"] },
    );
    render(<ToolCall call={refused} variant="terminal" liveness={DEAD} />);
    expect(screen.getByText("not applied")).toBeTruthy();
    expect(screen.getByText("Task not found")).toBeTruthy();
    expect(screen.getByText("Also set: owner")).toBeTruthy();
  });

  it("shows a running create without inventing its id", () => {
    render(<ToolCall call={create("Pending one", null, null)} variant="terminal" liveness={LIVE} />);
    expect(screen.getByRole("group").getAttribute("aria-label")).toBe("TaskCreate: Pending one");
    expect(screen.getByText("Running…")).toBeTruthy();
  });

  it("renders TaskList and TaskGet by name, not as unknown tools", () => {
    render(
      <>
        <ToolCall
          call={call("TaskList", { tool: "task_list" }, output({ text: "#1 [pending] A" }))}
          variant="terminal"
          liveness={DEAD}
        />
        <ToolCall
          call={call("TaskGet", { tool: "task_get", task_id: "1" }, output({ text: "A" }))}
          variant="terminal"
          liveness={DEAD}
        />
      </>,
    );
    const labels = screen.getAllByRole("group").map((g) => g.getAttribute("aria-label"));
    expect(labels).toEqual(["TaskList", "TaskGet: #1"]);
    expect(screen.queryByText(/Arguments:/)).toBeNull();
  });
});
