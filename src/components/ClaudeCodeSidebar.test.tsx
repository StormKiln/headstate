import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeSession, ClaudeSessionList } from "@/types/pr";
import { useFilters } from "@/store/filters";
import { stubViewport } from "@/test-utils";

/// Mocked away for the reason `DockerSidebar.test.tsx` gives: the switcher
/// is every view's labels and a `useUiPrefs` read, and none of that is what
/// this column's own claims are about. With it gone, the buttons this file
/// counts are exactly the ones `ClaudeCodeSidebar` puts there itself.
vi.mock("./ViewSwitcher", () => ({ ViewSwitcher: () => null }));

const refetchFn = vi.hoisted(() => vi.fn());
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));

const state = vi.hoisted(() => ({
  list: undefined as ClaudeSessionList | undefined,
  failed: false,
  /// FIXED, never `Date.now()`: the rows carry relative dates, and reading
  /// the clock during render is what `ClaudeCodePage`'s purity test forbids.
  now: Date.parse("2026-09-13T12:00:00Z"),
}));

vi.mock("../api/hooks", () => ({
  // #1280's reverse lookup. `off` -- nothing typed here is a pull
  // request reference -- which is what every assertion in this file
  // assumes; `ClaudeCodePage.test.tsx` is where the other states are
  // exercised.
  useClaudeSessionsForPrQuery: () => ({ state: "off" }),
  useClaudeSessions: () => ({
    list: {
      data: state.list,
      isLoading: false,
      isError: state.failed,
      error: "database is locked",
      refetch: refetchFn,
    },
    imported: { data: undefined, isError: false, isFetching: false, error: undefined },
    now: state.now,
    rescan: rescanFn,
  }),
  useWorktrees: () => ({ data: [], isError: false, error: undefined }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../lib/clipboard", () => ({ copyText: vi.fn(() => Promise.resolve(null)) }));
vi.mock("../api/tauri", () => ({ claudeRevealPath: vi.fn() }));

import { CLAUDE_PAGES, ClaudeCodeSidebar } from "./ClaudeCodeSidebar";

/// One LIST row. The detail half is not built here: this file renders
/// `ClaudeSessionColumn`, which since #985 receives only what the list
/// carries -- so a fixture with a resume command in it would describe a
/// payload this component never sees.
const session = (over: Partial<ClaudeSession> = {}): ClaudeSession => ({
  session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
  name: "HeadState GitHub issues filing",
  cwd: "/Users/acme/code/widget",
  git_branch: "feat/spoon",
  last_activity_at: "2026-09-13T09:00:00Z",
  liveness: { state: "dead", why: "pid 14779 is no longer running" },
  cwd_state: { state: "exists" },
  kind: { kind: "own" },
  subagents: 0,
  // The pre-hook default (#1067, #1065): no notification record has ever
  // arrived, and no compaction was ever recorded. Both render nothing, so
  // the rows these tests count and order are unchanged.
  waiting: { state: "no", reason: "never-observed" },
  context_pressure: null,
  ...over,
});

const listOf = (sessions: ClaudeSession[]): ClaudeSessionList => ({
  sessions,
  registry_failure: null,
  registry_unreadable: [],
});

beforeEach(() => {
  state.list = listOf([session()]);
  state.failed = false;
  stubViewport(1400);
  // The REAL store: `claudePage` is what this column both reads and writes,
  // and a mock would let the sidebar's selection and the store's default
  // drift apart while every assertion here still passed. Reset explicitly
  // because the store is a module singleton shared across these tests.
  useFilters.setState({
    view: "claude-code",
    claudePage: "overview",
    claudeQuery: "",
    claudeSelected: undefined,
  });
});

afterEach(() => {
  cleanup();
  stubViewport(null);
});

describe("the page order, which #939 reversed", () => {
  /// **The ordering claim, asserted as an ORDER and not as presence.**
  ///
  /// #921 had `sessions` first. #939 swapped them because the overview
  /// carries the resurrection tiles and the resumable list, which is what a
  /// user acts on -- and because the sessions list is no longer behind the
  /// overview at all, it is in this column.
  ///
  /// Two `getByRole` calls asserting both exist would pass under either
  /// order, which is exactly the defect: the claim is about POSITION. So
  /// this reads the rendered buttons in document order and pins the first
  /// one. Swap `CLAUDE_PAGES` back and it fails, naming "Sessions".
  it("renders Overview as the first page row", () => {
    render(<ClaudeCodeSidebar />);
    const labels = screen
      .getAllByRole("button")
      .map((b) => b.textContent?.trim())
      .filter((t): t is string => t !== undefined && t.length > 0);

    expect(labels[0]).toBe("Overview");
    expect(labels[1]).toBe("Sessions");
  });

  /// The SOURCE list, in case the rendering ever stops iterating it. The
  /// array is exported precisely so there is one order, so the order in the
  /// array is a claim in its own right.
  it("lists overview first in CLAUDE_PAGES itself", () => {
    // Plugins last (#1075): it answers an occasional question, and its
    // scan is the most expensive thing behind any of these pages.
    expect(CLAUDE_PAGES.map((p) => p.id)).toEqual(["overview", "sessions", "plugins"]);
  });

  /// The store has to agree with the column, or where you land depends on
  /// how you arrived. Both the cold-launch default and `setView`'s re-entry
  /// reset are checked, because they are two separate writes of one
  /// decision.
  it("defaults the store to the page the column lists first", () => {
    expect(useFilters.getState().claudePage).toBe(CLAUDE_PAGES[0]?.id);

    useFilters.getState().setClaudePage("sessions");
    useFilters.getState().setView("my-prs");
    useFilters.getState().setView("claude-code");
    expect(useFilters.getState().claudePage).toBe(CLAUDE_PAGES[0]?.id);
  });

  /// `aria-current="page"`, not `aria-pressed`: these are navigation, and a
  /// screen reader announcing "pressed" for the page you are already on
  /// describes a control that did something rather than a location.
  it("marks the open page with aria-current and offers no toggle", () => {
    render(<ClaudeCodeSidebar />);
    expect(
      screen.getByRole("button", { name: /overview/i }).getAttribute("aria-current"),
    ).toBe("page");
    expect(
      screen.getByRole("button", { name: /^sessions$/i }).getAttribute("aria-current"),
    ).toBeNull();
    // No toggle semantics anywhere in the column.
    expect(document.querySelector("[aria-pressed]")).toBeNull();
  });

  /// The Plugins row navigates (#1075).
  ///
  /// `src/CLAUDE.md`'s rule that a component and its host can land in
  /// different PRs, applied to a page: a sidebar row that sets a
  /// `claudePage` nothing routes on highlights itself and renders the
  /// overview, which looks like a broken page rather than a missing one.
  it("navigates to the plugins page", () => {
    render(<ClaudeCodeSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /plugins/i }));
    expect(useFilters.getState().claudePage).toBe("plugins");
    expect(
      screen.getByRole("button", { name: /plugins/i }).getAttribute("aria-current"),
    ).toBe("page");
  });
});

describe("the session list, which #939 moved into this column", () => {
  /// The rows are the `Sessions` page's own content, so they appear when
  /// that page is open and not beneath a selected `Overview` -- which would
  /// have the column answering a question the user navigated away from.
  it("shows no rows while Overview is the open page", () => {
    render(<ClaudeCodeSidebar />);
    expect(screen.queryByLabelText(/search claude code sessions/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /HeadState GitHub issues filing/i })).toBeNull();
  });

  it("shows the search box and the rows once Sessions is open", () => {
    useFilters.setState({ claudePage: "sessions" });
    render(<ClaudeCodeSidebar />);
    expect(screen.getByLabelText(/search claude code sessions/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /HeadState GitHub issues filing/i })).toBeTruthy();
  });

  /// **The search guard.** Typing in the sidebar's box narrows the sidebar's
  /// rows.
  ///
  /// Break the filter in `useMatchedSessions` -- return `all` instead of
  /// `hits`, or drop `s.name` from the four fields -- and this fails,
  /// naming the row that should have gone.
  it("filters the rows from the box above them", () => {
    useFilters.setState({ claudePage: "sessions" });
    state.list = listOf([
      session(),
      session({
        session_id: "aaaaaaaa-0000-0000-0000-000000000000",
        name: "Notarization plumbing",
      }),
    ]);
    render(<ClaudeCodeSidebar />);

    fireEvent.change(screen.getByLabelText(/search claude code sessions/i), {
      target: { value: "notarization" },
    });

    expect(screen.getByRole("button", { name: /notarization plumbing/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /HeadState GitHub issues filing/i })).toBeNull();
    // The "N of M match" count, which is a DIFFERENT fact from the render
    // cap's footer and has to survive the move with the rows.
    expect(screen.getByText(/1 of 2 match/)).toBeTruthy();
  });

  /// Search covers all four fields, each for a stated reason: the title
  /// because `aiTitle` names 1,436 of 1,438 sessions, the directory and
  /// branch because 286 titles are shared with a sibling, and the id for
  /// pasting one in from somewhere else.
  it("searches the title, the directory, the branch and the id", () => {
    useFilters.setState({ claudePage: "sessions" });
    render(<ClaudeCodeSidebar />);
    const box = screen.getByLabelText(/search claude code sessions/i);
    const row = /HeadState GitHub issues filing/i;

    for (const term of ["headstate", "code/widget", "feat/spoon", "e5dff3bd"]) {
      fireEvent.change(box, { target: { value: term } });
      expect(screen.queryByRole("button", { name: row })).toBeTruthy();
    }

    fireEvent.change(box, { target: { value: "nothing matches this" } });
    expect(screen.queryByRole("button", { name: row })).toBeNull();
  });

  /// #1200 added `<mark>` highlighting inside the row, and the first
  /// version of it BROKE this: wrapping matched runs in `<mark>` and
  /// `<span>` split the label into several text nodes, the computed
  /// accessible name came out EMPTY, and every row stopped being
  /// reachable by name -- a screen reader announced nothing at all.
  ///
  /// Caught by the two search tests above, which is why the name is now
  /// stated with `aria-label` rather than computed from the children.
  /// This asserts the property directly, so a future change that drops
  /// the attribute fails HERE, where the reason is written down, rather
  /// than showing up as a confusing failure in a search test.
  it("names each row independently of how the search highlights it", () => {
    useFilters.setState({ claudePage: "sessions" });
    render(<ClaudeCodeSidebar />);
    const box = screen.getByLabelText(/search claude code sessions/i);
    const row = /HeadState GitHub issues filing/i;

    // The name is the same with a search active, with a search that
    // matches the name itself, and with none.
    for (const term of ["", "headstate", "github", "nothing"]) {
      fireEvent.change(box, { target: { value: term } });
      const found = screen.queryAllByRole("button", { name: row });
      if (term === "nothing") {
        expect(found).toHaveLength(0);
      } else {
        expect(found.length, `row unreachable by name while searching "${term}"`).toBeGreaterThan(0);
      }
    }
  });

  /// Clicking a row writes the id the main panel reads, which is the whole
  /// point of the state living in the store: the row is in this column and
  /// the detail is in another.
  it("selects a session into the store the main panel reads", () => {
    useFilters.setState({ claudePage: "sessions" });
    render(<ClaudeCodeSidebar />);
    fireEvent.click(screen.getByRole("button", { name: /HeadState GitHub issues filing/i }));
    expect(useFilters.getState().claudeSelected).toBe("e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2");
  });

  /// #846 in this column too: a rejected read must say so, and must NEVER
  /// come out as "no sessions". The error arm is before the empty arm and
  /// there is no `= []` default, so with `list.data` undefined the empty
  /// branch is unreachable.
  it("says a failed read failed rather than claiming there are none", () => {
    useFilters.setState({ claudePage: "sessions" });
    state.list = undefined;
    state.failed = true;
    render(<ClaudeCodeSidebar />);
    expect(screen.getByText(/could not read the claude code sessions/i)).toBeTruthy();
    expect(screen.queryByText(/no claude code sessions/i)).toBeNull();
  });

  /// The phone answer, from this side. Below `MOBILE_BREAKPOINT` this
  /// column is a `Sheet` that closes on navigation, so the list is NOT here
  /// -- `ClaudeCodePage` mounts it in the main panel instead, which
  /// `ClaudeCodePage.mobile.test.tsx` asserts. Both halves are needed: a
  /// list in both places at a phone width would be two search boxes over
  /// two copies of the same rows.
  it("leaves the list out of the sheet at a phone width", () => {
    useFilters.setState({ claudePage: "sessions" });
    stubViewport(390);
    render(<ClaudeCodeSidebar />);
    expect(screen.getByRole("button", { name: /^sessions$/i })).toBeTruthy();
    expect(screen.queryByLabelText(/search claude code sessions/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /HeadState GitHub issues filing/i })).toBeNull();
  });
});
