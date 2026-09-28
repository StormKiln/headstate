/// Whether a worktree row's props are unchanged, for `memo` (#1582).
///
/// The Worktrees page re-renders once per frame while a classification
/// pass streams its verdicts, and each render rebuilt all of a
/// repository's rows: 141 rows for one verdict. MEASURED in the Worktrees
/// browser harness (`make bench-worktrees-browser`), that was a ~4 ms
/// commit per frame in Chromium on a fast machine and ~21 ms at 4x CPU
/// throttling, where it held the main thread at 89% and delivered
/// verdicts up to 7.5 s late.
///
/// Every prop is compared by identity EXCEPT the worktree itself, which
/// is compared field by field. The page builds each row as a fresh
/// spread of the listing, its verdict and its size, so the OBJECT is new
/// on every render while its FIELDS are the same values, and the same
/// references, until that row's own verdict or size arrives. A nested
/// value (`safety`, `upstream`) is still compared by identity: a verdict
/// that re-arrives is a new object, and the row re-renders for it, which
/// is right, because its content may have changed.
///
/// Handlers must be stable for this to save anything; the page wraps its
/// own in `useStableHandlers` for that reason.
export function sameRowProps<P extends { wt: object }>(a: P, b: P): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]) as Set<keyof P>;
  for (const k of keys) {
    if (k === "wt") {
      if (!shallowEqual(a.wt, b.wt)) return false;
    } else if (!Object.is(a[k], b[k])) {
      return false;
    }
  }
  return true;
}

function shallowEqual(a: object, b: object): boolean {
  if (a === b) return true;
  const x = a as Record<string, unknown>;
  const y = b as Record<string, unknown>;
  const kx = Object.keys(x);
  if (kx.length !== Object.keys(y).length) return false;
  return kx.every((k) => Object.prototype.hasOwnProperty.call(y, k) && Object.is(x[k], y[k]));
}
