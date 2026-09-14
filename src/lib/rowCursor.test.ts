import { afterEach, describe, expect, it, vi } from "vitest";
import {
  activeRowCursor,
  nextCursor,
  registerRowCursor,
  resetRowCursorForTest,
  type RowCursorTarget,
} from "./rowCursor";

afterEach(() => resetRowCursorForTest());

/// The mechanism behind #953, tested without a DOM.
///
/// `App.tsx`'s keyboard block used to close over `visible` -- a
/// `PullRequest[]` -- so `j`/`k`/`Enter`/`x` worked on exactly one list of
/// ten, and on the other nine either did nothing or silently moved a PR
/// cursor behind the page on screen.
describe("the row-cursor registry", () => {
  const target = (rows: number): RowCursorTarget => ({
    rows: () => rows,
    open: vi.fn(),
  });

  /// Null is a real answer, not an oversight: nine views register
  /// nothing, and on those the cursor keys must do NOTHING rather than
  /// fall back to some other view's list -- which is the defect.
  it("starts empty, so an unregistered view claims no list", () => {
    expect(activeRowCursor()).toBeNull();
  });

  it("hands back the registered list", () => {
    const t = target(3);
    registerRowCursor(t);
    expect(activeRowCursor()?.rows()).toBe(3);
  });

  it("clears on deregistration", () => {
    const undo = registerRowCursor(target(3));
    undo();
    expect(activeRowCursor()).toBeNull();
  });

  /// ONE cursor. `filters.ts` holds a single `cursor` value and #953
  /// forbids a second: "two lists owning two cursors simultaneously would
  /// be the drift this repo refuses elsewhere."
  it("replaces rather than stacks, so only one list is ever active", () => {
    registerRowCursor(target(3));
    registerRowCursor(target(9));
    expect(activeRowCursor()?.rows()).toBe(9);
  });

  /// The ordering hazard React actually produces on a view switch: the
  /// incoming subtree mounts before the outgoing one unmounts, so the old
  /// list's cleanup runs AFTER the new list has registered. A cleanup
  /// that blanked the registry unconditionally would leave every key dead
  /// on the view the user just opened.
  it("a late cleanup from the previous list does not blank the new one", () => {
    const undoFirst = registerRowCursor(target(3));
    registerRowCursor(target(9));
    undoFirst();
    expect(activeRowCursor()?.rows()).toBe(9);
  });

  /// The stale-list hazard #953 names explicitly: "a registered list has
  /// to be read at keypress time, not captured at mount." A registry that
  /// stored a length rather than calling a function would pass every test
  /// above and still walk yesterday's rows.
  it("reads the row count when asked, not when registered", () => {
    let rows = 2;
    registerRowCursor({ rows: () => rows, open: vi.fn() });
    expect(activeRowCursor()?.rows()).toBe(2);
    rows = 40;
    expect(activeRowCursor()?.rows()).toBe(40);
  });

  /// A list with no bulk action leaves `toggle` out, and `x` must then do
  /// nothing rather than the view inventing a selection it cannot act on.
  it("tolerates a list with no bulk action", () => {
    registerRowCursor(target(3));
    expect(activeRowCursor()?.toggle).toBeUndefined();
  });
});

/// The movement rule, which lived inside a `keydown` handler and was
/// therefore neither stated once nor testable.
describe("where the cursor moves next", () => {
  /// `App.tsx`: "wrapping from the bottom back to the top silently moves
  /// the eye across the whole screen." #953 lists the clamp among the
  /// things a fix must not break.
  it("clamps at the bottom rather than wrapping to the top", () => {
    expect(nextCursor(9, 10, "next")).toBe(9);
    expect(nextCursor(8, 10, "next")).toBe(9);
  });

  it("clamps at the top rather than wrapping to the bottom", () => {
    expect(nextCursor(0, 10, "prev")).toBe(0);
    expect(nextCursor(1, 10, "prev")).toBe(0);
  });

  /// The first press of either key lands on row 0, which is what the pull
  /// request list has always done -- including `k`, so a user who reaches
  /// for the wrong key still gets a cursor rather than nothing.
  it("starts at the first row from no cursor, in either direction", () => {
    expect(nextCursor(null, 10, "next")).toBe(0);
    expect(nextCursor(null, 10, "prev")).toBe(0);
  });

  /// An empty list has no row to be on, and a cursor of 0 over zero rows
  /// would draw a ring on nothing and let `Enter` index past the end.
  it("stays absent over an empty list", () => {
    expect(nextCursor(null, 0, "next")).toBeNull();
    expect(nextCursor(3, 0, "next")).toBeNull();
  });

  /// A cursor left over from a longer list -- the search box narrowing
  /// 1,474 rows to 4 is the everyday case -- must land inside the new
  /// list rather than past its end.
  it("pulls a cursor past the end back inside the list", () => {
    expect(nextCursor(900, 4, "next")).toBe(3);
    expect(nextCursor(900, 4, "prev")).toBe(3);
  });
});
