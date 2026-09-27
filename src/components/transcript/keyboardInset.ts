/// How much of the layout viewport the on-screen keyboard covers (#1491,
/// #1490 constraint 3).
///
/// iOS does not shrink the layout viewport when the keyboard opens; it
/// shrinks the VISUAL viewport and may scroll it. A bar laid out at the
/// bottom of the page would sit under the keyboard. The part of the
/// layout viewport below the visual one is what the keyboard covers:
///
/// ```text
/// inset = innerHeight - visualViewport.height - visualViewport.offsetTop
/// ```
///
/// The phone composer pads its bottom by this, inside the viewer's
/// column, so the transcript's scroller shrinks rather than the bar
/// being covered -- and the scroller's follow re-pins the live edge
/// when its viewport shrinks.
///
/// No `visualViewport` (engines that resize the layout viewport
/// instead): the inset is 0, and the layout already avoids the keyboard.
///
/// Not measured on a device in 7.9: the composer is behind
/// `COMPOSER_ENABLED`, so nothing a phone shows uses it yet. 7.10 must
/// check it on an iPhone before the flag is turned on.

import { useCallback, useSyncExternalStore } from "react";

export function keyboardInset(
  viewport: { height: number; offsetTop: number },
  innerHeight: number,
): number {
  return Math.max(0, Math.round(innerHeight - viewport.height - viewport.offsetTop));
}

export function useKeyboardInset(enabled: boolean): number {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const vv = enabled ? window.visualViewport : null;
      if (!vv) return () => {};
      vv.addEventListener("resize", onChange);
      vv.addEventListener("scroll", onChange);
      return () => {
        vv.removeEventListener("resize", onChange);
        vv.removeEventListener("scroll", onChange);
      };
    },
    [enabled],
  );
  const read = () => {
    const vv = enabled ? window.visualViewport : null;
    return vv ? keyboardInset(vv, window.innerHeight) : 0;
  };
  return useSyncExternalStore(subscribe, read, () => 0);
}
