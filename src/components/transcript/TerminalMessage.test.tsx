/// #1480: the desktop terminal renderer, one DOM test per record kind and
/// one per wiring the tool renderers (#1483) asked of it. Generic
/// fixtures only: the privacy guard scans this file.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Liveness } from "../../types/pr";
import type { TranscriptMessage } from "../../types/transcript";
import { call, DEAD, LIVE, output, PENDING_STATES, pendingMessage } from "./fixtures";
import { deriveTaskChecklist } from "./tasks";
import { TerminalMessage, TerminalPendingMessage, type TerminalEnv } from "./TerminalMessage";
import type { TurnFooter } from "./turnFooter";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
vi.mock("@/lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));

afterEach(() => {
  cleanup();
  copyFn.mockClear();
  toastSuccess.mockClear();
});

function msg(
  kind: TranscriptMessage["kind"],
  blocks: TranscriptMessage["blocks"] = [],
  over: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return {
    id: "m1",
    id_source: "uuid",
    turn_id: "u1",
    kind,
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

const text = (t: string, index = 0, clip: { shown_chars: number; total_chars: number } | null = null) => ({
  kind: "text" as const,
  index,
  text: t,
  clip,
});

function env(over: Partial<TerminalEnv> = {}): TerminalEnv {
  return { liveness: DEAD, density: "comfortable", messages: () => [], ...over };
}

function show(
  m: TranscriptMessage,
  e: TerminalEnv = env(),
  footer?: TurnFooter,
  tasks?: ReturnType<typeof deriveTaskChecklist>,
) {
  return render(<TerminalMessage message={m} footer={footer} env={e} tasks={tasks} />);
}

const BASH = { tool: "bash", command: "cargo test --lib", description: null, truncated: false } as const;

describe("the user's turn", () => {
  it("is a band with an accent bar, a > glyph and the time on the right", () => {
    const { container } = show(
      msg({ kind: "user_prompt", origin: null }, [text("fix the parser")], {
        id: "u1",
        turn_id: "u1",
        timestamp: "2026-01-02T03:04:05Z",
      }),
    );
    const row = container.querySelector('[data-slot="message"]')!;
    expect(row.getAttribute("data-kind")).toBe("user_prompt");
    // No bubble: the desktop is one left-aligned column.
    expect(row.getAttribute("data-align")).toBe("start");
    const band = container.querySelector('[data-slot="message-content"]') as HTMLElement;
    expect(band.className).toContain("border-l-2");
    expect(band.style.borderColor).toBe("rgb(217, 119, 87)");
    const glyph = within(band).getByText(">");
    expect(glyph.getAttribute("aria-hidden")).toBe("true");
    const time = container.querySelector("time")!;
    expect(time.getAttribute("dateTime")).toBe("2026-01-02T03:04:05Z");
    expect(time.closest('[data-slot="message-header"]')).toBeTruthy();
    expect(screen.getByText("fix the parser")).toBeTruthy();
    // The opener copies its whole turn; a message copies itself.
    expect(screen.getByRole("button", { name: "Copy turn as markdown" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy message as markdown" })).toBeTruthy();
  });

  it("shows no time it was not given", () => {
    const { container } = show(msg({ kind: "user_prompt", origin: null }, [text("hi")], { id: "u1" }));
    expect(container.querySelector("time")).toBeNull();
  });

  it("offers turn copy only on the message that opens the turn", () => {
    show(msg({ kind: "queued_prompt", mode: null }, [text("and then this")], { id: "q1", turn_id: "u1" }));
    expect(screen.getByText("You (queued)")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Copy turn as markdown" })).toBeNull();
  });

  it("draws a shell command with the ! glyph, in monospace", () => {
    show(msg({ kind: "shell_input", command: "git status" }, [], { id: "s1", turn_id: "s1" }));
    expect(screen.getByText("!")).toBeTruthy();
    expect(screen.getByText("git status").className).toContain("font-mono");
  });

  it("wraps a long unbroken line rather than widening the page", () => {
    const long = "x".repeat(4000);
    show(msg({ kind: "user_prompt", origin: null }, [text(long)], { id: "u1" }));
    expect(screen.getByText(long).className).toContain("[overflow-wrap:anywhere]");
  });

  it("draws text the desktop masked as a pill, never as the marker", () => {
    show(msg({ kind: "user_prompt", origin: null }, [text("key is ⟦hidden:api-key⟧ ok")], { id: "u1" }));
    expect(screen.getByTitle("Hidden on this phone: an API key")).toBeTruthy();
    expect(document.body.textContent).not.toContain("⟦hidden");
  });

  it("renders a Claude Code meta prompt as a divider, not as the user speaking", () => {
    show(msg({ kind: "user_prompt", origin: null }, [text("caveat text")], { is_meta: true }));
    expect(screen.getByRole("note", { name: "Added by Claude Code" })).toBeTruthy();
    expect(screen.queryByText("You")).toBeNull();
  });
});

describe("Claude's reply", () => {
  it("is a bullet and markdown", () => {
    const { container } = show(msg({ kind: "assistant" }, [text("It **passed**.")]));
    expect(screen.getByText("⏺").getAttribute("aria-hidden")).toBe("true");
    expect(container.querySelector("strong")?.textContent).toBe("passed");
  });

  it("says an empty message is empty", () => {
    show(msg({ kind: "assistant" }, []));
    expect(screen.getByText("(nothing in this message)")).toBeTruthy();
  });

  it("shows thinking by default, dimmed, behind a label that folds it", () => {
    show(
      msg({ kind: "assistant" }, [
        { kind: "thinking", index: 0, text: "weighing it", clip: null, recorded: true },
      ]),
    );
    const toggle = screen.getByRole("button", { name: /thinking/i });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("weighing it").className).toContain("italic");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("weighing it")).toBeNull();
  });

  it("says thinking that was not recorded, rather than an empty thought", () => {
    show(
      msg({ kind: "assistant" }, [{ kind: "thinking", index: 0, text: "", clip: null, recorded: false }]),
    );
    expect(screen.getByText("✻ Thinking (not recorded)")).toBeTruthy();
  });

  it("names an image and a block it cannot draw rather than dropping them", () => {
    show(
      msg({ kind: "assistant" }, [
        { kind: "image", index: 0, image: { media_type: "image/png", approx_bytes: 4096, width: 10, height: 20 } },
        { kind: "other", index: 1, block_type: "server_tool_use" },
      ]),
    );
    expect(screen.getByText(/\[image, image\/png, 10×20, about 4 KB: not shown\]/)).toBeTruthy();
    expect(screen.getByText("[a server_tool_use block, not shown]")).toBeTruthy();
  });

  it("fetches a clipped block's full text through the host's callback, by its address", async () => {
    const load = vi.fn(() =>
      Promise.resolve({ message_id: "m1", index: 3, text: "the whole thing", clip: null }),
    );
    show(
      msg({ kind: "assistant" }, [text("the wh", 3, { shown_chars: 6, total_chars: 15 })], {
        offset: 2048,
      }),
      env({ onLoadFullText: load }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Show all 15 characters" }));
    // With the record's offset, so the fetch reads one record (#1476).
    expect(load).toHaveBeenCalledWith({ messageId: "m1", index: 3, offset: 2048 });
    await waitFor(() => expect(screen.getByText("the whole thing")).toBeTruthy());
  });
});

describe("tool calls", () => {
  it("are the terminal tool renderer, timed from the call's record to its result's", () => {
    show(
      msg(
        { kind: "assistant" },
        [call("Bash", BASH, output({ text: "ok", timestamp: "2026-01-01T00:00:02.500Z" }))],
        { timestamp: "2026-01-01T00:00:00Z" },
      ),
    );
    const g = screen.getByRole("group", { name: /Bash/ });
    expect(g.className).toContain("font-mono");
    expect(within(g).getByText("2.5 s")).toBeTruthy();
  });

  it("show no duration when either end was not recorded", () => {
    show(msg({ kind: "assistant" }, [call("Bash", BASH, output({ text: "ok" }))], { timestamp: "2026-01-01T00:00:00Z" }));
    expect(screen.queryByText(/\d s$|\d ms$/)).toBeNull();
  });

  /// Only a live session's unanswered call is running.
  it.each<[string, Liveness, string]>([
    ["running", LIVE, "running"],
    ["dead", DEAD, "not_recorded"],
  ])("say a call with no result is %s only as the session's liveness allows", (_, liveness, state) => {
    show(msg({ kind: "assistant" }, [call("Bash", BASH, null)]), env({ liveness }));
    expect(screen.getByRole("group", { name: /Bash/ }).getAttribute("data-state")).toBe(state);
  });

  it("open a subagent's transcript through the host's callback", () => {
    const open = vi.fn();
    const sub = {
      agent_id: "ag1",
      status: "completed",
      agent_type: "explorer",
      transcript_path: "/tmp/projects/p/sub.jsonl",
      transcript_found: true,
    };
    show(
      msg({ kind: "assistant" }, [
        call(
          "Task",
          { tool: "task", description: "look around", subagent_type: "explorer", prompt: "go", truncated: false },
          output({ text: "report", subagent: sub }),
        ),
      ]),
      env({ onOpenSubagent: open }),
    );
    fireEvent.click(screen.getByRole("button", { name: /open the transcript of subagent explorer/i }));
    expect(open).toHaveBeenCalledWith(sub);
  });

  it("name an updated task from the session's checklist", () => {
    const create = msg({ kind: "assistant" }, [
      call(
        "TaskCreate",
        { tool: "task_create", subject: "Write the parser", description: null, active_form: null, truncated: false },
        output({ tool_use_id: "c1", task: { task_id: "3", success: null, status_from: null, status_to: null } }),
        "c1",
      ),
    ]);
    const update = msg(
      { kind: "assistant" },
      [
        call(
          "TaskUpdate",
          { tool: "task_update", task_id: "3", status: "in_progress", subject: null, active_form: null, fields: ["status"], truncated: false },
          output({ tool_use_id: "u9", task: { task_id: "3", success: true, status_from: "pending", status_to: "in_progress" } }),
          "u9",
        ),
      ],
      { id: "m2" },
    );
    const tasks = deriveTaskChecklist([create, update], { truncated: false });
    show(update, env(), undefined, tasks);
    expect(screen.getByRole("group", { name: "TaskUpdate: #3 → in progress · Write the parser" })).toBeTruthy();
  });

  it("draw a result whose call is in an earlier part as that, not as a missing call", () => {
    show(msg({ kind: "tool_results" }, [{ kind: "tool_result", ...output({ text: "late" }) }]));
    expect(screen.getByText("Result of a call in an earlier part of the transcript.")).toBeTruthy();
  });
});

/// Every record that is not the conversation is a thin labelled divider:
/// shown by default, and never nothing.
describe("dividers", () => {
  it.each<[TranscriptMessage["kind"], string]>([
    [{ kind: "slash_command", name: "/review" }, "/review"],
    [{ kind: "interruption", during_tool_use: false }, "Interrupted by user"],
    [{ kind: "compaction_boundary", trigger: "manual", pre_tokens: null, post_tokens: null }, "Conversation compacted (manual)"],
    [{ kind: "compaction_summary" }, "Summary of the compacted conversation"],
    [{ kind: "summary", leaf_uuid: null }, "Session summary"],
    [{ kind: "model_change", from: "model-a", to: "model-b" }, "Model changed to model-b (from model-a)"],
    [{ kind: "permission_mode_change", mode: "plan" }, "Permission mode: plan"],
    [
      { kind: "hook_output", event: "PreToolUse", name: "guard", outcome: "blocked", exit_code: 2, prevented_continuation: null },
      "PreToolUse hook · guard · blocked · exit 2",
    ],
    [
      { kind: "api_error", status: 529, error_type: "overloaded", retry_attempt: 2, max_retries: 10, retry_in_ms: 5000 },
      "API error 529 (overloaded) · retry 2 of 10 · in 5.0 s",
    ],
    [{ kind: "notice", subtype: "local_command", level: "info" }, "local command (info)"],
    [{ kind: "agent_notification", task_id: null, status: "completed" }, "Background task notification: completed"],
    [{ kind: "injected", origin: "hook" }, "Added to the conversation by hook"],
    [{ kind: "command_output", command: "/cost" }, "Output of /cost"],
    [{ kind: "turn_duration", message_count: 3 }, "Turn complete"],
    [{ kind: "unrecognised", record_type: "brand-new" }, "Unrecognised record: brand-new"],
  ])("%j is a divider labelled %s", (kind, label) => {
    const { container } = show(msg(kind));
    expect(screen.getByRole("note", { name: label })).toBeTruthy();
    expect(container.querySelector('[data-slot="message"]')?.getAttribute("data-kind")).toBe(kind.kind);
  });

  it("puts short text on the rule and folds long text with its line count", () => {
    show(msg({ kind: "summary", leaf_uuid: null }, [text("Fixed the parser")]));
    expect(screen.getByRole("note", { name: "Session summary" }).textContent).toContain(": Fixed the parser");
    cleanup();
    show(msg({ kind: "compaction_summary" }, [text("one\ntwo\nthree")]));
    const fold = screen.getByRole("button", { name: "Summary of the compacted conversation, 3 lines" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
  });

  it("colours an API error AND says it in words", () => {
    show(msg({ kind: "api_error", status: 500, error_type: null, retry_attempt: null, max_retries: null, retry_in_ms: null }));
    const label = screen.getByText("API error 500");
    expect(label.getAttribute("style")).toContain("rgb(248, 81, 73)");
  });

  it("draws a background task's state with its own row", () => {
    show(msg({ kind: "task_status", task_id: "t1", task_type: "local_bash", status: "running" }, [text("build")]));
    expect(screen.getByText(/Background bash task/)).toBeTruthy();
  });
});

describe("the turn footer", () => {
  const footer: TurnFooter = {
    outputTokens: 1234,
    tokensPartial: false,
    durationMs: 63_000,
    durationSource: "recorded",
    models: ["model-a"],
    qualifier: null,
  };

  it("gives tokens, duration and model, muted", () => {
    show(msg({ kind: "assistant" }, [text("done")]), env(), footer);
    const f = screen.getByTestId("turn-footer");
    expect(f.getAttribute("data-slot")).toBe("message-footer");
    expect(f.textContent).toBe("1 m 03 s·1,234 output tokens·model-a");
    expect(f.getAttribute("style")).toContain("rgb(139, 148, 158)");
  });

  it("qualifies a floor and says why", () => {
    show(
      msg({ kind: "assistant" }, [text("done")]),
      env(),
      { ...footer, tokensPartial: true, durationMs: null, qualifier: "began_above" },
    );
    expect(screen.getByTestId("turn-footer").textContent).toBe(
      "at least 1,234 output tokens·model-a·this turn began before what was read",
    );
  });

  it("is what a turn_duration record that hosts it draws, instead of a divider", () => {
    show(msg({ kind: "turn_duration", message_count: 2 }, [], { duration_ms: 1000 }), env(), footer);
    expect(screen.getByTestId("turn-footer")).toBeTruthy();
    expect(screen.queryByRole("note")).toBeNull();
  });
});

describe("copy", () => {
  it("copies a message as markdown, and says it did", async () => {
    show(msg({ kind: "assistant" }, [text("All **green**.")]));
    fireEvent.click(screen.getByRole("button", { name: "Copy message as markdown" }));
    expect(copyFn).toHaveBeenCalledWith("All **green**.");
    await waitFor(() => expect(toastSuccess).toHaveBeenCalledWith("Copied the message as markdown"));
  });

  it("copies a whole turn from the messages the viewer holds", () => {
    const opener = msg({ kind: "user_prompt", origin: null }, [text("ask")], { id: "u1", turn_id: "u1" });
    const reply = msg({ kind: "assistant" }, [text("answer")], { id: "a1", turn_id: "u1" });
    show(opener, env({ messages: () => [opener, reply] }));
    fireEvent.click(screen.getByRole("button", { name: "Copy turn as markdown" }));
    expect(copyFn).toHaveBeenCalledWith("**You**\n\n> ask\n\nanswer\n");
  });
});

describe("density", () => {
  it("packs rows tighter when compact", () => {
    const { container, rerender } = show(msg({ kind: "assistant" }, [text("a")]));
    const row = () => container.querySelector('[data-slot="message"]')!;
    expect(row().getAttribute("data-density")).toBe("comfortable");
    expect(row().className).toContain("gap-1.5");
    rerender(<TerminalMessage message={msg({ kind: "assistant" }, [text("a")])} footer={undefined} env={env({ density: "compact" })} />);
    expect(row().getAttribute("data-density")).toBe("compact");
    expect(row().className).toContain("gap-0.5");
  });
});

describe("a message being sent (#1491)", () => {
  const showPending = (over: Parameters<typeof pendingMessage>[0] = {}) =>
    render(<TerminalPendingMessage pending={pendingMessage(over)} density="comfortable" />);

  it("is the user's band, drawn provisionally: dashed, no time, nothing to copy", () => {
    const { container } = showPending();
    const row = container.querySelector('[data-slot="message"]')!;
    expect(row.getAttribute("data-kind")).toBe("pending");
    expect(row.getAttribute("data-pending-state")).toBe("pending");
    const band = container.querySelector('[data-slot="message-content"]') as HTMLElement;
    expect(band.className).toContain("border-l-2");
    expect(band.className).toContain("border-dashed");
    expect(screen.getByText("please run the tests")).toBeTruthy();
    expect(container.querySelector("time")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("differs from a recorded prompt with the same text", () => {
    const recorded = show(
      msg({ kind: "user_prompt", origin: null }, [text("please run the tests")], { id: "u1" }),
    );
    const bandClass = (
      recorded.container.querySelector('[data-slot="message-content"]') as HTMLElement
    ).className;
    cleanup();
    const { container } = showPending();
    const band = container.querySelector('[data-slot="message-content"]') as HTMLElement;
    expect(band.className).not.toBe(bandClass);
  });

  it.each(PENDING_STATES)("says what is known in state %s", (state) => {
    showPending({ state, reason: state === "failed" || state === "unconfirmed" ? "no answer" : null });
    const expected = {
      pending: /^Sending…$/,
      delivered: /^Sent\. Not in the transcript yet\.$/,
      unconfirmed: /^Not confirmed: .*\(no answer\).*before sending it again\.$/,
      failed: /^Not sent: no answer\.$/,
    }[state];
    expect(screen.getByRole("status").textContent).toMatch(expected);
  });

  it("shows the text as typed, never as markdown", () => {
    showPending({ text: "**not bold**" });
    expect(screen.getByText("**not bold**")).toBeTruthy();
  });
});
