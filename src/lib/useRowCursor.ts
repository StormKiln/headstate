import { useEffect, useRef } from "react";
import { registerRowCursor, type RowCursorTarget } from "./rowCursor";

/// Claim the keyboard row cursor for this list while it is mounted.
///
/// The React half of `rowCursor.ts`; the reasoning for the whole
/// mechanism, and for why only the sessions list uses it, is there.
///
/// # Why a ref, and why the effect has no dependencies
///
/// The effect registers ONCE and deregisters on unmount. What it
/// registers is a stable shim that reads a ref, and the ref is updated on
/// every render. That is the shape #953 requires in so many words -- "a
/// registered list has to be read at keypress time, not captured at
/// mount" -- with neither of the two failure modes it warns about:
///
/// - registering the caller's object directly and depending on it would
///   re-register on every render, which is churn but correct;
/// - registering it with `[]` would freeze the list at first render,
///   which is exactly the stale-list bug `App.tsx`'s `visible` dependency
///   exists to prevent, restated one layer down.
///
/// The ref-plus-stable-shim does neither: one registration, always
/// current rows.
export function useRowCursor(target: RowCursorTarget): void {
  const latest = useRef(target);
  // In an EFFECT rather than during render.
  //
  // Writing a ref during render is the obvious spelling and `yarn lint`
  // rejects it -- `react-hooks/refs`, "Cannot update ref during render" --
  // because the React Compiler may render a component more than once for
  // one commit, and a ref written on a render that is then thrown away
  // would leave the registry describing a list nobody sees.
  //
  // Correct here without qualification: this effect runs before the
  // browser can deliver another keyboard event, since React flushes
  // passive effects before yielding to the event loop that would dispatch
  // one. A `keydown` therefore always reads the committed render's rows.
  //
  // Runs on EVERY render -- no dependency array -- because `target` is a
  // fresh object literal each time and a dependency on it would be the
  // same as no array with an extra comparison. What must not happen is an
  // empty array, which is the stale-capture bug this whole file exists to
  // avoid.
  useEffect(() => {
    latest.current = target;
  });

  // Whether this list has a bulk action at all, decided from the FIRST
  // render and pinned.
  //
  // The shim below cannot forward an optional method by wrapping it: a
  // wrapper is always defined, so `toggle` would exist on every
  // registration and a list with no bulk action would look like one that
  // has an inert bulk action. `x` would still do nothing -- the inner
  // call is optional-chained -- but the distinction disappears from the
  // registry, and "this list has no bulk action" is a fact a caller
  // should be able to read rather than discover by trying.
  //
  // Pinned at first render rather than re-read, because a list that grew
  // a bulk action mid-life would need a re-registration to express it,
  // and no caller does that. `hasToggle` is therefore captured once, in
  // the same effect that registers.
  const hasToggle = useRef(target.toggle !== undefined);

  useEffect(() => {
    return registerRowCursor({
      rows: () => latest.current.rows(),
      open: (i) => latest.current.open(i),
      ...(hasToggle.current
        ? { toggle: (i: number) => latest.current.toggle?.(i) }
        : {}),
    });
  }, []);
}
