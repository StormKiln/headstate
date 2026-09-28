import { useLayoutEffect, useRef, useState } from "react";

/// Handlers with a stable identity that always call the latest version
/// (#1582).
///
/// A memoised row only skips a render when every prop is unchanged, and a
/// handler written inline in the page is a new function on every render,
/// so one unstable handler makes the memo useless. `useCallback` cannot
/// fix these: each closes over the page's state (the selection, the
/// anchor, the visible order), so its dependencies change exactly when
/// the page renders.
///
/// Each returned function forwards to the handler from the LAST COMMITTED
/// render, which is the one whose rows are on screen when a click lands.
/// The set of keys is fixed at mount; a key added later is not forwarded.
export function useStableHandlers<H extends Record<string, (...args: never[]) => unknown>>(handlers: H): H {
  const latest = useRef(handlers);
  useLayoutEffect(() => {
    latest.current = handlers;
  });
  const [stable] = useState(() => {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(handlers)) {
      out[key] = (...args: never[]) => latest.current[key](...args);
    }
    return out as H;
  });
  return stable;
}
