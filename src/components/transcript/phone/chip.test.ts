import { describe, expect, it } from "vitest";
import { call, DEAD, LIVE, output, RECORDED_CHANGE, UNKNOWN } from "../fixtures";
import { basename, toolChip } from "./chip";

describe("toolChip", () => {
  it("uses Claude's own description of a command, and says how it failed", () => {
    const bash = { tool: "bash", command: "cargo test", description: "Run the tests", truncated: false } as const;
    expect(toolChip(call("Bash", bash, output({ text: "ok" })), DEAD)).toEqual({
      text: "Run the tests",
      status: null,
      tone: "ok",
    });
    expect(
      toolChip(call("Bash", bash, output({ is_error: true, text: "Exit code 2\nno" })), DEAD).status,
    ).toBe("failed, exit 2");
    expect(toolChip(call("Bash", { ...bash, description: null }, null), LIVE).text).toBe(
      "Ran cargo test",
    );
  });

  it("keeps each no-result state apart, and says running only of a live session", () => {
    const read = call("Read", { tool: "read", file_path: "/a/b/c.rs", offset: null, limit: null }, null);
    expect(toolChip(read, LIVE).status).toBe("running…");
    expect(toolChip(read, DEAD).status).toBe("no result recorded");
    expect(toolChip(read, UNKNOWN).status).toBe("result not known");
    expect(toolChip(call("Read", read.args, null, null), LIVE).status).toBe("result cannot be matched");
  });

  it("counts an edit's lines, as a floor when lines were not recorded", () => {
    const edit = {
      tool: "edit",
      file_path: "src/example.rs",
      old_string: "a",
      new_string: "b",
      replace_all: false,
      truncated: false,
    } as const;
    expect(toolChip(call("Edit", edit, output({ change: RECORDED_CHANGE })), DEAD).text).toBe(
      "Edited example.rs +2 −1",
    );
    const partial = { ...RECORDED_CHANGE, hunks_omitted: 1 };
    expect(toolChip(call("Edit", edit, output({ change: partial })), DEAD).text).toBe(
      "Edited example.rs at least +2 −1",
    );
    // A refused edit makes no claim about lines changed.
    const refused = toolChip(call("Edit", edit, output({ is_error: true, text: "no match" })), DEAD);
    expect(refused).toEqual({ text: "Edited example.rs", status: "failed", tone: "error" });
  });

  it("names a search's result count from the result", () => {
    const grep = { tool: "grep", pattern: "fn main", path: null, output_mode: null } as const;
    expect(
      toolChip(call("Grep", grep, output({ text: "Found 3 files\na\nb\nc" })), DEAD),
    ).toMatchObject({ text: "Searched for fn main", status: "3 files" });
  });

  it("says a refused task update was not applied", () => {
    const update = call(
      "TaskUpdate",
      {
        tool: "task_update",
        task_id: "9",
        status: "completed",
        subject: null,
        active_form: null,
        fields: ["status"],
        truncated: false,
      },
      output({ text: "Task not found", task: { task_id: null, success: false, status_from: null, status_to: null } }),
    );
    expect(toolChip(update, DEAD)).toMatchObject({ status: "not applied", tone: "error" });
  });

  it("names an MCP tool by server and tool", () => {
    expect(toolChip(call("mcp__files__read", { tool: "other", keys: ["p"] }, output()), DEAD).text).toBe(
      "files › read",
    );
  });
});

describe("basename", () => {
  it("keeps the last segment of either separator", () => {
    expect(basename("/a/b/c.rs")).toBe("c.rs");
    expect(basename("C:\\x\\y.ts")).toBe("y.ts");
    expect(basename("plain")).toBe("plain");
  });
});
