/// Which slice of a long list is worth painting (#1200).
///
/// # The measurement this exists for
///
/// The sessions list is the longest in the app. On this machine the
/// corpus is 2,561 transcripts, and painting all of them measured:
///
/// | rows painted | expand | DOM nodes |
/// |---|---|---|
/// | 200 (the old `RENDER_CAP`) | 53 ms | 3,418 |
/// | 600 | 125 ms | 10,215 |
/// | 1,200 | 303 ms | 20,415 |
/// | 2,561 | 627 ms | 43,552 |
///
/// 627 ms is a visibly frozen window, and it is the cost of the one
/// control -- "Show all" -- that the cap offered as its own remedy. The
/// cap was honest about hiding rows; what it could not do is make the
/// rest cheap to ask for.
///
/// # Why this is not a virtualization library
///
/// `@tanstack/react-virtual` is the obvious candidate: 61 KB, one
/// transitive dependency, and the TanStack family is already here
/// through `react-query`. It was rejected on a property of the TEST
/// environment rather than of the library.
///
/// It measures. So does every other virtualizer worth using -- that is
/// what makes them able to handle rows of differing height -- and it
/// measures through `ResizeObserver` and `getBoundingClientRect`. In
/// jsdom, which is what all 2,185 of this project's tests run in:
///
/// - `ResizeObserver` is `undefined`
/// - `IntersectionObserver` is `undefined`
/// - `clientHeight`, `offsetHeight` and `getBoundingClientRect().height`
///   are all `0`, for every element, always
/// - `Element.prototype.scrollTo` and `scrollIntoView` do not exist
///
/// A measuring virtualizer in that environment computes a viewport of
/// zero and therefore a window of zero rows. Every existing test that
/// asserts on a session row -- and there are four files of them -- would
/// stop finding any row at all. The fix would be a global
/// `ResizeObserver` polyfill plus stubbed metrics in a setup file the
/// project does not currently have, which is a lot of test-only
/// scaffolding standing between the suite and the thing it asserts.
///
/// So the window is computed ARITHMETICALLY from three numbers -- scroll
/// offset, viewport height, row height -- and the viewport is supplied by
/// the caller rather than measured here. That is the same shape
/// `coalesce.ts` uses for its scheduler and for the same stated reason:
/// injected so the behaviour is deterministic in a test, rather than
/// depending on an environment detail the test cannot drive.
///
/// The cost of the choice is stated plainly: rows must be a KNOWN
/// height, and this cannot handle rows that size themselves. The
/// sessions row is not fixed-height -- it grows a line for an opening
/// prompt and another for a waiting badge -- so `ROW_HEIGHT` below is an
/// over-estimate and the overscan absorbs the error. See its comment.
///
/// # Which lists use this, and which deliberately do not
///
/// The sessions list only. The other two long lists were measured at
/// their real size on this machine and left alone, because a
/// virtualizer is not free -- it costs a fixed row height, a scroll
/// container that owns its own height, and the accessibility tradeoff
/// below -- and none of that buys anything at a few hundred rows.
///
/// | list | rows at real size | render | verdict |
/// |---|---|---|---|
/// | sessions | 2,561 | 627 ms uncapped | virtualized |
/// | artifacts | 178 | 60 ms | left alone |
/// | worktrees | 8 (largest repo) | trivial | left alone |
///
/// The worktree number is the one worth writing down, because "~295
/// worktrees on this machine" is true and misleading: `WorktreesPage`
/// renders ONE REPOSITORY at a time -- its own empty state says "No
/// worktrees in this repository" -- so the 295 is a machine-wide total
/// that is never painted at once. The largest single repository here
/// has 8. Virtualizing that would be machinery guarding nothing.
///
/// The artifacts list does paint all its rows, and at 178 rows and 60 ms
/// that is under two frames. If either list grows an order of magnitude
/// the measurement is the thing to redo, not this comment.

/// The slice to paint, and the space to leave around it.
export interface VirtualWindow {
  /// First index to render, inclusive.
  start: number;
  /// Last index to render, EXCLUSIVE -- a `slice` bound.
  end: number;
  /// Pixels of spacer above `start`, so the painted rows sit where the
  /// unpainted ones would have put them.
  padTop: number;
  /// Pixels of spacer below `end`, so the scrollbar spans the whole
  /// list rather than only the painted part.
  padBottom: number;
}

/// How many rows to paint beyond each edge of the viewport.
///
/// Not a performance knob -- a CORRECTNESS one, twice over.
///
/// The rows are not truly fixed-height (see `ROW_HEIGHT`), so the
/// arithmetic here can be wrong by a row or two over a screenful. An
/// overscan wide enough to cover that error means the wrongness shows up
/// as slightly-too-many rows painted rather than as a gap at the edge of
/// the viewport, which is what the user would actually see.
///
/// It also gives the keyboard cursor somewhere to land. Arrowing one row
/// past the bottom edge must not require a scroll event to have been
/// processed before the row exists to be focused.
const OVERSCAN = 8;

/// Which rows to paint.
///
/// `total` is the whole list, NOT a pre-capped slice: this is the
/// replacement for the cap, so it must be handed everything.
///
/// A `viewportHeight` of 0 -- which is every element in jsdom, and also
/// a real container before its first layout -- would arithmetically mean
/// "no rows fit", and painting nothing is the wrong answer to a question
/// asked before the browser has laid anything out. `FALLBACK_VIEWPORT`
/// is used instead, so an unmeasured container paints a screenful rather
/// than a blank.
export function virtualWindow(
  total: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
): VirtualWindow {
  if (total <= 0 || rowHeight <= 0) {
    return { start: 0, end: 0, padTop: 0, padBottom: 0 };
  }

  // An unlaid-out container asks for a screenful rather than for
  // nothing. Painting zero rows because nothing has been measured yet
  // is indistinguishable, to a reader, from a list that failed to load.
  const height = viewportHeight > 0 ? viewportHeight : FALLBACK_VIEWPORT;
  // Negative scroll is real: macOS rubber-band scrolling reports it.
  const top = Math.max(0, scrollTop);

  const first = Math.floor(top / rowHeight);
  const visible = Math.ceil(height / rowHeight);

  const start = Math.max(0, first - OVERSCAN);
  const end = Math.min(total, first + visible + OVERSCAN);

  return {
    start,
    end,
    padTop: start * rowHeight,
    // From `end`, not from `start + count`: those differ at the tail,
    // where `end` is clamped to `total`, and using the wrong one would
    // leave a phantom gap under the last row.
    padBottom: Math.max(0, (total - end) * rowHeight),
  };
}

/// What to assume a container is tall before anything has measured it.
///
/// Roughly a laptop window's list column. The exact number does not
/// matter much -- it decides only how much is painted on the very first
/// frame, before a scroll event or a layout effect has supplied the real
/// height -- but it must be large enough to fill a real viewport, or the
/// first paint would show a short list above blank space.
export const FALLBACK_VIEWPORT = 900;

/// The assumed height of one session row, in pixels.
///
/// An OVER-estimate on purpose, and the reason is the honest weakness of
/// a fixed-height virtualizer over rows that are not fixed-height. A
/// session row is:
///
/// - title line, always
/// - opening prompt, when the session recorded one
/// - liveness and date, always
/// - waiting/context-pressure line, only when either is set
/// - directory, always
///
/// so a row is three lines or five depending on the session. Estimating
/// LOW would make `padBottom` too short and the list would scroll past
/// its own end; estimating high overshoots the scrollable height
/// slightly, which reads as a little extra space under the last row and
/// costs nothing else. Between a list that cannot reach its last row and
/// one with a small tail gap, the tail gap is obviously right.
///
/// Derived from the row's own classes rather than guessed. `py-1.5` is
/// 6px twice, `mb-1` is 4px, `gap-0.5` is 2px per seam, and each line is
/// a 16px line-box (`text-xs` is 12px/16px; the 11px lines round to the
/// same box):
///
/// | row | arithmetic | height |
/// |---|---|---|
/// | three-line | 12 + 4 + 3x16 + 2x2 | 68px |
/// | five-line | 12 + 4 + 5x16 + 4x2 | 104px |
///
/// So 104 -- the TALLER of the two, not the midpoint. A midpoint would
/// under-estimate every five-line row, which is the one error this
/// constant must not make.
export const ROW_HEIGHT = 104;

/// Whether an index is inside a window -- i.e. currently painted.
///
/// Exists so the cursor's scroll-into-view can ask the question in the
/// same vocabulary the renderer answers it, rather than re-deriving the
/// bounds at the call site and drifting from them.
export function isPainted(w: VirtualWindow, index: number): boolean {
  return index >= w.start && index < w.end;
}

/// The scroll offset that brings `index` into view, or `null` when it
/// already is.
///
/// `null` rather than the current offset is the load-bearing part: the
/// caller must be able to tell "no scroll needed" from "scroll to 0",
/// because assigning `scrollTop` unconditionally on every cursor move
/// would yank a list the user had scrolled by hand back under them
/// whenever the cursor happened to be visible already.
///
/// The row is brought to the NEAR edge -- the top when scrolling up, the
/// bottom when scrolling down -- rather than centred. Centring moves the
/// eye by half a screen for a one-row step, which is the same objection
/// `rowCursor.ts` records against wrapping the cursor.
export function scrollToIndex(
  index: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
): number | null {
  if (index < 0 || rowHeight <= 0) return null;
  const height = viewportHeight > 0 ? viewportHeight : FALLBACK_VIEWPORT;

  const rowTop = index * rowHeight;
  const rowBottom = rowTop + rowHeight;

  if (rowTop < scrollTop) return rowTop;
  if (rowBottom > scrollTop + height) return rowBottom - height;
  return null;
}
