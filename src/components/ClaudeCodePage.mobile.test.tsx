import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudePreview,
  ClaudeSession,
  ClaudeSessionDetail,
  ClaudeSessionList,
  ClaudeUsage,
} from "@/types/pr";
import { useFilters } from "@/store/filters";
import { stubViewport } from "@/test-utils";

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
///
/// And at a PHONE VIEWPORT, as of #939, which is a second thing and now a
/// necessary one. `IS_MOBILE_BUILD` says which build this is;
/// `useIsMobile()` says whether the narrow LAYOUT is on, and jsdom has no
/// `matchMedia`, so every test in this file previously read as desktop
/// width while claiming to be the phone. That was harmless while both
/// widths drew the same single component. It is not harmless now: #939
/// moved the session list into `ClaudeCodeSidebar` on the desktop and left
/// `ClaudeCodePage` mounting it itself on the phone, so the two widths
/// render different trees and only the narrow one is this file's subject.
/// `stubViewport` below is what makes these assertions about the phone.
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/code/app")));
const refetchFn = vi.hoisted(() => vi.fn());
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));

const state = vi.hoisted(() => ({
  list: undefined as ClaudeSessionList | undefined,
  now: Date.parse("2026-09-13T12:00:00Z"),
  /// #959 and #982. Both commands are `Class::Read`, so unlike the two
  /// reveal buttons the phone DOES get them -- and the preview is the one
  /// Claude action whose phone case is stronger than the desktop's, since
  /// `claude_reveal_path` is `Local` and there is otherwise no path to a
  /// transcript's content at all. Filled here so the tests below can
  /// assert that, rather than only that the Local controls are gone.
  usage: undefined as ClaudeUsage | undefined,
  preview: undefined as ClaudePreview | undefined,
  /// One session's detail, keyed by id (#985). `Class::Read`, so the
  /// phone gets this too -- and the phone is who the split is for: the
  /// list crosses the pairing transport every ten seconds and was
  /// carrying every session's resume command to render one.
  details: new Map<string, ClaudeSessionDetail>(),
  /// Every id the detail hook was asked for while enabled, so the tests
  /// below can assert the phone fetches ONE.
  detailAskedFor: [] as string[],
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
  useClaudeSessionUsage: (path: string | null) => ({
    data: state.usage,
    isError: false,
    error: undefined,
    isLoading: path !== null && state.usage === undefined,
  }),
  useClaudeSessionDetail: (sessionId: string | null, enabled: boolean) => {
    if (enabled && sessionId) state.detailAskedFor.push(sessionId);
    return {
      data: sessionId ? state.details.get(sessionId) : undefined,
      isError: false,
      error: undefined,
      refetch: refetchFn,
    };
  },
  useClaudeTranscriptTail: (path: string | null, enabled: boolean) => ({
    data: state.preview,
    isError: false,
    error: undefined,
    isLoading: enabled && path !== null && state.preview === undefined,
  }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("../api/tauri", () => ({ claudeRevealPath: revealFn }));

const { ClaudeCodePage } = await import("./ClaudeCodePage");

/// One session as a whole, split across the two tiers #985 introduced.
///
/// Returns the LIST row and files the detail under the same id, so the
/// phone's two reads answer for one session and these tests keep reading
/// as statements about a session rather than about a wire format.
const session = (over: Partial<ClaudeSession & ClaudeSessionDetail> = {}): ClaudeSession => {
  const id = over.session_id ?? "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2";
  const liveness = over.liveness ?? {
    state: "dead" as const,
    why: "pid 14779 is no longer running",
  };
  state.details.set(id, {
    session_id: id,
    claude_version: "2.1.270",
    transcript_path: "/Users/acme/.claude/projects/slug/e5dff3bd.jsonl",
    first_seen_at: "2026-09-11T09:00:00Z",
    liveness,
    transcript_state: { state: "exists" },
    resume: {
      command: `cd '/Users/acme/code/widget' && claude --resume ${id}`,
      caveat: null,
      anchored: true,
    },
    runs: 1,
    registry_failure: null,
    ...over,
  });
  return {
    session_id: id,
    name: "HeadState GitHub issues filing",
    cwd: "/Users/acme/code/widget",
    git_branch: "feat/spoon",
    last_activity_at: "2026-09-13T09:00:00Z",
    liveness,
    cwd_state: { state: "exists" },
    ...over,
  };
};

const listOf = (sessions: ClaudeSession[]): ClaudeSessionList => ({
  sessions,
  registry_failure: null,
  registry_unreadable: [],
});

beforeEach(() => {
  state.details.clear();
  state.detailAskedFor = [];
  state.list = listOf([session()]);
  state.usage = {
    messages: 994,
    input_tokens: 1_988,
    output_tokens: 582_035,
    cache_read_tokens: 405_086_242,
    cache_creation_tokens: 4_971_059,
    models: [{ model: "claude-opus-5", messages: 994 }],
    truncated: false,
    bytes_read: 183_237,
    file_bytes: 183_237,
  };
  state.preview = {
    messages: [
      {
        role: "assistant",
        timestamp: "2026-09-13T11:00:05Z",
        model: "claude-opus-5",
        blocks: [{ kind: "text", text: "Running the tests now.", truncated: false }],
      },
    ],
    truncated: false,
    bytes_read: 183_237,
    file_bytes: 183_237,
    non_conversation_records: 0,
    unparseable_records: 0,
  };
  // 390px: an iPhone 15's CSS width, comfortably under `MOBILE_BREAKPOINT`.
  stubViewport(390);
  // The search text and the selection live in the store since #939, and it
  // is a module singleton -- so a selection made by one test would open a
  // detail screen in the next one before it clicked anything, which on the
  // phone means the LIST is the thing that is hidden.
  useFilters.setState({ claudeQuery: "", claudeSelected: undefined });
  copyFn.mockClear();
  revealFn.mockClear();
});

afterEach(() => {
  stubViewport(null);
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

  /// **The #939 mobile guard.** The search box is REACHABLE on the phone,
  /// in the main panel, without opening the navigation sheet.
  ///
  /// This is the objection `ClaudeCodeSidebar`'s doc comment used to raise
  /// against putting the box in that column at all -- "on a phone this
  /// column is a sheet that closes on navigation, which would take the
  /// search with it" -- and it is answered by the list having a second
  /// mount point here rather than by the objection being deleted. Move the
  /// `isMobile` mount in `ClaudeCodePage` behind `!isMobile`, or drop it,
  /// and this fails: the phone would be left with a search box only
  /// reachable from behind a hamburger that closes when you tap a result.
  ///
  /// `ClaudeCodePage` alone, with no sidebar rendered, which is the point:
  /// on the phone the sidebar is inside a closed `Sheet` and contributes
  /// nothing to the screen.
  it("puts the search box in the main panel, not behind the navigation sheet", () => {
    render(<ClaudeCodePage />);
    expect(screen.getByLabelText(/search claude code sessions/i)).toBeTruthy();
  });

  /// And searching from there narrows the rows. Reaching the box is not the
  /// same claim as the box working at this width.
  it("filters the phone's rows from that search box", () => {
    state.list = listOf([
      session(),
      session({
        session_id: "aaaaaaaa-0000-0000-0000-000000000000",
        name: "Notarization plumbing",
      }),
    ]);
    render(<ClaudeCodePage />);
    expect(screen.getByRole("button", { name: /notarization plumbing/i })).toBeTruthy();

    fireEvent.change(screen.getByLabelText(/search claude code sessions/i), {
      target: { value: "notarization" },
    });

    expect(screen.getByRole("button", { name: /notarization plumbing/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /HeadState GitHub issues filing/i })).toBeNull();
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

  /// #959 on the phone. `claude_session_usage` is `Class::Read`, so
  /// unlike the two reveal buttons this one is NOT behind
  /// `IS_MOBILE_BUILD` -- and the assertion is the positive half, which
  /// `surfaceGuard.test.ts` cannot make: that test checks the allowlist
  /// and would stay green whether the page rendered this or not.
  ///
  /// The question it answers is the away-from-desk one: "was that the
  /// long session or the typo" is how the row worth resuming is picked,
  /// and the phone is where the picking happens.
  it("shows how much work a session did, because claude_session_usage is Class::Read", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");

    expect(screen.getByText(/how much work it did/i)).toBeTruthy();
    expect(screen.getByText("994")).toBeTruthy();
    expect(screen.getByText("582,035")).toBeTruthy();
  });

  /// #982 on the phone, and this is the strongest case in the set.
  ///
  /// `claude_reveal_path` is `Class::Local` and its buttons are asserted
  /// absent above, so WITHOUT this a companion user could see that a
  /// session died and not one word of what it was doing. The desktop user
  /// can `cat` the file; the phone cannot reach the machine at all.
  ///
  /// **Sabotage:** wrap `<TranscriptPreview>` in `!IS_MOBILE_BUILD` in
  /// `ClaudeCodePage` and this fails while every other test stays green.
  it("lets the phone read a transcript, because claude_transcript_tail is Class::Read", () => {
    render(<ClaudeCodePage />);
    open("HeadState GitHub issues filing");

    // Behind the disclosure on the phone as on the desktop: a 256 KB read
    // over the pairing transport is exactly what must not happen on every
    // selection.
    expect(screen.getByRole("button", { name: /read the transcript/i })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText("Running the tests now.")).toBeTruthy();
  });

  /// The viewport stub is load-bearing and is asserted rather than
  /// trusted. A test that mocks `IS_MOBILE_BUILD` but not `matchMedia`
  /// runs at DESKTOP width while claiming to be a phone -- and since #939
  /// the two widths render different trees, so every assertion above
  /// would be about the wrong one.
  it("really is running at a phone viewport", () => {
    // `matchMedia` is what `stubViewport(390)` replaces and what
    // `useIsMobile()` reads, so it -- not `window.innerWidth`, which jsdom
    // leaves at its own 1024 default -- is the thing that decides which
    // tree renders. Asserting the stub actually bit, at the breakpoint the
    // page uses and at one above it, is what stops this file silently
    // becoming a second desktop suite.
    expect(window.matchMedia("(max-width: 767px)").matches).toBe(true);
    expect(window.matchMedia("(max-width: 389px)").matches).toBe(false);
    // And the narrow tree is the one on screen: on the phone the list and
    // the detail are two SCREENS, so opening a session hides the list.
    render(<ClaudeCodePage />);
    expect(screen.getByRole("button", { name: /HeadState GitHub issues filing/i })).toBeTruthy();
    open("HeadState GitHub issues filing");
    expect(screen.getByRole("button", { name: /all sessions/i })).toBeTruthy();
  });
});
