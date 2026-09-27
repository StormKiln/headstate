import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TranscriptSubagent } from "../../types/transcript";
import { call, DEAD, LIVE, output, RECORDED_CHANGE, UNKNOWN } from "./fixtures";
import { TaskStatusRow } from "./TaskStatusRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolCall } from "./ToolCall";
import { ToolResultOrphan } from "./ToolResultOrphan";

afterEach(cleanup);

const BASH = {
  tool: "bash",
  command: "cargo test --lib",
  description: "Run the tests",
  truncated: false,
} as const;

/// A `<pre>` whose text is exactly `text`, newlines and all.
function pre(text: string, scope: Pick<typeof screen, "getByText"> = screen) {
  return scope.getByText((_, el) => el?.tagName === "PRE" && el.textContent === text);
}

function group() {
  return screen.getByRole("group");
}

describe("Bash", () => {
  it("shows the command, exit status and duration, output collapsed to its line count", () => {
    render(
      <ToolCall
        call={call("Bash", BASH, output({ is_error: true, text: "Exit code 101\nline 2\nline 3" }))}
        variant="terminal"
        liveness={DEAD}
        durationMs={2300}
      />,
    );
    expect(screen.getByText("cargo test --lib")).toBeTruthy();
    expect(screen.getByText("exit 101")).toBeTruthy();
    expect(screen.getByText("2.3 s")).toBeTruthy();
    // Collapsed: the fold announces what it hides and how much.
    const fold = screen.getByRole("button", { name: "Output, 3 lines" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText(/line 2/)).toBeNull();
    fireEvent.click(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    const out = screen.getByText(/line 2/);
    // Errors are red.
    expect(out.closest("pre")?.getAttribute("style")).toContain("rgb(248, 81, 73)");
  });

  it("says completed, not exit 0, for a non-error result, and shows no duration it did not get", () => {
    render(
      <ToolCall call={call("Bash", BASH, output({ text: "ok" }))} variant="terminal" liveness={DEAD} />,
    );
    expect(screen.getByText("completed")).toBeTruthy();
    expect(screen.queryByText(/exit 0/)).toBeNull();
    expect(screen.queryByText(/\d+ ms|\d s/)).toBeNull();
  });

  it("qualifies the line count of clipped output and fetches the rest on request", async () => {
    const load = vi.fn().mockResolvedValue({
      message_id: "m-result",
      index: 0,
      text: "a\nb\nc\nd",
      clip: null,
    });
    render(
      <ToolCall
        call={call(
          "Bash",
          BASH,
          output({ text: "a\nb", clip: { shown_chars: 3, total_chars: 7 }, offset: 4096 }),
        )}
        variant="terminal"
        liveness={DEAD}
        onLoadFullText={load}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Output, at least 2 lines" }));
    expect(screen.getByText(/Showing the first 3 of 7 characters/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show all 7 characters" }));
    // With the record's offset, so the fetch reads one record (#1476).
    expect(load).toHaveBeenCalledWith({ messageId: "m-result", index: 0, offset: 4096 });
    await waitFor(() => expect(pre("a\nb\nc\nd")).toBeTruthy());
    expect(screen.queryByText(/Showing the first/)).toBeNull();
  });

  it("states a failed full-text load and keeps the button to try again", async () => {
    const load = vi.fn().mockRejectedValue(new Error("the transcript moved"));
    render(
      <ToolCall
        call={call("Bash", BASH, output({ text: "a", clip: { shown_chars: 1, total_chars: 9 } }))}
        variant="terminal"
        liveness={DEAD}
        onLoadFullText={load}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Output/ }));
    fireEvent.click(screen.getByRole("button", { name: "Show all 9 characters" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("the transcript moved"));
    expect(screen.getByRole("button", { name: "Show all 9 characters" })).toBeTruthy();
  });

  it("offers no full-text button when nothing can fetch it", () => {
    render(
      <ToolCall
        call={call("Bash", BASH, output({ text: "a", clip: { shown_chars: 1, total_chars: 9 } }))}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Output/ }));
    expect(screen.getByText(/Showing the first 1 of 9 characters/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Show all/ })).toBeNull();
  });

  /// The phone opens output in a sheet rather than in place.
  it("opens output in a sheet in the compact variant", async () => {
    render(
      <ToolCall
        call={call("Bash", BASH, output({ text: "one\ntwo" }))}
        variant="compact"
        liveness={DEAD}
      />,
    );
    const fold = screen.getByRole("button", { name: "Output, 2 lines" });
    expect(fold.getAttribute("aria-haspopup")).toBe("dialog");
    fireEvent.click(fold);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Run the tests")).toBeTruthy();
    expect(pre("one\ntwo", within(dialog))).toBeTruthy();
  });
});

describe("result states", () => {
  /// The four issue states render four different ways, and "could not
  /// tell" is a fifth sentence, not a borrowed one.
  it("renders paired, running, not recorded and earlier-page differently", () => {
    const seen = new Set<string>();
    const note = () => group().textContent ?? "";

    render(<ToolCall call={call("Bash", BASH, output({ text: "x" }))} variant="terminal" liveness={LIVE} />);
    expect(group().dataset.state).toBe("paired");
    seen.add(note());
    cleanup();

    render(<ToolCall call={call("Bash", BASH)} variant="terminal" liveness={LIVE} />);
    expect(group().dataset.state).toBe("running");
    expect(screen.getByText("Running…")).toBeTruthy();
    seen.add(note());
    cleanup();

    render(<ToolCall call={call("Bash", BASH)} variant="terminal" liveness={DEAD} />);
    expect(group().dataset.state).toBe("not_recorded");
    expect(screen.getByText(/no longer running/)).toBeTruthy();
    expect(screen.queryByText("Running…")).toBeNull();
    seen.add(note());
    cleanup();

    render(<ToolResultOrphan block={{ kind: "tool_result", ...output({ text: "x" }) }} variant="terminal" />);
    expect(group().dataset.state).toBe("call_in_earlier_page");
    expect(screen.getByText(/earlier part of the transcript/)).toBeTruthy();
    seen.add(note());
    cleanup();

    render(<ToolCall call={call("Bash", BASH)} variant="terminal" liveness={UNKNOWN} />);
    expect(screen.getByText(/could not be determined/)).toBeTruthy();
    expect(screen.queryByText("Running…")).toBeNull();
    seen.add(note());

    expect(seen.size).toBe(5);
  });

  it("offers to load earlier messages only when it can", () => {
    const onLoadEarlier = vi.fn();
    render(
      <ToolResultOrphan
        block={{ kind: "tool_result", ...output({ text: "x" }) }}
        variant="terminal"
        onLoadEarlier={onLoadEarlier}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Load earlier messages" }));
    expect(onLoadEarlier).toHaveBeenCalledOnce();
    // With the call it answers named, so the host pages back until it is held (#1484).
    expect(onLoadEarlier).toHaveBeenCalledWith("toolu_1");
  });

  it("says an unkeyed call cannot be matched", () => {
    render(<ToolCall call={call("Bash", BASH, null, null)} variant="terminal" liveness={LIVE} />);
    expect(screen.getByText(/no id/)).toBeTruthy();
  });
});

describe("Read", () => {
  it("summarises the path and the lines that came back", () => {
    render(
      <ToolCall
        call={call(
          "Read",
          { tool: "read", file_path: "src/lib.rs", offset: null, limit: null },
          output({ text: "     1\tfn a() {}\n     2\t}\n" }),
        )}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("src/lib.rs (2 lines)")).toBeTruthy();
  });

  it("words an unanswered window as what was asked for", () => {
    render(
      <ToolCall
        call={call("Read", { tool: "read", file_path: "a.rs", offset: 100, limit: 20 })}
        variant="terminal"
        liveness={LIVE}
      />,
    );
    expect(screen.getByText("a.rs (from line 100, up to 20 lines)")).toBeTruthy();
  });
});

describe("Edit / MultiEdit / Write", () => {
  const EDIT = {
    tool: "edit",
    file_path: "src/example.rs",
    old_string: "    let total = add(1, 2);",
    new_string: "    let total = add(1, 3);",
    replace_all: false,
    truncated: false,
  } as const;

  it("draws a recorded change as a numbered unified diff, expanded by default", () => {
    render(
      <ToolCall
        call={call("Edit", EDIT, output({ change: RECORDED_CHANGE }))}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    const toggle = screen.getByRole("button", { name: /^Diff of src\/example.rs/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("@@ -10,3 +10,4 @@")).toBeTruthy();
    const olds = screen.getAllByTestId("old-no").map((e) => e.textContent);
    const news = screen.getAllByTestId("new-no").map((e) => e.textContent);
    expect(olds).toEqual(["10", "11", "", "", "12"]);
    expect(news).toEqual(["10", "", "11", "12", "13"]);
    // Word-level: only the changed digit is marked.
    expect(screen.getAllByText("2", { selector: "mark" })).toHaveLength(1);
    expect(screen.getAllByText("3", { selector: "mark" })).toHaveLength(1);
    expect(screen.queryByText(/reconstructed/i)).toBeNull();
    fireEvent.click(toggle);
    expect(screen.queryByText("@@ -10,3 +10,4 @@")).toBeNull();
  });

  it("keeps one gutter on the phone", () => {
    render(
      <ToolCall
        call={call("Edit", EDIT, output({ change: RECORDED_CHANGE }))}
        variant="compact"
        liveness={DEAD}
      />,
    );
    expect(screen.queryAllByTestId("old-no")).toHaveLength(0);
    expect(screen.getAllByTestId("line-no").map((e) => e.textContent)).toEqual([
      "10",
      "11",
      "11",
      "12",
      "13",
    ]);
  });

  it("labels a diff reconstructed from the call's arguments and numbers nothing", () => {
    render(<ToolCall call={call("Edit", EDIT)} variant="terminal" liveness={LIVE} />);
    expect(screen.getByText("reconstructed")).toBeTruthy();
    expect(screen.getByText(/no surrounding lines or line numbers were recorded/)).toBeTruthy();
    expect(screen.getAllByTestId("old-no").every((e) => e.textContent === "")).toBe(true);
    expect(screen.queryByText(/^@@/)).toBeNull();
  });

  it("shows a refused edit's error and folds the attempt as not applied", () => {
    render(
      <ToolCall
        call={call("Edit", EDIT, output({ is_error: true, text: "String to replace not found" }))}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("String to replace not found")).toBeTruthy();
    const toggle = screen.getByRole("button", { name: /^Diff of/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(screen.getByText(/Not applied/)).toBeTruthy();
  });

  it("counts a multi-edit's replacements, omitted ones included", () => {
    render(
      <ToolCall
        call={call("MultiEdit", {
          tool: "multi_edit",
          file_path: "a.rs",
          edits: [{ old_string: "x", new_string: "y", replace_all: false, truncated: false }],
          edits_omitted: 2,
        })}
        variant="terminal"
        liveness={LIVE}
      />,
    );
    expect(screen.getByText("a.rs (3 edits)")).toBeTruthy();
    expect(screen.getByText(/2 more changed regions/)).toBeTruthy();
  });

  it("shows a created file as new, with its line count and numbered lines", () => {
    render(
      <ToolCall
        call={call(
          "Write",
          { tool: "write", file_path: "notes.md", content: "# Title\n\nBody\n", truncated: false },
          output({
            change: { file_path: "notes.md", source: "recorded", hunks: [], hunks_omitted: 0, created: true },
          }),
        )}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("notes.md (new file, 3 lines)")).toBeTruthy();
    expect(screen.getByText("new file")).toBeTruthy();
    expect(screen.getByText("@@ -0,0 +1,3 @@")).toBeTruthy();
    expect(screen.getAllByTestId("new-no").map((e) => e.textContent)).toEqual(["1", "2", "3"]);
  });

  it("does not claim a Write created its file when nothing said so", () => {
    render(
      <ToolCall
        call={call("Write", { tool: "write", file_path: "x.txt", content: "a\nb", truncated: true })}
        variant="terminal"
        liveness={LIVE}
      />,
    );
    expect(screen.getByText("x.txt (at least 2 lines)")).toBeTruthy();
    expect(screen.queryByText(/new file/)).toBeNull();
  });
});

describe("Grep / Glob", () => {
  it("summarises matches and lists files on expand", () => {
    render(
      <ToolCall
        call={call(
          "Grep",
          { tool: "grep", pattern: "fn main", path: "src", output_mode: null },
          output({ text: "Found 2 files\nsrc/a.rs\nsrc/b.rs" }),
        )}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("fn main in src")).toBeTruthy();
    expect(screen.getByText("2 files")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Files, 2 files" }));
    expect(pre("src/a.rs\nsrc/b.rs")).toBeTruthy();
  });

  it("counts a glob's files", () => {
    render(
      <ToolCall
        call={call("Glob", { tool: "glob", pattern: "**/*.ts", path: null }, output({ text: "a.ts\nb.ts\nc.ts" }))}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("3 files")).toBeTruthy();
  });
});

describe("Task / Agent", () => {
  const TASK = {
    tool: "task",
    description: "Survey the parser",
    subagent_type: "Explore",
    prompt: "Look at the parser.",
    truncated: false,
  } as const;
  const sub = (over: Partial<TranscriptSubagent> = {}): TranscriptSubagent => ({
    agent_id: "a1",
    status: "completed",
    agent_type: "Explore",
    transcript_path: "/tmp/session/subagents/agent-a1.jsonl",
    transcript_found: true,
    ...over,
  });

  it("links to the subagent's transcript and hands the link back to the viewer", () => {
    const open = vi.fn();
    const s = sub();
    render(
      <ToolCall
        call={call("Agent", TASK, output({ text: "Found it.", subagent: s }))}
        variant="terminal"
        liveness={DEAD}
        onOpenSubagent={open}
      />,
    );
    expect(screen.getByText("Survey the parser (Explore)")).toBeTruthy();
    expect(screen.getByText("completed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open the transcript of subagent Explore" }));
    expect(open).toHaveBeenCalledWith(s);
  });

  it("says an unchecked transcript was not checked, and a missing one was not found", () => {
    render(
      <ToolCall
        call={call("Agent", TASK, output({ subagent: sub({ transcript_found: null }) }))}
        variant="terminal"
        liveness={DEAD}
        onOpenSubagent={vi.fn()}
      />,
    );
    expect(screen.getByText(/not checked whether its file exists/)).toBeTruthy();
    cleanup();
    render(
      <ToolCall
        call={call("Agent", TASK, output({ subagent: sub({ transcript_found: false }) }))}
        variant="terminal"
        liveness={DEAD}
        onOpenSubagent={vi.fn()}
      />,
    );
    expect(screen.getByText(/transcript file was not found/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Open the transcript/ })).toBeNull();
  });
});

describe("TodoWrite", () => {
  it("renders a checklist with each status in words", () => {
    render(
      <ToolCall
        call={call("TodoWrite", {
          tool: "todo_write",
          todos: [
            { content: "Parse", status: "completed", active_form: null, truncated: false },
            { content: "Render", status: "in_progress", active_form: "Rendering", truncated: false },
            { content: "Ship", status: "pending", active_form: null, truncated: false },
            { content: "Unknown", status: null, active_form: null, truncated: false },
          ],
          todos_omitted: 0,
        }, output())}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("1 of 4 done")).toBeTruthy();
    const items = within(screen.getByRole("list", { name: "Todo list" })).getAllByRole("listitem");
    expect(items.map((i) => i.textContent)).toEqual([
      "☑done: Parse",
      "◐in progress: Rendering",
      "☐pending: Ship",
      "?status not recorded: Unknown",
    ]);
  });
});

describe("WebFetch / WebSearch", () => {
  it("shows the URL and what was asked, with the result folded", () => {
    render(
      <ToolCall
        call={call(
          "WebFetch",
          { tool: "web_fetch", url: "https://example.com/doc", prompt: "Summarise it", truncated: false },
          output({ text: "# Doc\nA summary." }),
        )}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("https://example.com/doc")).toBeTruthy();
    expect(screen.getByText("Summarise it")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Result, 2 lines" }));
    // Prose results go through TranscriptMarkdown (#1482).
    expect(screen.getByRole("heading", { name: "Doc" })).toBeTruthy();
  });

  it("renders raw HTML in a fetched page as text, never as an element", () => {
    const { container } = render(
      <ToolCall
        call={call(
          "WebFetch",
          { tool: "web_fetch", url: "https://example.com/x", prompt: "", truncated: false },
          output({ text: "<img src=https://example.com/t.png> after" }),
        )}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Result, 1 line" }));
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("after");
  });

  it("shows the query", () => {
    render(
      <ToolCall
        call={call("WebSearch", { tool: "web_search", query: "unified diff", truncated: false })}
        variant="terminal"
        liveness={LIVE}
      />,
    );
    expect(screen.getByText("unified diff")).toBeTruthy();
  });
});

describe("MCP and unknown tools", () => {
  it("names an MCP tool's server and tool, with its argument keys", () => {
    render(
      <ToolCall
        call={call("mcp__docs__search_pages", { tool: "other", keys: ["limit", "query"] }, output({ text: "[]" }))}
        variant="terminal"
        liveness={DEAD}
      />,
    );
    expect(screen.getByText("docs › search_pages")).toBeTruthy();
    expect(screen.getByText("MCP")).toBeTruthy();
    expect(screen.getByText("Arguments: limit, query")).toBeTruthy();
  });

  /// Never hidden: an unknown tool is named with its keys, and no input
  /// at all is its own sentence.
  it("names an unknown tool and tells no-arguments from unrecorded arguments", () => {
    render(
      <ToolCall call={call("FutureTool", { tool: "other", keys: ["x"] })} variant="terminal" liveness={LIVE} />,
    );
    expect(screen.getByText("FutureTool")).toBeTruthy();
    expect(screen.getByText("Arguments: x")).toBeTruthy();
    cleanup();
    render(<ToolCall call={call("FutureTool", { tool: "none" })} variant="terminal" liveness={LIVE} />);
    expect(screen.getByText("No arguments were recorded.")).toBeTruthy();
  });
});

describe("ThinkingBlock", () => {
  it("renders an unrecorded thought as not recorded, never as empty", () => {
    render(
      <ThinkingBlock
        block={{ kind: "thinking", index: 0, text: "", clip: null, recorded: false }}
        messageId="m1"
        offset={null}
        variant="terminal"
      />,
    );
    expect(screen.getByText(/Thinking \(not recorded\)/)).toBeTruthy();
  });

  it("shows a recorded thought and its clip", () => {
    render(
      <ThinkingBlock
        block={{
          kind: "thinking",
          index: 2,
          text: "Consider the parser",
          clip: { shown_chars: 19, total_chars: 400 },
          recorded: true,
        }}
        messageId="m1"
        offset={null}
        variant="compact"
        onLoadFullText={vi.fn()}
      />,
    );
    expect(screen.getByText("Consider the parser")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show all 400 characters" })).toBeTruthy();
  });
});

describe("TaskStatusRow", () => {
  it("shows a background task's status, and says when none was recorded", () => {
    render(
      <TaskStatusRow
        kind={{ kind: "task_status", task_id: "b1", task_type: "local_bash", status: "running" }}
        description="Wait for the build"
        variant="terminal"
      />,
    );
    expect(screen.getByText(/Background bash task/).textContent).toContain("running");
    cleanup();
    render(
      <TaskStatusRow
        kind={{ kind: "task_status", task_id: null, task_type: null, status: null }}
        description={null}
        variant="compact"
      />,
    );
    expect(screen.getByText(/status not recorded/)).toBeTruthy();
  });
});
