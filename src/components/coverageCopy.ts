/// The coverage panel's copy and its one figure formatter. Apart from
/// `ClaudeCoveragePanel.tsx` so that file exports only components
/// (#1965), which is what lets dev hot-reload swap it in place.

/// The three absence cases, phrased so they cannot be read as each
/// other.
///
/// This is the editorial core of #1212. "Not measured", "measured as
/// zero" and "could not be read" are three different facts, and
/// collapsing any pair is the defect this panel exists to prevent:
///
/// - **Not measured** — we did not take this reading. Says nothing about
///   the value. Never rendered as 0.
/// - **Measured as zero** — we took the reading and it was zero. A real
///   finding, and the only one of the three that licenses the digit 0.
/// - **Could not be read** — we tried to take the reading and failed.
///   The total is short by an unknown amount.
///
/// Exported so the copy is decided in ONE place and tested directly,
/// following `NotMeasured` in `SystemHealthPage`: one component rather
/// than a repeated string, so the phrase and the fact that it is not a
/// number are settled together.
export const ABSENCE = {
  /// No reading was taken. NOT a statement that the value is zero.
  notMeasured: "Not measured",
  /// A reading was taken and it was zero. The digit is honest here.
  measuredZero: "Measured as zero",
  /// A reading was attempted and did not complete.
  couldNotRead: "Could not be read",
} as const;

/// A count with the denominator it is read against, as one string.
///
/// The panel's hard rule in function form: there is no way to render a
/// figure here WITHOUT its denominator, because this is the only thing
/// that renders a figure and it takes both. A bare "88" and a bare "6%"
/// are both the defect, and neither can be produced from this signature.
///
/// Thousands separators come from `toLocaleString()` at runtime rather
/// than from a literal — `measuredFigures.test.ts` is the house guard
/// against a measured count reaching the screen as source text.
export function ofTotal(n: number, total: number, unit: string): string {
  const noun = total === 1 ? unit : `${unit}s`;
  return `${n.toLocaleString()} of ${total.toLocaleString()} ${noun}`;
}
