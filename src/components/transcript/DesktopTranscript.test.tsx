/// #1480: the desktop host of the terminal renderer -- what it wires, not
/// what a row looks like (`TerminalMessage.test.tsx`). Generic fixtures.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useFilters } from "../../store/filters";
import type { TranscriptMessage, TranscriptPage } from "../../types/transcript";
import { DesktopTranscript } from "./DesktopTranscript";
import type { Liveness } from "../../types/pr";
import { call, DEAD, LIVE, liveOf, output } from "./fixtures";
import { installScrollShim, type ScrollShim } from "./scrollShim";

const state = vi.hoisted(() => ({
  pages: {} as Record<string, unknown>,
  asked: [] as { path: string | null; live: boolean }[],
}));
vi.mock("../../api/hooks", () => ({
  useClaudeTranscriptLive: (path: string | null, options: { liveness: Liveness }) => {
    const live = options.liveness.state === "running";
    state.asked.push({ path, live });
    return liveOf(path ? (state.pages[path] as TranscriptPage | undefined) : undefined, undefined, {
      status: live ? "following" : "stopped",
      lastReadAt: Date.parse("2026-01-01T10:11:12"),
    });
  },
}));
const blockText = vi.hoisted(() =>
  vi.fn((_path: string, messageId: string, index: number) =>
    Promise.resolve({ message_id: messageId, index, text: "all of it", clip: null }),
  ),
);
vi.mock("../../api/tauri", () => ({ claudeTranscriptBlockText: blockText }));
/// What the host hands pending reconciliation (#1491), per render.
const reconciled = vi.hoisted(() => ({ lists: [] as (readonly { id: string }[])[] }));
vi.mock("./usePendingMessages", async (importOriginal) => {
  const real = await importOriginal<typeof import("./usePendingMessages")>();
  return {
    ...real,
    usePendingMessages: (messages: readonly TranscriptMessage[]) => {
      reconciled.lists.push(messages);
      return real.usePendingMessages(messages);
    },
  };
});

const MAIN = "/tmp/projects/p/main.jsonl";
const SUB = "/tmp/projects/p/subagents/agent-1.jsonl";

function msg(id: string, turn: string, kind: TranscriptMessage["kind"], blocks: TranscriptMessage["blocks"]): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: turn,
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
  };
}

function page(messages: TranscriptMessage[]): TranscriptPage {
  return {
    messages,
    truncated: false,
    bytes_read: 1000,
    file_bytes: 1000,
    machinery_records: [],
    unparseable_records: 0,
    duplicate_records: 0,
  };
}

const text = (t: string, clip: { shown_chars: number; total_chars: number } | null = null) => ({
  kind: "text" as const,
  index: 0,
  text: t,
  clip,
});

let shim: ScrollShim;
beforeEach(() => {
  shim = installScrollShim();
  state.pages = {};
  state.asked = [];
  useFilters.setState({ transcriptDensity: "comfortable", transcriptShow: {} });
});
afterEach(() => {
  cleanup();
  shim.restore();
  blockText.mockClear();
});

describe("Show filters and 7.10's send path (#1484, #1490)", () => {
  const hideable = () =>
    page([
      msg("m1", "m1", { kind: "user_prompt", origin: null }, [text("added by the harness")]),
      msg("a1", "m1", { kind: "assistant" }, [{ kind: "thinking", index: 0, text: "hmm", clip: null, recorded: true }]),
    ]).messages.map((m, i) => (i === 0 ? { ...m, is_meta: true } : m));

  it("reconciles pending messages against every held message, not the filtered ones", () => {
    state.pages[MAIN] = { ...page([]), messages: hideable() };
    useFilters.setState({ transcriptShow: { system: false, thinking: false } });
    reconciled.lists = [];
    render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    expect(reconciled.lists.at(-1)?.map((m) => m.id)).toEqual(["m1", "a1"]);
  });

  it("keeps the viewer, its composer slot and pending rows mounted when filters hide every row", () => {
    state.pages[MAIN] = { ...page([]), messages: hideable() };
    useFilters.setState({ transcriptShow: { system: false, thinking: false } });
    const { container } = render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    expect(screen.getByTestId("all-hidden").textContent).toBe(
      "Everything loaded is hidden by the Show settings.",
    );
    expect(container.querySelector('[data-slot="transcript-viewer"]')).toBeTruthy();
    expect(container.querySelector('[data-slot="transcript-composer"]')).toBeTruthy();
    expect(container.querySelector('[data-message-id="m1"]')).toBeNull();
  });
});

describe("the desktop transcript", () => {
  it("renders the conversation through the terminal renderer, in a log", () => {
    state.pages[MAIN] = page([
      msg("u1", "u1", { kind: "user_prompt", origin: null }, [text("run it")]),
      msg("a1", "u1", { kind: "assistant" }, [text("Ran it.")]),
    ]);
    const { container } = render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    const log = screen.getByRole("log");
    expect(within(log).getByText("run it")).toBeTruthy();
    expect(container.querySelector('[data-kind="user_prompt"]')).toBeTruthy();
    expect(container.querySelector('[data-kind="assistant"]')).toBeTruthy();
  });

  /// "Following" is said only of a session that is running, and only
  /// then is the read re-polled.
  it("follows only a running session, and says which it is doing", () => {
    state.pages[MAIN] = page([msg("u1", "u1", { kind: "user_prompt", origin: null }, [text("x")])]);
    render(<DesktopTranscript path={MAIN} liveness={LIVE} />);
    expect(screen.getByTestId("transcript-read-status").textContent).toBe("Following. Last read at 10:11:12.");
    expect(state.asked.at(-1)).toEqual({ path: MAIN, live: true });
    cleanup();
    render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    expect(screen.getByTestId("transcript-read-status").textContent).toBe(
      "Read at 10:11:12. Not following: the session is not running.",
    );
    expect(state.asked.at(-1)).toEqual({ path: MAIN, live: false });
  });

  it("fetches a clipped block's full text from this transcript", async () => {
    state.pages[MAIN] = page([
      {
        ...msg("a1", "a1", { kind: "assistant" }, [text("all", { shown_chars: 3, total_chars: 9 })]),
        offset: 512,
      },
    ]);
    render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    fireEvent.click(screen.getByRole("button", { name: "Show all 9 characters" }));
    // With the record's offset hint, so the fetch reads one record (#1476).
    expect(blockText).toHaveBeenCalledWith(MAIN, "a1", 0, false, 512);
    await act(() => shim.flush());
    expect(screen.getByText("all of it")).toBeTruthy();
  });

  it("opens a subagent's transcript in the viewer, and goes back", () => {
    const sub = {
      agent_id: "ag1",
      status: "completed",
      agent_type: "explorer",
      transcript_path: SUB,
      transcript_found: true,
    };
    state.pages[MAIN] = page([
      msg("a1", "a1", { kind: "assistant" }, [
        call(
          "Task",
          { tool: "task", description: "look", subagent_type: "explorer", prompt: "go", truncated: false },
          output({ text: "report", subagent: sub }),
        ),
      ]),
    ]);
    state.pages[SUB] = page([msg("s1", "s1", { kind: "user_prompt", origin: null }, [text("subagent's own ask")])]);
    render(<DesktopTranscript path={MAIN} liveness={LIVE} />);
    fireEvent.click(screen.getByRole("button", { name: /open the transcript of subagent explorer/i }));

    const view = screen.getByTestId("subagent-transcript");
    expect(within(view).getByText("subagent's own ask")).toBeTruthy();
    expect(within(view).getByLabelText("Subagent transcript: explorer")).toBeTruthy();
    // A completed subagent is not followed, whatever its session is doing.
    expect(state.asked.at(-1)).toEqual({ path: SUB, live: false });

    fireEvent.click(screen.getByRole("button", { name: /back to the main transcript/i }));
    expect(screen.queryByTestId("subagent-transcript")).toBeNull();
    expect(screen.getByRole("button", { name: /open the transcript of subagent explorer/i })).toBeTruthy();
  });

  it("has a density toggle that says which is on and is remembered", () => {
    state.pages[MAIN] = page([msg("a1", "a1", { kind: "assistant" }, [text("x")])]);
    const { container } = render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    const group = screen.getByRole("group", { name: "Transcript density" });
    const compact = within(group).getByRole("button", { name: "compact" });
    expect(compact.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(compact);
    expect(compact.getAttribute("aria-pressed")).toBe("true");
    expect(useFilters.getState().transcriptDensity).toBe("compact");
    expect(container.querySelector('[data-density="compact"]')).toBeTruthy();
  });

  it("pins the session's task checklist beside the transcript when it has tasks", () => {
    state.pages[MAIN] = page([
      msg("a1", "a1", { kind: "assistant" }, [
        call(
          "TaskCreate",
          { tool: "task_create", subject: "Write the parser", description: null, active_form: null, truncated: false },
          output({ tool_use_id: "c1", task: { task_id: "1", success: null, status_from: null, status_to: null } }),
          "c1",
        ),
      ]),
    ]);
    render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    const panels = screen.getAllByRole("region", { name: "Tasks" });
    expect(panels.length).toBeGreaterThan(0);
    expect(within(panels[0]).getByText(/Write the parser/)).toBeTruthy();
  });

  it("shows no task panel for a session with no tasks", () => {
    state.pages[MAIN] = page([msg("a1", "a1", { kind: "assistant" }, [text("x")])]);
    render(<DesktopTranscript path={MAIN} liveness={DEAD} />);
    expect(screen.queryByRole("region", { name: "Tasks" })).toBeNull();
  });
});
