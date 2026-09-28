import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { Liveness } from "../../../types/pr";
import type { TranscriptBlock, TranscriptMessage } from "../../../types/transcript";
import {
  call,
  DEAD,
  LIVE,
  output,
  PENDING_STATES,
  pendingMessage,
  RECORDED_CHANGE,
} from "../fixtures";
import { deriveTaskChecklist } from "../tasks";
import { PhoneContext, type PhoneTranscriptContext } from "./context";
import { PhoneMessage, PhonePendingMessage } from "./PhoneMessage";
import { INLINE_DIFF_LINES } from "./ToolChip";
import transcriptTypes from "../../../types/transcript.ts?raw";

afterEach(cleanup);

/// A message of `kind` with `blocks`. Generic ids and text only: the
/// privacy guard scans fixtures.
function msg(
  kind: TranscriptMessage["kind"],
  blocks: TranscriptBlock[] = [],
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return {
    id: "m1",
    id_source: "uuid",
    turn_id: "m1",
    kind,
    timestamp: "2026-01-01T00:00:10Z",
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

const text = (t: string, index = 0): TranscriptBlock => ({ kind: "text", index, text: t, clip: null });

function ctx(over: Partial<PhoneTranscriptContext> = {}): PhoneTranscriptContext {
  return {
    liveness: DEAD,
    tasks: deriveTaskChecklist([], { truncated: false }),
    scale: 1,
    thinkingStarts: new Map(),
    ...over,
  };
}

function show(m: TranscriptMessage, over: Partial<PhoneTranscriptContext> = {}) {
  return render(
    <PhoneContext.Provider value={ctx(over)}>
      <PhoneMessage message={m} />
    </PhoneContext.Provider>,
  );
}

describe("each record kind renders", () => {
  /// Every kind the read model has, and a word it must show. A kind that
  /// rendered as nothing would be a record silently dropped from the
  /// conversation -- the one thing the read model's allowlist forbids.
  const CASES: [TranscriptMessage["kind"], TranscriptBlock[], RegExp][] = [
    [{ kind: "user_prompt", origin: null }, [text("please run it")], /please run it/],
    [{ kind: "slash_command", name: "review" }, [text("the diff")], /\/review/],
    [{ kind: "shell_input", command: "ls -la" }, [], /ls -la/],
    [{ kind: "command_output", command: "/cost" }, [text("total 3")], /Output of \/cost/],
    [{ kind: "assistant" }, [text("All **done**.")], /All done\./],
    [{ kind: "tool_results" }, [{ kind: "tool_result", ...output({ text: "orphan out" }) }], /earlier part/],
    [
      { kind: "agent_notification", task_id: "t1", status: "completed" },
      [text("agent finished")],
      /Background agent completed/,
    ],
    [
      { kind: "task_status", task_id: "t1", task_type: "local_bash", status: "running" },
      [text("build")],
      /Background bash task/,
    ],
    [{ kind: "injected", origin: null }, [text("ctx")], /Added context/],
    [{ kind: "queued_prompt", mode: null }, [text("next one")], /Queued/],
    [{ kind: "interruption", during_tool_use: true }, [], /Interrupted during a tool call/],
    [
      { kind: "compaction_boundary", trigger: "auto", pre_tokens: 120000, post_tokens: 30000 },
      [],
      /120,000 → 30,000 tokens/,
    ],
    [{ kind: "compaction_summary" }, [text("earlier work")], /Summary of the earlier conversation/],
    [{ kind: "summary", leaf_uuid: null }, [text("a title")], /a title/],
    [
      {
        kind: "api_error",
        status: 529,
        error_type: "overloaded_error",
        retry_attempt: 2,
        max_retries: 10,
        retry_in_ms: 4000,
      },
      [],
      /retry 2 of 10 in 4\.0 s/,
    ],
    [
      {
        kind: "hook_output",
        event: "PreToolUse",
        name: "guard",
        outcome: "blocked",
        exit_code: 2,
        prevented_continuation: true,
      },
      [],
      /blocked · exit 2 · stopped the turn/,
    ],
    [{ kind: "turn_duration", message_count: 4 }, [], /Turn took 1 m 05 s/],
    [{ kind: "notice", subtype: "away_summary", level: "info" }, [text("while away")], /away summary/],
    [{ kind: "model_change", from: "model-a", to: "model-b" }, [], /model-a → model-b/],
    [{ kind: "permission_mode_change", mode: "plan" }, [], /Permission mode/],
    [{ kind: "unrecognised", record_type: "brand-new" }, [], /brand-new/],
  ];

  it.each(CASES)("%o", (kind, blocks, expected) => {
    const { container } = show(msg(kind, blocks, { duration_ms: 65_000 }));
    expect(container.textContent).toMatch(expected);
  });

  it("covers every kind the read model has", () => {
    // Read from the union's source, so a kind added there without a case
    // here fails this rather than going untested.
    const start = transcriptTypes.indexOf("type TranscriptMessageKind =");
    const union = transcriptTypes.slice(start, transcriptTypes.indexOf("\n\n", start));
    const declared = [...union.matchAll(/kind: "([a-z_]+)"/g)].map((m) => m[1]);
    expect(declared.length).toBeGreaterThan(20);
    expect(new Set(CASES.map(([k]) => k.kind))).toEqual(new Set(declared));
  });
});

describe("layout", () => {
  it("puts the user's prompt in a right-aligned bubble", () => {
    const { container } = show(msg({ kind: "user_prompt", origin: null }, [text("hello")]));
    const message = container.querySelector('[data-slot="message"]');
    expect(message?.getAttribute("data-align")).toBe("end");
    expect(within(message as HTMLElement).getByText("hello").closest('[data-slot="bubble"]')).not.toBeNull();
  });

  it("draws Claude's reply as full-width prose, with no bubble", () => {
    const { container } = show(msg({ kind: "assistant" }, [text("# Heading\n\nsome prose")]));
    expect(container.querySelector('[data-slot="bubble"]')).toBeNull();
    expect(screen.getByRole("heading", { name: "Heading" })).toBeTruthy();
  });

  it("draws a system record as a centred small-caps divider", () => {
    show(msg({ kind: "interruption", during_tool_use: false }, []));
    const note = screen.getByRole("note");
    expect(note.textContent).toBe("Interrupted");
    expect(note.className).toContain("all-small-caps");
    expect(note.parentElement?.className).toContain("text-center");
  });

  it("says an empty reply is empty rather than drawing a gap", () => {
    show(msg({ kind: "assistant" }, []));
    expect(screen.getByText("(nothing in this reply)")).toBeTruthy();
  });
});

const BASH = { tool: "bash", command: "cargo test", description: "Run the tests", truncated: false } as const;

describe("tool calls as chips", () => {
  it("shows a one-line chip, and a tap opens the whole call in a bottom sheet", async () => {
    const c = call("Bash", BASH, output({ is_error: true, text: "Exit code 1\nboom", timestamp: "2026-01-01T00:00:12.5Z" }));
    show(msg({ kind: "assistant" }, [c]));
    const chip = screen.getByRole("button", { name: "Run the tests, failed, exit 1" });
    expect(chip.getAttribute("aria-haspopup")).toBe("dialog");
    // Only the chip until tapped: the command is not on the page.
    expect(screen.queryByText("cargo test")).toBeNull();

    fireEvent.click(chip);
    const sheet = await screen.findByRole("dialog");
    expect(within(sheet).getByText("cargo test")).toBeTruthy();
    // The output is OPEN in the sheet: a fold in a sheet opens in place,
    // not as a second sheet over the first.
    expect(within(sheet).getByText(/boom/)).toBeTruthy();
    // The duration, from the call's and the result's recorded times.
    expect(within(sheet).getByText("2.5 s")).toBeTruthy();
  });

  it("says a call with no result is running only while the session runs", () => {
    const c = call("Bash", BASH, null);
    const { unmount } = show(msg({ kind: "assistant" }, [c]), { liveness: LIVE });
    expect(screen.getByRole("button", { name: /running…/ })).toBeTruthy();
    unmount();
    show(msg({ kind: "assistant" }, [c]), { liveness: DEAD });
    expect(screen.getByRole("button", { name: /no result recorded/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /running/ })).toBeNull();
  });

  it("shows no duration it could not measure", async () => {
    const c = call("Bash", BASH, output({ text: "ok", timestamp: null }));
    show(msg({ kind: "assistant" }, [c]));
    fireEvent.click(screen.getByRole("button", { name: "Run the tests" }));
    const sheet = await screen.findByRole("dialog");
    expect(within(sheet).queryByText(/\d+(\.\d)? s$|\d+ ms/)).toBeNull();
  });

  it("shows an edit's diff inline, and caps a long one behind 'Show full diff'", () => {
    const short = call(
      "Edit",
      { tool: "edit", file_path: "src/example.rs", old_string: "a", new_string: "b", replace_all: false, truncated: false },
      output({ change: RECORDED_CHANGE }),
    );
    const { unmount } = show(msg({ kind: "assistant" }, [short]));
    expect(screen.getByRole("button", { name: "Edited example.rs +2 −1" })).toBeTruthy();
    expect(screen.getByTestId("inline-diff").getAttribute("data-capped")).toBeNull();
    expect(screen.queryByRole("button", { name: /show full diff/i })).toBeNull();
    unmount();

    const lines = Array.from({ length: INLINE_DIFF_LINES + 5 }, (_, i) => ({
      op: "added" as const,
      text: `line ${i}`,
    }));
    const long = call(
      "Edit",
      { tool: "edit", file_path: "src/example.rs", old_string: "", new_string: "x", replace_all: false, truncated: false },
      output({ change: { ...RECORDED_CHANGE, hunks: [{ ...RECORDED_CHANGE.hunks[0], lines }] } }),
    );
    show(msg({ kind: "assistant" }, [long]));
    const diff = screen.getByTestId("inline-diff");
    expect(diff.getAttribute("data-capped")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Show full diff" }));
    expect(diff.getAttribute("data-capped")).toBeNull();
  });

  it("names a task update's task from the session's checklist", () => {
    const create = call(
      "TaskCreate",
      { tool: "task_create", subject: "Write the parser", description: null, active_form: null, truncated: false },
      output({ text: "Task #3 created successfully: Write the parser", tool_use_id: "c1" }),
      "c1",
    );
    const update = call(
      "TaskUpdate",
      {
        tool: "task_update",
        task_id: "3",
        status: "in_progress",
        subject: null,
        active_form: null,
        fields: ["status"],
        truncated: false,
      },
      output({ text: "Updated task #3 status", tool_use_id: "u1" }),
      "u1",
    );
    const tasks = deriveTaskChecklist([msg({ kind: "assistant" }, [create])], { truncated: false });
    show(msg({ kind: "assistant" }, [update]), { tasks });
    expect(screen.getByRole("button", { name: /Task #3 → in progress · Write the parser/ })).toBeTruthy();
  });
});

describe("thinking", () => {
  const thinking: TranscriptBlock = {
    kind: "thinking",
    index: 0,
    text: "weighing the options",
    clip: null,
    recorded: true,
  };

  it("is 'Thought for about N s', open by default and collapsible", () => {
    show(msg({ kind: "assistant" }, [thinking]), {
      thinkingStarts: new Map([["m1", "2026-01-01T00:00:00Z"]]),
    });
    const toggle = screen.getByRole("button", { name: /Thought for about 10 s/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("weighing the options")).toBeTruthy();
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("weighing the options")).toBeNull();
  });

  it("gives no duration when the start was not recorded", () => {
    show(msg({ kind: "assistant" }, [thinking]));
    expect(screen.getByRole("button", { name: /^Thought, 1 line$/ })).toBeTruthy();
  });

  it("says unrecorded thinking was not recorded, never shows it empty", () => {
    show(msg({ kind: "assistant" }, [{ ...thinking, text: "", recorded: false }]));
    expect(screen.getByText(/Thinking not recorded/)).toBeTruthy();
  });
});

describe("masked secrets (#1488)", () => {
  const MARK = "⟦hidden:api-key⟧";

  it("draws a pill in a prompt, in prose and in tool output", async () => {
    show(msg({ kind: "user_prompt", origin: null }, [text(`key is ${MARK} ok`)]));
    expect(screen.getByTitle("Hidden on this phone: an API key")).toBeTruthy();
    cleanup();

    show(msg({ kind: "assistant" }, [text(`The **key** is ${MARK}.`)]));
    expect(screen.getByTitle("Hidden on this phone: an API key")).toBeTruthy();
    expect(document.body.textContent).not.toContain("⟦");
    cleanup();

    show(msg({ kind: "assistant" }, [call("Bash", BASH, output({ text: `TOKEN=${MARK}` }))]));
    fireEvent.click(screen.getByRole("button", { name: "Run the tests" }));
    const sheet = await screen.findByRole("dialog");
    expect(within(sheet).getByTitle("Hidden on this phone: an API key")).toBeTruthy();
  });

  it("draws a pill in a fenced code block, which is then not highlighted", () => {
    show(msg({ kind: "assistant" }, [text("```ts\nconst k = \"" + MARK + "\";\n```")]));
    expect(screen.getByTitle("Hidden on this phone: an API key")).toBeTruthy();
    expect(document.body.textContent).not.toContain("⟦");
  });
});

describe("Dynamic Type (#1481)", () => {
  /// jsdom does no layout, so "does not clip" is asserted as what makes
  /// clipping impossible: the scale reaches the row as `zoom` (which
  /// takes part in layout, so text re-wraps), and nothing that carries
  /// text is truncated, kept on one line, or given a fixed height.
  it("scales each row, and nothing carrying text is truncated or fixed-height", () => {
    const LONG = "a-very-long-label-that-must-wrap-rather-than-be-cut ".repeat(4);
    const { container } = show(
      msg({ kind: "assistant" }, [
        text(LONG),
        call("Bash", { ...BASH, description: LONG }, null),
        { kind: "thinking", index: 2, text: LONG, clip: null, recorded: true },
      ]),
      { scale: 2, liveness: DEAD as Liveness },
    );
    const row = container.querySelector<HTMLElement>('[data-slot="phone-message"]');
    expect(row?.style.zoom).toBe("2");
    for (const el of container.querySelectorAll<HTMLElement>("*")) {
      const cls = typeof el.className === "string" ? el.className : "";
      expect(cls, el.outerHTML.slice(0, 80)).not.toMatch(/\btruncate\b|\bwhitespace-nowrap\b|\bline-clamp-|(^|\s)h-\d/);
    }
    // The chip's label is all there, to be wrapped.
    expect(screen.getByRole("button", { name: new RegExp(LONG.trim().slice(0, 40)) })).toBeTruthy();
  });

  it("adds no zoom at the default size", () => {
    const { container } = show(msg({ kind: "assistant" }, [text("hi")]));
    expect(container.querySelector<HTMLElement>('[data-slot="phone-message"]')?.style.zoom).toBe("");
  });
});

describe("a message being sent (#1491)", () => {
  const showPending = (over: Parameters<typeof pendingMessage>[0] = {}) =>
    render(
      <PhoneContext.Provider value={ctx()}>
        <PhonePendingMessage pending={pendingMessage(over)} />
      </PhoneContext.Provider>,
    );

  it("is the user's right-aligned bubble, drawn provisionally", () => {
    const { container } = showPending();
    const row = container.querySelector('[data-slot="phone-message"]')!;
    expect(row.getAttribute("data-kind")).toBe("pending");
    expect(row.getAttribute("data-pending-state")).toBe("pending");
    expect(container.querySelector('[data-slot="message"]')?.getAttribute("data-align")).toBe("end");
    const bubble = container.querySelector<HTMLElement>('[data-slot="bubble"]')!;
    expect(bubble.getAttribute("data-align")).toBe("end");
    // Dimmed by colour, with a dashed edge -- not faded (#1489).
    const content = bubble.querySelector<HTMLElement>('[data-slot="bubble-content"]')!;
    expect(content.className).toContain("border-dashed");
    expect(content.style.color).not.toBe("");
    expect(screen.getByRole("article", { name: "You, not in the transcript yet" })).toBe(row);
    expect(screen.getByText("please run the tests")).toBeTruthy();
  });

  it("differs from a recorded prompt with the same text", () => {
    const recorded = show(msg({ kind: "user_prompt", origin: null }, [text("please run the tests")]));
    const label = screen.getByRole("article").getAttribute("aria-label");
    const before = recorded.container.querySelector('[data-slot="bubble-content"]')!;
    const cls = before.className;
    cleanup();
    const { container } = showPending();
    expect(screen.getByRole("article").getAttribute("aria-label")).not.toBe(label);
    expect(container.querySelector('[data-slot="bubble-content"]')!.className).not.toBe(cls);
  });

  it.each(PENDING_STATES)("says what is known in state %s, beneath the bubble", (state) => {
    const { container } = showPending({
      state,
      reason: state === "failed" ? "the session has ended" : null,
    });
    const status = screen.getByRole("status");
    // Beneath the bubble, not inside its dimmed part.
    expect(container.querySelector('[data-slot="bubble"]')!.contains(status)).toBe(false);
    const expected = {
      pending: /^Sending…$/,
      delivered: /^Sent\. Not in the transcript yet\.$/,
      unconfirmed: /^Not confirmed: .*before sending it again\.$/,
      failed: /^Not sent: the session has ended\.$/,
    }[state];
    expect(status.textContent).toMatch(expected);
  });
});
