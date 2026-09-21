import { describe, expect, it } from "vitest";
import {
  FALLBACK_VIEWPORT,
  isPainted,
  ROW_HEIGHT,
  scrollToIndex,
  virtualWindow,
} from "./virtualWindow";

/// #1200: the arithmetic that replaced `RENDER_CAP = 200`.
///
/// Tested without a DOM on purpose. The whole reason this is arithmetic
/// rather than a measuring virtualizer is that the test environment has
/// no layout -- `ResizeObserver` is undefined and every element measures
/// 0 -- so a window computed from three numbers is the one thing that
/// CAN be asserted exactly. The module's header records that reasoning.

const ROW = 100;

describe("which rows get painted", () => {
  it("paints a screenful plus overscan, not the whole list", () => {
    const w = virtualWindow(2561, 0, 1000, ROW);
    // 10 rows fit; the rest of the window is overscan.
    expect(w.start).toBe(0);
    expect(w.end).toBeGreaterThan(10);
    // The point of the change: far fewer than everything.
    expect(w.end).toBeLessThan(100);
  });

  /// The property the old cap could not offer at all: a row at index
  /// 2,000 of 2,561 is reachable, because the window FOLLOWS the scroll
  /// rather than being anchored at the top.
  it("follows the scroll offset down the list", () => {
    const w = virtualWindow(2561, 2000 * ROW, 1000, ROW);
    expect(isPainted(w, 2000)).toBe(true);
    expect(isPainted(w, 0)).toBe(false);
  });

  /// The spacers are what make the scrollbar tell the truth. Without
  /// them the list would be as tall as its painted slice and the
  /// scrollbar would claim the corpus is 30 rows long.
  it("reserves the height of the rows it did not paint", () => {
    const w = virtualWindow(1000, 500 * ROW, 1000, ROW);
    const painted = (w.end - w.start) * ROW;
    expect(w.padTop + painted + w.padBottom).toBe(1000 * ROW);
  });

  it("puts no spacer above the top of the list", () => {
    const w = virtualWindow(1000, 0, 1000, ROW);
    expect(w.padTop).toBe(0);
  });

  /// At the very bottom the window is clamped to `total`, and
  /// `padBottom` has to be computed from the clamped `end` or the list
  /// grows a phantom gap under its last row.
  it("leaves no gap under the last row", () => {
    const w = virtualWindow(1000, 1000 * ROW, 1000, ROW);
    expect(w.end).toBe(1000);
    expect(w.padBottom).toBe(0);
    expect(isPainted(w, 999)).toBe(true);
  });

  /// jsdom, and any container before its first layout, reports a height
  /// of 0. Arithmetically that means "nothing fits", and painting
  /// nothing looks exactly like a list that failed to load.
  it("paints a screenful when the container has not been measured", () => {
    const w = virtualWindow(2561, 0, 0, ROW);
    expect(w.end).toBeGreaterThan(0);
    expect(w.end).toBe(Math.ceil(FALLBACK_VIEWPORT / ROW) + 8);
  });

  /// macOS rubber-band scrolling reports a negative offset. Left
  /// unclamped it would produce a negative `start` and a negative
  /// `padTop`, which is a broken layout rather than a clamped one.
  it("survives the negative scroll offset a rubber-band produces", () => {
    const w = virtualWindow(1000, -300, 1000, ROW);
    expect(w.start).toBe(0);
    expect(w.padTop).toBe(0);
  });

  it("paints nothing when there is nothing to paint", () => {
    expect(virtualWindow(0, 0, 1000, ROW)).toEqual({
      start: 0,
      end: 0,
      padTop: 0,
      padBottom: 0,
    });
  });

  /// A guard against a divide-by-zero turning into `Infinity` rows.
  it("refuses a zero row height rather than dividing by it", () => {
    expect(virtualWindow(1000, 0, 1000, 0).end).toBe(0);
  });

  /// The overscan is why a cursor can step one row past the edge and
  /// find a row already painted. Asserted as a property rather than by
  /// pinning the constant.
  it("paints past both edges of the viewport", () => {
    const w = virtualWindow(1000, 500 * ROW, 1000, ROW);
    // The viewport covers rows 500..510. Both neighbours are painted.
    expect(isPainted(w, 499)).toBe(true);
    expect(isPainted(w, 511)).toBe(true);
  });
});

describe("bringing a row into view", () => {
  /// The distinction the return type exists for: "already visible" must
  /// not be reported as "scroll to 0", or every cursor move would yank a
  /// hand-scrolled list back to the top.
  it("says nothing is needed when the row is already visible", () => {
    expect(scrollToIndex(5, 0, 1000, ROW)).toBeNull();
  });

  it("scrolls up to the row's top edge when the row is above", () => {
    expect(scrollToIndex(2, 1000, 1000, ROW)).toBe(200);
  });

  /// The NEAR edge, not the centre. A one-row step must move the list by
  /// one row, not half a screen -- the same objection `rowCursor.ts`
  /// records against wrapping.
  it("scrolls down by exactly one row when stepping past the bottom", () => {
    // Viewport shows 0..1000 (rows 0..9). Row 10 is one row past it.
    expect(scrollToIndex(10, 0, 1000, ROW)).toBe(ROW * 11 - 1000);
  });

  it("refuses a negative index rather than scrolling somewhere odd", () => {
    expect(scrollToIndex(-1, 0, 1000, ROW)).toBeNull();
  });
});

describe("the row height constant", () => {
  /// It must OVER-estimate, never under. Under-estimating makes
  /// `padBottom` too short and the list cannot scroll to its own last
  /// row -- the one error the constant's comment says it must not make.
  ///
  /// 104px is the five-line row: 12px padding, 4px margin, five 16px
  /// line-boxes and four 2px gaps.
  it("is the tallest row, not the average", () => {
    const threeLine = 12 + 4 + 3 * 16 + 2 * 2;
    const fiveLine = 12 + 4 + 5 * 16 + 4 * 2;
    expect(ROW_HEIGHT).toBe(fiveLine);
    expect(ROW_HEIGHT).toBeGreaterThan(threeLine);
  });
});
