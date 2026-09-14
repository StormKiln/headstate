import { fireEvent, render, screen } from "@testing-library/react";
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
/// The measured case is why this list and not another: ~1,474 rows with
/// "Show all" pressed, each a focusable `<button>`, no roving tabindex.
/// Reaching the detail pane past it is ~1,474 Tab presses.

const state = vi.hoisted(() => ({
  list: undefined as ClaudeSessionList | undefined,
  loading: false,
  failed: false,
}));

vi.mock("../api/hooks", () => ({
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

const session = (n: number): ClaudeSession => ({
  session_id: `session-${String(n).padStart(4, "0")}`,
  name: `Session ${n}`,
  cwd: "/Users/acme/code/widget",
  git_branch: "feat/spoon",
  claude_version: "2.1.270",
  transcript_path: `/Users/acme/.claude/projects/slug/${n}.jsonl`,
  first_seen_at: "2026-09-11T09:00:00Z",
  // DESCENDING, so the fixture's order is the order the column draws --
  // the list is sorted by last activity, and a fixture that ignored that
  // would let an index assertion pass against the wrong row.
  last_activity_at: new Date(Date.parse("2026-09-13T09:00:00Z") - n * 60_000).toISOString(),
  liveness: { state: "dead", why: "pid 14779 is no longer running" },
  cwd_state: { state: "exists" },
  transcript_state: { state: "exists" },
  resume: { command: "claude --resume", caveat: null, anchored: true },
  runs: 1,
});

const listOf = (n: number): ClaudeSessionList => ({
  sessions: Array.from({ length: n }, (_, i) => session(i)),
  registry_failure: null,
  registry_unreadable: [],
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

  /// The rows the cursor walks must be the rows on SCREEN.
  ///
  /// The column caps its rendering and offers "Show all"; below that cap
  /// the remaining rows are not in the DOM at all. A cursor that could
  /// reach index 500 of 1,474 would highlight nothing and `Enter` would
  /// open a session the reader cannot see -- which is the stale-list
  /// hazard #953 names, in its other form.
  it("walks only the rows drawn, not the rows filtered in", () => {
    state.list = listOf(260);
    render(<ClaudeSessionColumn />);
    // RENDER_CAP is 200. The count is read from the registry rather than
    // restated, so this tracks the cap rather than pinning a number the
    // component owns.
    const walked = activeRowCursor()?.rows() ?? 0;
    expect(walked).toBeLessThan(260);
    expect(screen.getAllByRole("button", { name: /Session \d/ }).length).toBe(walked);
  });

  it("walks all of them once Show all is pressed", () => {
    state.list = listOf(260);
    render(<ClaudeSessionColumn />);
    fireEvent.click(screen.getByRole("button", { name: /Show all/ }));
    expect(activeRowCursor()?.rows()).toBe(260);
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
