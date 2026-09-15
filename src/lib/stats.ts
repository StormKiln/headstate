/// Percent change between two periods.
///
/// Returns `Infinity` when `previous` is 0 but `current` is not -- there is
/// no meaningful ratio, and the UI renders that as "new" rather than a
/// number. Returns `null` when both are 0, which is "no activity", not a
/// 0% change.
export function pctChange(current: number, previous: number): number | null {
  if (previous === 0) return current === 0 ? null : Infinity;
  return ((current - previous) / previous) * 100;
}

/// Nearest-rank percentile over an ALREADY SORTED ascending array.
///
/// Nearest rank is `ceil(n * p)` in 1-based terms, so `ceil(n*p) - 1`
/// zero-indexed. `floor(n*p)` -- what this used to do -- agrees only when
/// `n*p` is NOT an integer, and it is an integer at exactly the two call
/// sites the UI exercises: p50 and p90 over a 100-PR sample. That returned
/// the 51st and 91st values where the 50th and 90th are correct.
///
/// Both ends are clamped: `p = 1.0` would index `n`, and `p = 0` would
/// index `-1`.
export function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.ceil(sorted.length * p) - 1;
  return sorted[Math.min(Math.max(idx, 0), sorted.length - 1)];
}

export function formatPct(v: number | null): string {
  if (v === null) return "--";
  if (!Number.isFinite(v)) return "new";
  return `${v >= 0 ? "+" : ""}${Math.round(v)}%`;
}

/// How many missing days to NAME before falling back to a count (#1045).
///
/// Three, because naming is useful in proportion to how easily a reader can
/// act on it: three dates can be held in the head and gone and looked at,
/// and past that the list stops being a set of days to check and becomes a
/// paragraph to skip. The original branch had no cap at all, which is how a
/// 30-day window printed 30 dates.
export const NAMED_DAYS = 3;

/// The two ways a daily series can be short, which need different messages.
///
/// The distinction is the whole of #1045. Naming 2 missing days out of 30 is
/// genuinely the most informative thing available -- the chart exists, and
/// the note says which parts of it to distrust. Naming 30 out of 30 tells a
/// reader nothing they cannot already see, and buries the fact that actually
/// matters: there is no chart, because the measurement did not complete.
export type FailedDays =
  | { kind: "none" }
  /// Some days are missing from a chart that exists.
  | { kind: "partial"; count: number; named: string[]; rest: number }
  /// NO day was measured. An error state, not an annotation.
  | { kind: "total"; count: number };

/// Classify a series' failed days against the window it was measured over.
///
/// `days` is the window the chart asked for, NOT `points.length`: a series
/// that returned no points at all would make those two agree at zero and
/// report a total failure as "none", which is the inversion this function
/// exists to prevent.
///
/// Pure and separate from the component so the three branches are testable
/// without rendering a chart -- and because the comparison against `days` is
/// the load-bearing line, and a line deciding between "here is your chart"
/// and "this failed entirely" should not be buried in JSX.
export function classifyFailedDays(
  failedDays: string[],
  days: number,
): FailedDays {
  const count = failedDays.length;
  if (count === 0) return { kind: "none" };
  // `>=` rather than `===`. The window is clamped server-side and a
  // boundary could hand back one more failed day than the client asked for;
  // a total failure that missed an equality test by one would fall through
  // to the partial branch and print the wall of dates this exists to stop.
  if (days > 0 && count >= days) return { kind: "total", count };
  return {
    kind: "partial",
    count,
    named: failedDays.slice(0, NAMED_DAYS),
    rest: Math.max(count - NAMED_DAYS, 0),
  };
}

/// Render a partial classification's day list: the named few, then a count.
///
/// "2026-08-17, 2026-08-18, 2026-08-19, and 9 others" rather than twelve
/// dates. The remainder is COUNTED rather than dropped, so the sentence
/// still says how much is missing -- shortening the list must not shorten
/// the claim.
export function namedDaysText(named: string[], rest: number): string {
  const list = named.join(", ");
  if (rest <= 0) return list;
  return `${list}, and ${rest} other${rest === 1 ? "" : "s"}`;
}
