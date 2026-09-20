import { useCallback, useLayoutEffect, useRef, useState } from "react";
import {
  scrollToIndex,
  virtualWindow,
  type VirtualWindow,
} from "./virtualWindow";

/// The React half of `virtualWindow.ts`; the arithmetic, and the reason
/// it is arithmetic rather than a measuring library, is there.
///
/// # What this owns
///
/// Two numbers -- how far the container is scrolled and how tall it is --
/// and the effect that keeps a keyboard cursor's row on screen. The
/// window itself is COMPUTED DURING RENDER from those numbers, never
/// stored: a window in state would be a second copy of something
/// derivable, and keeping it current would mean `setState` in an effect,
/// which this project forbids and which would paint one stale frame
/// every time.
///
/// # Why the viewport is state and not a ref
///
/// Because a change to it must re-render -- it changes which rows are
/// painted. A ref would update silently and the list would keep drawing
/// the window it computed for the old height. It is written only from
/// the scroll handler and the resize observer-substitute below, both of
/// which already run outside render.
export interface VirtualList {
  /// The slice to paint, recomputed every render.
  window: VirtualWindow;
  /// Attach to the scrolling container.
  ref: React.RefObject<HTMLDivElement | null>;
  /// Attach to the container's `onScroll`.
  onScroll: () => void;
}

export function useVirtualList(
  total: number,
  rowHeight: number,
  /// The row to keep on screen -- the keyboard cursor. `null` when
  /// there is no cursor, which must NOT be read as "row 0": scrolling to
  /// the top because nobody has pressed a key would be a list that
  /// fights the user.
  cursor: number | null,
): VirtualList {
  const ref = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);

  // Read straight off the element rather than from the event, so the
  // same function can be called from the layout effect below where
  // there is no event.
  const sync = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    // Both at once: a window resize changes the height without a
    // scroll event, and a scroll can change both when the list shrinks
    // under it.
    setScrollTop(el.scrollTop);
    setViewport(el.clientHeight);
  }, []);

  // The container's height on first layout.
  //
  // A LAYOUT effect rather than a passive one: this runs before the
  // browser paints, so the first frame the user sees already has the
  // real viewport rather than `FALLBACK_VIEWPORT`. With a passive
  // effect the list would paint a fallback-sized window and then
  // immediately repaint, which is a visible flicker on every mount.
  //
  // This is the one place a `setState` happens from an effect, and it is
  // the case the rule exists to permit: it reads a value that ONLY the
  // DOM knows and that cannot be computed during render. It is also
  // self-limiting -- `setViewport` with an unchanged number is a no-op
  // in React, so this cannot loop.
  useLayoutEffect(() => {
    sync();
  }, [sync]);

  // Keep the cursor's row on screen.
  //
  // In an effect because it WRITES to the DOM (`scrollTop`), which is a
  // side effect by definition and must not happen during render.
  // Layout, again, so the scroll lands in the same frame as the row
  // being drawn under it rather than a frame later.
  //
  // `scrollToIndex` returning `null` for "already visible" is what keeps
  // this from yanking a hand-scrolled list: with no cursor movement, or
  // with the cursor already painted, nothing is assigned at all.
  useLayoutEffect(() => {
    if (cursor === null) return;
    const el = ref.current;
    if (!el) return;
    const to = scrollToIndex(cursor, el.scrollTop, el.clientHeight, rowHeight);
    if (to === null) return;
    el.scrollTop = to;
    // The window is derived from `scrollTop` state, so assigning the
    // DOM property is not enough -- state has to learn about it too, or
    // the row scrolled to would not be painted until the next scroll
    // event. Setting it to the value just assigned rather than reading
    // it back, because jsdom does not clamp `scrollTop` the way a
    // browser does and reading it back would diverge between the two.
    setScrollTop(to);
  }, [cursor, rowHeight]);

  return {
    // Computed during render, from state. Never stored.
    window: virtualWindow(total, scrollTop, viewport, rowHeight),
    ref,
    onScroll: sync,
  };
}
