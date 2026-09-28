import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudeSession,
  ClaudeSessionDetail,
  ClaudeUsage,
  Liveness,
} from "../../types/pr";

/// The session header (#1485), on both builds.

const state = vi.hoisted(() => ({
  mobile: false,
  usage: {} as { data?: unknown; isError?: boolean; error?: unknown },
  usageCalls: [] as [string | null, boolean][],
  terminal: "",
}));
const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
const launchSession = vi.hoisted(() => vi.fn(() => Promise.resolve()));

vi.mock("@/lib/target", () => ({
  get IS_MOBILE_BUILD() {
    return state.mobile;
  },
  get IS_DESKTOP_BUILD() {
    return !state.mobile;
  },
}));
vi.mock("@/api/hooks", () => ({
  useClaudeSessionUsage: (path: string | null, live: boolean) => {
    state.usageCalls.push([path, live]);
    return { data: state.usage.data, isError: state.usage.isError ?? false, error: state.usage.error };
  },
  useUiPrefs: () => ({ prefs: { terminal_command: state.terminal } }),
  useWorktrees: () => ({ data: undefined }),
}));
vi.mock("@/api/tauri", () => ({
  claudeLaunchSession: launchSession,
  claudeLaunchSessionPreview: vi.fn(() => Promise.resolve({ program: "claude", args: [] })),
  claudeLaunchWorktree: vi.fn(),
  claudeLaunchWorktreePreview: vi.fn(),
  claudifyCommand: vi.fn(),
  claudeLaunchTerms: vi.fn(() => Promise.resolve({ models: [], permissionModes: [], unattended: [] })),
}));
vi.mock("@/lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const { TranscriptHeader } = await import("./TranscriptHeader");

const RUNNING: Liveness = { state: "running", pid: 4242, status: null };
const DEAD: Liveness = { state: "dead", why: "pid 4242 is no longer running" };
const KEY_ONLY: Liveness = {
  state: "unknown",
  why:
    "1 Claude Code session that did not record which session it is could be this one " +
    "(pid 4242, running in /work/app), so it cannot be called stopped",
};

function session(over: Partial<ClaudeSession> = {}): ClaudeSession {
  return {
    session_id: "s1",
    name: "Tidy the widget",
    opening_prompt: "tidy the widget",
    cwd: "/Users/someone/code/org/widget",
    git_branch: "feat/tidy",
    last_activity_at: "2026-09-26T09:00:00Z",
    liveness: DEAD,
    cwd_state: { state: "exists" },
    kind: { kind: "own" },
    subagents: 0,
    waiting: { state: "no", reason: "never-observed" },
    context_pressure: null,
    ...over,
  };
}

function detail(over: Partial<ClaudeSessionDetail> = {}): ClaudeSessionDetail {
  return {
    session_id: "s1",
    claude_version: null,
    transcript_path: "/tmp/s1.jsonl",
    first_seen_at: "2026-09-26T08:00:00Z",
    liveness: DEAD,
    transcript_state: { state: "exists" },
    resume: { command: "cd '/Users/someone/code/org/widget' && claude --resume s1", caveat: null, anchored: true },
    runs: 1,
    registry_failure: null,
    kind: { kind: "own" },
    subagents: [],
    parent: null,
    unattributed: null,
    compactions: null,
    agent_types: null,
    waiting: { state: "no", reason: "never-observed" },
    ...over,
  };
}

function usage(over: Partial<ClaudeUsage> = {}): ClaudeUsage {
  return {
    messages: 3,
    input_tokens: 1200,
    output_tokens: 45_300,
    cache_read_tokens: 2_100_000,
    cache_creation_tokens: 300,
    models: [{ model: "claude-opus-5", messages: 3 }],
    truncated: false,
    bytes_read: 1,
    file_bytes: 1,
    recorded_cost: null,
    context_floor: null,
    ...over,
  } as ClaudeUsage;
}

const NOW = Date.parse("2026-09-26T10:05:00Z");

function headerElement(
  d: Partial<ClaudeSessionDetail> = {},
  s: Partial<ClaudeSession> = {},
  props: { variant?: "desktop" | "phone"; withheld?: boolean; rollup?: boolean } = {},
) {
  return (
    <TranscriptHeader
      session={session(s)}
      detail={detail(d)}
      now={NOW}
      variant={props.variant ?? "desktop"}
      withheld={props.withheld}
      subagentRollup={props.rollup ? <p>the rollup</p> : undefined}
    />
  );
}

function renderHeader(...args: Parameters<typeof headerElement>) {
  return render(headerElement(...args));
}

beforeEach(() => {
  state.mobile = false;
  state.usage = { data: usage() };
  state.usageCalls = [];
  state.terminal = "";
  copyFn.mockClear();
  launchSession.mockClear();
});
afterEach(cleanup);

describe("TranscriptHeader liveness", () => {
  it("renders running, stopped and unknown distinctly", () => {
    const labels = [RUNNING, DEAD, KEY_ONLY].map((l) => {
      const { container, unmount } = renderHeader({ liveness: l });
      const tone = container.querySelector("[data-tone]")?.getAttribute("data-tone");
      const text = screen.getByTestId("transcript-header").textContent;
      unmount();
      return [tone, text];
    });
    expect(labels.map(([t]) => t)).toEqual(["running", "stopped", "unknown"]);
    expect(labels[0][1]).toContain("Running");
    expect(labels[1][1]).toContain("Stopped");
    expect(labels[2][1]).toContain("Could not tell whether it is running");
  });

  it("never shows an unknown liveness as stopped", () => {
    renderHeader({ liveness: { state: "unknown", why: "could not read the registry" } });
    const header = screen.getByTestId("transcript-header");
    expect(header.textContent).not.toMatch(/Stopped/);
    expect(header.textContent).toContain("could not read the registry");
  });

  it("a key-only session names its pid and folder, and is not stopped (#1315)", () => {
    renderHeader({ liveness: KEY_ONLY });
    const why = screen.getByTestId("transcript-header-liveness-why");
    expect(why.textContent).toContain("pid 4242");
    expect(why.textContent).toContain("/work/app");
    expect(screen.getByTestId("transcript-header").textContent).not.toMatch(/Stopped/);
  });

  it("running with no transcript says 'no transcript yet', not missing", () => {
    renderHeader({
      liveness: RUNNING,
      transcript_path: null,
      transcript_state: { state: "not-recorded" },
    });
    expect(screen.getByText("Running, no transcript yet")).toBeTruthy();
    // No usage read is issued for a transcript that does not exist.
    expect(state.usageCalls).toHaveLength(0);
  });
});

describe("TranscriptHeader waiting", () => {
  it("waiting for input is prominent and announced", () => {
    renderHeader({
      liveness: RUNNING,
      waiting: { state: "now", kind: "idle_prompt", at: "2026-09-26T10:00:00Z" },
    });
    expect(screen.getByRole("status").textContent).toContain("Waiting for your input");
  });

  it("waiting for permission reads differently from waiting for input", () => {
    renderHeader({
      liveness: RUNNING,
      waiting: { state: "now", kind: "permission_prompt", at: "2026-09-26T10:00:00Z" },
    });
    expect(screen.getByRole("status").textContent).toContain("Waiting for your permission");
  });

  it("'not recorded' and 'not waiting' are different sentences, and neither is announced", () => {
    renderHeader({ waiting: { state: "no", reason: "never-observed" } });
    expect(screen.getByText("Whether it is waiting is not recorded")).toBeTruthy();
    // The region is there, so starting to wait WILL be read, and empty.
    expect(screen.getByRole("status").textContent).toBe("");
    cleanup();
    renderHeader({ waiting: { state: "no", reason: "superseded" } });
    expect(screen.getByText("Not waiting for you")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("");
  });

  /// #1489: a live region that arrives already holding its text is often
  /// not read, so the one that says "waiting" is there before it waits.
  it("announces the session starting to wait from a region already mounted", () => {
    const { rerender } = renderHeader({ waiting: { state: "no", reason: "superseded" } });
    const region = screen.getByRole("status");
    rerender(
      headerElement({
        liveness: RUNNING,
        waiting: { state: "now", kind: "idle_prompt", at: "2026-09-26T10:00:00Z" },
      }),
    );
    expect(screen.getByRole("status")).toBe(region);
    expect(region.textContent).toContain("Waiting for your input");
  });
});

describe("TranscriptHeader facts", () => {
  it("a running session's usage is re-read and qualified as a floor", () => {
    renderHeader({ liveness: RUNNING });
    expect(state.usageCalls.at(-1)).toEqual(["/tmp/s1.jsonl", true]);
    expect(screen.getByTestId("transcript-header-tokens").textContent).toMatch(/^At least /);
    expect(screen.getByTestId("transcript-header-cost").textContent).toBe("Cost not recorded yet");
  });

  it("a stopped session's complete read is stated plainly", () => {
    state.usage = {
      data: usage({
        recorded_cost: {
          total_cost_usd: 1.234,
          models: [],
          total_api_ms: 0,
          total_api_without_retries_ms: 0,
          has_unknown_model_cost: false,
        } as unknown as ClaudeUsage["recorded_cost"],
      }),
    };
    renderHeader();
    expect(state.usageCalls.at(-1)).toEqual(["/tmp/s1.jsonl", false]);
    expect(screen.getByTestId("transcript-header-tokens").textContent).not.toMatch(/At least/);
    expect(screen.getByTestId("transcript-header-cost").textContent).toBe(
      "$1.23 recorded by Claude Code",
    );
    expect(screen.getByTestId("transcript-header-model").textContent).toBe("claude-opus-5");
  });

  it("a failed usage read is not zeros", () => {
    state.usage = { isError: true, error: "denied" };
    renderHeader();
    expect(screen.getByText(/Usage could not be read/)).toBeTruthy();
    expect(screen.queryByTestId("transcript-header-tokens")).toBeNull();
  });

  it("context pressure has three wordings", () => {
    const words = [true, false, null].map((p) => {
      const { unmount } = renderHeader({}, { context_pressure: p });
      const t = screen.getByTestId("transcript-header-context").textContent;
      unmount();
      return t;
    });
    expect(new Set(words).size).toBe(3);
  });

  it("the phone abbreviates the folder; the desktop does not", () => {
    renderHeader({}, {}, { variant: "phone" });
    expect(screen.getByTestId("transcript-header-cwd").textContent).toBe("~/…/org/widget");
    cleanup();
    renderHeader({}, {}, { variant: "desktop" });
    expect(screen.getByTestId("transcript-header-cwd").textContent).toBe(
      "/Users/someone/code/org/widget",
    );
  });

  it("shows elapsed time from first seen", () => {
    renderHeader({ liveness: RUNNING });
    expect(screen.getByTestId("transcript-header-elapsed").textContent).toBe(
      "2 h 5 min since first seen",
    );
  });

  it("a withheld opening prompt says transcripts are turned off for this phone", () => {
    renderHeader({}, { opening_prompt: null }, { withheld: true });
    expect(screen.getByTestId("transcript-header-withheld").textContent).toContain(
      "Transcripts are turned off for this phone on the desktop",
    );
  });
});

describe("TranscriptHeader actions", () => {
  it("desktop with no terminal copies the resume command", () => {
    renderHeader();
    fireEvent.click(screen.getByRole("button", { name: "Copy resume command" }));
    expect(copyFn).toHaveBeenCalledWith(detail().resume.command);
  });

  it("desktop with a terminal shows the argv before resuming, then launches", () => {
    state.terminal = "iterm";
    renderHeader({ liveness: KEY_ONLY });
    expect(launchSession).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Resume in terminal" }));
    // The unknown caveat rides along: it may already be open.
    expect(screen.getByText(/may already be open somewhere/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(launchSession).toHaveBeenCalledWith("s1", "/Users/someone/code/org/widget", {});
  });

  it("a running session is not offered a resume on either build", () => {
    renderHeader({ liveness: RUNNING });
    expect(screen.queryByRole("button", { name: /resume/i })).toBeNull();
    cleanup();
    state.mobile = true;
    renderHeader({ liveness: RUNNING });
    expect(screen.queryByRole("button", { name: /resume/i })).toBeNull();
  });

  it("the phone build copies the resume command and offers no terminal", () => {
    state.mobile = true;
    state.terminal = "iterm";
    renderHeader();
    expect(screen.queryByRole("button", { name: "Resume in terminal" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Claudify/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Copy resume command" }));
    expect(copyFn).toHaveBeenCalledWith(detail().resume.command);
  });

  it("opens the subagent rollup in place", () => {
    renderHeader(
      { subagents: [{ session_id: "c1", name: "Child", agent_id: "a1" }] as ClaudeSessionDetail["subagents"] },
      {},
      { rollup: true },
    );
    expect(screen.queryByText("the rollup")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Subagents (1)" }));
    expect(within(screen.getByTestId("transcript-header-rollup")).getByText("the rollup")).toBeTruthy();
  });

  it("makes no send-capability claim in 7.9", () => {
    renderHeader({ liveness: RUNNING });
    expect(screen.getByTestId("transcript-header").textContent).not.toMatch(/reply|send/i);
  });
});
