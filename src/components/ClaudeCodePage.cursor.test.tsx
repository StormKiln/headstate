import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeSession, ClaudeSessionList } from "@/types/pr";
import { useFilters } from "@/store/filters";
import { activeRowCursor, resetRowCursorForTest } from "@/lib/rowCursor";

/// #953: the sessions list is the second list the keyboard cursor walks.
///
/// Its own file rather than more cases in `ClaudeCodePage.test.tsx`,
/// which is a 2,000-line fixture set about the page's content. This is
/// about one seam -- does the column claim the cursor, and does it hand
/// back the rows that are actually DRAWN -- and it needs almost none of
/// those fixtures.
///
/// The measured case is why this list and not another: 2,561 rows on
/// this machine, each a focusable `<button>`, no roving tabindex.
/// Reaching the detail pane past it is 2,561 Tab presses.
///
/// Since #1200 the list is virtualized, so the cursor walks rows that
/// are not painted and the view scrolls to bring one in. That is what
/// the cases below about the painted window are checking.

const state = vi.hoisted(() => ({
  list: undefined as ClaudeSessionList | undefined,
  loading: false,
  failed: false,
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
      isLoading: state.loading,
      isError: state.failed,
      error: "database is locked",
      refetch: vi.fn(),
    },
    imported: { data: undefined, isError: false, isFetching: false, error: undefined },
    now: Date.parse("2026-09-13T12:00:00Z"),
    rescan: vi.fn(),
  }),
  useWorktrees: () => ({ data: undefined, isError: false, error: undefined }),
  useClaudeSessionUsage: () => ({
    data: undefined,
    isError: false,
    error: undefined,
    isLoading: false,
  }),
  useClaudeSubagentRollup: () => ({
    data: undefined,
    isError: false,
    error: undefined,
    isLoading: false,
  }),
  useClaudeTranscriptTail: () => ({
    data: undefined,
    isError: false,
    error: undefined,
    isLoading: false,
  }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../lib/clipboard", () => ({ copyText: vi.fn() }));
vi.mock("../api/tauri", () => ({ claudeRevealPath: vi.fn() }));

import { ClaudeSessionColumn } from "./ClaudeCodePage";

// Only the fields the LIST carries. #999 moved `claude_version`,
// `transcript_path`, `first_seen_at`, `transcript_state`, `resume` and
// `runs` onto `ClaudeSessionDetail`, fetched when a row is selected --
// 65% of the payload was being sent for every row so one row's detail
// pane could render. Adding them back here would typecheck only by
// widening the type, which would put them back on the wire.
const session = (n: number): ClaudeSession => ({
  session_id: `session-${String(n).padStart(4, "0")}`,
  name: `Session ${n}`,
  cwd: "/Users/acme/code/widget",
  git_branch: "feat/spoon",
  // DESCENDING, so the fixture's order is the order the column draws --
  // the list is sorted by last activity, and a fixture that ignored that
  // would let an index assertion pass against the wrong row.
  last_activity_at: new Date(Date.parse("2026-09-13T09:00:00Z") - n * 60_000).toISOString(),
  liveness: { state: "dead", why: "pid 14779 is no longer running" },
  cwd_state: { state: "exists" },
  kind: { kind: "own" },
  subagents: 0,
  // The pre-hook default (#1067, #1065). Both render nothing, so every
  // row here stays the same height -- which matters for a file about a
  // keyboard cursor walking a list.
  waiting: { state: "no", reason: "never-observed" },
  context_pressure: null,
});

const listOf = (n: number): ClaudeSessionList => ({
  sessions: Array.from({ length: n }, (_, i) => session(i)),
  registry_failure: null,
  registry_unreadable: [],
  registry_unnamed: [],
});

beforeEach(() => {
  resetRowCursorForTest();
  state.list = listOf(5);
  state.loading = false;
  state.failed = false;
  useFilters.setState({ cursor: null, claudeSelected: undefined, claudeQuery: "" });
});

afterEach(() => resetRowCursorForTest());

describe("the sessions list claims the keyboard cursor (#953)", () => {
  it("registers itself while it is mounted", () => {
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()).not.toBeNull();
    expect(activeRowCursor()?.rows()).toBe(5);
  });

  it("releases the cursor when it unmounts", () => {
    const r = render(<ClaudeSessionColumn />);
    r.unmount();
    expect(activeRowCursor()).toBeNull();
  });

  /// `Enter` on the cursor opens the SESSION at that index -- the detail
  /// pane's selection -- which is what the key does on the list beside
  /// it. Driven through the registered target rather than through a
  /// `keydown`, because the key-to-action mapping is `shortcuts.ts`'s job
  /// and is tested there; this is the wiring underneath it.
  it("opens the session at the cursor", () => {
    render(<ClaudeSessionColumn />);
    activeRowCursor()?.open(2);
    expect(useFilters.getState().claudeSelected).toBe("session-0002");
  });

  /// No bulk action on sessions, so `x` must do NOTHING here rather than
  /// the view inventing a selection with nothing to act on it. Settings'
  /// keyboard grid still says "pull request" for that key, which stays
  /// accurate because of this.
  it("offers no bulk toggle, because sessions have no bulk action", () => {
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()?.toggle).toBeUndefined();
  });

  /// The CONSEQUENCE of that, which is the half a user would notice.
  ///
  /// Before #953 the keys were wired to the PR list whatever view was on
  /// screen, so `x` on the sessions page toggled a pull request checkbox
  /// BEHIND it -- an invisible bulk selection accumulating while the user
  /// pressed a key that appeared to do nothing. Asserting the absent
  /// `toggle` alone would not catch a regression that re-pointed the key
  /// at `useFilters`' `checked`, so the store is checked directly.
  it("leaves the pull request bulk selection alone", () => {
    useFilters.setState({ checked: [], cursor: 1 });
    render(<ClaudeSessionColumn />);
    activeRowCursor()?.toggle?.(1);
    expect(useFilters.getState().checked).toEqual([]);
  });

  /// The cursor walks EVERY matched row, which is the inversion #1200
  /// brought and the reason the two tests this replaces no longer state
  /// the truth.
  ///
  /// Before virtualization this said the opposite -- "walks only the
  /// rows drawn, not the rows filtered in" -- and asserted
  /// `rows() < 260`, because rows past `RENDER_CAP = 200` were not in
  /// the DOM and could not be scrolled to. A cursor reaching index 500
  /// would have ringed nothing.
  ///
  /// That premise is gone. Virtualized, a row's being painted is a fact
  /// about where the list is scrolled, not about the row, and any row
  /// can be scrolled to. Clamping the cursor to the current screenful
  /// would make `j` stop at the bottom of the viewport, which is a worse
  /// cursor than the capped one was. So the assertion is inverted along
  /// with the behaviour, deliberately, rather than loosened.
  it("walks every matched row, not only the painted ones", () => {
    state.list = listOf(260);
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()?.rows()).toBe(260);
  });

  /// The honest counterpart, stated rather than hidden: the rows the
  /// cursor walks are NOT all in the DOM, and far fewer are painted.
  ///
  /// This is the accessibility cost of virtualizing, asserted so it
  /// cannot regress silently in either direction -- if a future change
  /// painted all 260 again the performance win would be gone, and this
  /// test would say so.
  it("paints far fewer rows than it walks", () => {
    state.list = listOf(260);
    render(<ClaudeSessionColumn />);
    const painted = screen.getAllByRole("button", { name: /Session \d/ }).length;
    expect(painted).toBeGreaterThan(0);
    expect(painted).toBeLessThan(260);
  });

  /// There is no "Show all" any more, because there is nothing left to
  /// show: every row is reachable by scrolling. A lingering button would
  /// be a control that did nothing.
  it("offers no Show all button, because nothing is withheld", () => {
    state.list = listOf(260);
    render(<ClaudeSessionColumn />);
    expect(screen.queryByRole("button", { name: /Show all/ })).toBeNull();
    expect(screen.queryByText(/showing the \d+ most recent/i)).toBeNull();
  });

  /// Read at KEYPRESS time, not captured at mount. The search box
  /// narrowing the list is the everyday way this changes under a cursor,
  /// and a registry that froze the array at first render would keep
  /// reporting the old length.
  it("reports the narrowed count after the search box shrinks the list", () => {
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()?.rows()).toBe(5);
    fireEvent.change(screen.getByLabelText(/Search Claude Code sessions/i), {
      target: { value: "Session 3" },
    });
    expect(activeRowCursor()?.rows()).toBe(1);
  });

  /// And the cursor itself is pulled back inside the narrowed list, so
  /// the ring is on a row that exists and `Enter` cannot index past the
  /// end. Clamped at render time, which is what makes it correct for
  /// DRAWING as well as for the next key press.
  it("clamps a cursor the search has left past the end", () => {
    render(<ClaudeSessionColumn />);
    useFilters.setState({ cursor: 4 });
    fireEvent.change(screen.getByLabelText(/Search Claude Code sessions/i), {
      target: { value: "Session 3" },
    });
    expect(useFilters.getState().cursor).toBe(0);
  });

  it("drops the cursor entirely when the search matches nothing", () => {
    render(<ClaudeSessionColumn />);
    useFilters.setState({ cursor: 2 });
    fireEvent.change(screen.getByLabelText(/Search Claude Code sessions/i), {
      target: { value: "nothing matches this" },
    });
    expect(useFilters.getState().cursor).toBeNull();
  });

  /// The cursor is DRAWN, or it is not a cursor. A ring rather than a
  /// background, for the reason `PrRow` gives: "a cursor that looked like
  /// a hover" is not a cursor.
  it("draws a ring on the row under the cursor, and on only that one", () => {
    useFilters.setState({ cursor: 1 });
    render(<ClaudeSessionColumn />);
    const rows = screen.getAllByRole("button", { name: /Session \d/ });
    expect(rows[1].className).toContain("ring-2");
    expect(rows[0].className).not.toContain("ring-2");
    expect(rows[2].className).not.toContain("ring-2");
  });

  /// No noise on the happy path: with no cursor set, no row wears a ring.
  /// A permanently ringed first row would read as a selection nobody
  /// made.
  it("draws no ring before any key is pressed", () => {
    render(<ClaudeSessionColumn />);
    for (const row of screen.getAllByRole("button", { name: /Session \d/ })) {
      expect(row.className).not.toContain("ring-2");
    }
  });

  /// The cursor and the SELECTION are different facts and both may be on
  /// at once: the cursor is where the next `Enter` lands, the selection is
  /// what the detail pane shows. Drawing one as the other would make them
  /// impossible to tell apart at the moment they disagree.
  it("shows the cursor and the selection as separate things", () => {
    useFilters.setState({ cursor: 0, claudeSelected: "session-0003" });
    render(<ClaudeSessionColumn />);
    const rows = screen.getAllByRole("button", { name: /Session \d/ });
    expect(rows[0].className).toContain("ring-2");
    expect(rows[0].getAttribute("aria-current")).toBeNull();
    expect(rows[3].getAttribute("aria-current")).toBe("true");
    expect(rows[3].className).not.toContain("ring-2");
  });

  /// #1200's headline requirement: the cursor reaches a row far past the
  /// painted window, and the view scrolls to it.
  ///
  /// jsdom has no layout -- every element measures 0 and there is no
  /// `ResizeObserver` -- so the scroll container is given an explicit
  /// `clientHeight` here. That is the whole reason `virtualWindow.ts` is
  /// arithmetic over an injected viewport rather than a measuring
  /// library: this assertion is possible at all because the window is
  /// computed from numbers the test can set.
  it("reaches a row far past the painted window, and scrolls to it", () => {
    state.list = listOf(1000);
    const { container } = render(<ClaudeSessionColumn />);
    const scroller = container.querySelector(".overflow-y-auto") as HTMLElement;
    Object.defineProperty(scroller, "clientHeight", { value: 900, configurable: true });

    // Row 900 is nowhere near the first screenful.
    expect(screen.queryByRole("button", { name: "Session 900" })).toBeNull();

    act(() => {
      useFilters.setState({ cursor: 900 });
    });

    // The view scrolled...
    expect(scroller.scrollTop).toBeGreaterThan(0);
    // ...and the row is now painted, reachable BY NAME -- the property
    // #1233 shipped `aria-label` to protect, which virtualization must
    // not take away from a row once it is in the DOM.
    expect(screen.getByRole("button", { name: "Session 900" })).toBeTruthy();
    // ...and it wears the ring, so the index arithmetic across the
    // window offset is right rather than merely close.
    expect(
      screen.getByRole("button", { name: "Session 900" }).className,
    ).toContain("ring-2");
  });

  /// Stepping ONE row past the bottom edge, which is the everyday case
  /// rather than the dramatic jump above. The scroll must be a nudge,
  /// not a leap: `scrollToIndex` brings the row to the near edge for the
  /// reason `rowCursor.ts` gives about not moving the eye across the
  /// whole screen for a one-row step.
  it("scrolls by a nudge when the cursor steps just past the edge", () => {
    state.list = listOf(1000);
    const { container } = render(<ClaudeSessionColumn />);
    const scroller = container.querySelector(".overflow-y-auto") as HTMLElement;
    Object.defineProperty(scroller, "clientHeight", { value: 900, configurable: true });

    act(() => {
      useFilters.setState({ cursor: 0 });
    });
    expect(scroller.scrollTop).toBe(0);

    // Walk down to the first row below the fold.
    act(() => {
      useFilters.setState({ cursor: 9 });
    });
    const nudged = scroller.scrollTop;
    // It moved, but by roughly a row -- not to row 9's offset.
    expect(nudged).toBeGreaterThan(0);
    expect(nudged).toBeLessThan(9 * 104);
  });

  /// A cursor already on screen must NOT move the list. Assigning
  /// `scrollTop` on every cursor change would yank a hand-scrolled list
  /// back under the reader whenever the cursor happened to be visible,
  /// which is why `scrollToIndex` returns null rather than the current
  /// offset.
  it("leaves the scroll alone when the cursor is already visible", () => {
    state.list = listOf(1000);
    const { container } = render(<ClaudeSessionColumn />);
    const scroller = container.querySelector(".overflow-y-auto") as HTMLElement;
    Object.defineProperty(scroller, "clientHeight", { value: 900, configurable: true });

    act(() => {
      useFilters.setState({ cursor: 1 });
    });
    expect(scroller.scrollTop).toBe(0);
  });

  /// Find-over-data reaches EVERY row, not every painted row (#1233,
  /// and the property that made #1200's tradeoff dissolve).
  ///
  /// Row 900 is not in the DOM before the search. Searching for it finds
  /// it, because the filter runs over the whole array -- which is what
  /// native `Cmd-F` could never have done, under the cap or under
  /// virtualization.
  it("finds a row far past the painted window by searching", () => {
    state.list = listOf(1000);
    render(<ClaudeSessionColumn />);
    expect(screen.queryByRole("button", { name: "Session 900" })).toBeNull();

    fireEvent.change(screen.getByLabelText(/Search Claude Code sessions/i), {
      target: { value: "Session 900" },
    });

    expect(screen.getByRole("button", { name: "Session 900" })).toBeTruthy();
    expect(activeRowCursor()?.rows()).toBe(1);
  });

  /// A list that could not be read has no rows to walk, and the keys must
  /// do nothing rather than index into an empty array. The registration
  /// still happens -- it is above the early returns, as the Rules of
  /// Hooks require -- so what is asserted is that it reports zero.
  it("walks nothing when the list could not be read", () => {
    state.failed = true;
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()?.rows()).toBe(0);
  });

  it("walks nothing while the list is still loading", () => {
    state.loading = true;
    render(<ClaudeSessionColumn />);
    expect(activeRowCursor()?.rows()).toBe(0);
  });
});
