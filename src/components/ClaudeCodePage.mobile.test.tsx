import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeSession, ClaudeSessionList } from "@/types/pr";

/// The companion OFFERS this view and hides only what cannot work (#922).
///
/// `MOBILE_HIDDEN_VIEWS` stays empty on purpose. This view answers "did
/// the thing on my laptop die?", which is precisely a question asked away
/// from the laptop, so hiding the view would remove the companion's best
/// reason to exist.
///
/// What must be hidden is narrower: the two `claude_reveal_path` buttons.
/// `surfaceGuard.test.ts` already proves that wrapper is `Class::Local`
/// and that the phone may not call it -- but that is a check on the
/// ALLOWLIST, not on what gets rendered. A page that renders a button the
/// allowlist refuses still ships a control that can only fail, and the
/// allowlist test passes the whole time. This file is the other half: it
/// renders the page as the phone build and asserts on the DOM.
///
/// Run under the mobile build by mocking `@/lib/target`. `IS_MOBILE_BUILD`
/// is a Vite `define` that folds to a literal, so there is no environment
/// variable to set at test time -- see `lib/target.ts`.
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/code/app")));
const refetchFn = vi.hoisted(() => vi.fn());
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));

const state = vi.hoisted(() => ({
  list: undefined as ClaudeSessionList | undefined,
  now: Date.parse("2026-09-13T12:00:00Z"),
}));

vi.mock("../api/hooks", () => ({
  useClaudeSessions: () => ({
    list: {
      data: state.list,
      isLoading: false,
      isError: false,
      error: undefined,
      refetch: refetchFn,
    },
    imported: { data: undefined, isError: false, isFetching: false, error: undefined },
    now: state.now,
    rescan: rescanFn,
  }),
  useWorktrees: () => ({ data: undefined, isError: false, error: undefined }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("../api/tauri", () => ({ claudeRevealPath: revealFn }));

const { ClaudeCodePage } = await import("./ClaudeCodePage");

const session = (over: Partial<ClaudeSession> = {}): ClaudeSession => ({
  session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
  name: "HeadState GitHub issues filing",
  cwd: "/Users/acme/code/widget",
  git_branch: "feat/spoon",
  claude_version: "2.1.270",
  transcript_path: "/Users/acme/.claude/projects/slug/e5dff3bd.jsonl",
  first_seen_at: "2026-09-11T09:00:00Z",
  last_activity_at: "2026-09-13T09:00:00Z",
  liveness: { state: "dead", why: "pid 14779 is no longer running" },
  cwd_state: { state: "exists" },
  transcript_state: { state: "exists" },
  resume: {
    command: "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
    caveat: null,
    anchored: true,
  },
  runs: 1,
  ...over,
});

const listOf = (sessions: ClaudeSession[]): ClaudeSessionList => ({
  sessions,
  registry_failure: null,
  registry_unreadable: [],
});

beforeEach(() => {
  state.list = listOf([session()]);
  copyFn.mockClear();
  revealFn.mockClear();
});

function open(name: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(name, "i") }));
}

describe("the companion offers the view and hides only the Local actions", () => {
  /// **The sabotage test.** Drop either `!IS_MOBILE_BUILD` guard around
  /// the `RevealButton`s in `ClaudeCodePage` and this fails, naming the
  /// button. Nothing else catches it: `surfaceGuard.test.ts` checks the
  /// allowlist and stays green, and every other test in this directory
  /// runs as the desktop build, where the buttons SHOULD be there.
  it("renders neither reveal button, because claude_reveal_path is Class::Local", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");

    expect(screen.queryByRole("button", { name: /reveal directory/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /reveal transcript/i })).toBeNull();
  });

  /// The phone must never CALL it either, which is a different claim from
  /// not rendering it: a keyboard shortcut or an effect could reach the
  /// wrapper with no button on screen.
  it("never calls claudeRevealPath", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");
    expect(revealFn).not.toHaveBeenCalled();
  });

  /// The view itself is OFFERED. This is the half that would be lost if
  /// someone "fixed" the hidden buttons by hiding the page, so it is
  /// asserted rather than assumed: the session list, its liveness, and
  /// the detail pane all render on the phone.
  it("still renders the session list and its liveness", () => {
    render(<ClaudeCodePage />);
    expect(screen.getAllByText(/not running/i).length).toBeGreaterThan(0);
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/pid 14779 is no longer running/i)).toBeTruthy();
  });

  /// The view says WHOSE sessions these are. A phone showing a session
  /// list with no such line reads as "this phone's sessions", which is
  /// never true -- the companion runs none.
  it("says the sessions belong to the paired desktop", () => {
    render(<ClaudeCodePage />);
    expect(screen.getByText(/paired desktop's Claude Code sessions/i)).toBeTruthy();
  });

  /// The resume command STAYS, and the issue asked for this to be decided
  /// deliberately rather than by omission.
  ///
  /// Kept, because the command is SHOWN as text and not only copied. The
  /// phone's clipboard cannot reach a desktop shell, but a user reading
  /// "did it die?" in bed can read the exact line that will bring it back
  /// and type it in the morning -- and the `cd` prefix is the part that
  /// stops it landing in the wrong tree (#918). Hiding it would remove
  /// the answer while keeping the question. This is the same reasoning
  /// `remote/surface.rs` gives for classing `claude_sessions` as `Read`
  /// with its resume command in the payload.
  it("still shows the resume command, including its cd prefix", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");

    expect(
      screen.getByText(
        "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
      ),
    ).toBeTruthy();
  });

  /// Copying is harmless and kept: `copyText` writes to the phone's own
  /// clipboard and forwards nothing to the desktop, so it is not a
  /// `Class::Local` call at all.
  it("still copies to the phone's own clipboard", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");

    fireEvent.click(screen.getByRole("button", { name: /copy resume command/i }));
    expect(copyFn).toHaveBeenCalledOnce();
  });
});
