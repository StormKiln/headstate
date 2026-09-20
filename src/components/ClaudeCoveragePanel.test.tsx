import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ABSENCE, CoverageBody, ofTotal } from "./ClaudeCoveragePanel";
import panelSource from "./ClaudeCoveragePanel.tsx?raw";
import type { ClaudeCoverage, ClaudeMeasurement } from "@/types/pr";

function measurement(over: Partial<ClaudeMeasurement> = {}): ClaudeMeasurement {
  return {
    id: "cost",
    label: "Token and cost totals",
    unit: "session",
    scopeNote: "ran before the usage importer read them",
    reach: { measured: 88, outOfScope: 1365, unread: 0 },
    ...over,
  };
}

function coverage(over: Partial<ClaudeCoverage> = {}): ClaudeCoverage {
  return {
    sessions: 1453,
    measurements: [measurement()],
    truncatedMeasurements: 0,
    ...over,
  };
}

/// Every displayed figure carries its denominator.
///
/// The panel's hard rule, and the reason it exists: a bare "88" says
/// nothing and a bare "6%" says something false about a corpus that has
/// grown since. #1212's framing is "88 of 1,453 sessions carry a
/// recorded cost", and a number that reaches the screen without its
/// denominator is the defect.
///
/// Sabotage: changing `ofTotal` to return `n.toLocaleString()` alone
/// fails "states its denominator" on every figure. Confirmed.
describe("every figure names its denominator", () => {
  it("states a count against the total it was read from", () => {
    render(<CoverageBody data={coverage()} />);
    expect(screen.getByText(/88 of 1,453 sessions/)).toBeTruthy();
    expect(screen.getByText(/1,365 of 1,453 sessions/)).toBeTruthy();
  });

  /// The scan of what actually reached the screen, rather than a check
  /// of one call site. Every thousands-separated integer in the rendered
  /// text must be part of an "N of M" phrase — so a figure added later
  /// without a denominator fails here even though nothing in this file
  /// mentions it.
  it("renders no bare count anywhere in the panel", () => {
    const { container } = render(
      <CoverageBody
        data={coverage({
          measurements: [
            measurement(),
            measurement({
              id: "hooks",
              label: "Run and lifecycle records",
              reach: { measured: 12, outOfScope: 1400, unread: 41 },
            }),
          ],
          truncatedMeasurements: 7,
        })}
      />,
    );
    const text = container.textContent ?? "";
    // Strip every well-formed "N of M unit" phrase; any separated
    // integer left over reached the screen without a denominator.
    const stripped = text.replace(/[\d,]+ of [\d,]+ \w+/g, "");
    expect(stripped.match(/\d{1,3},\d{3}/g) ?? []).toEqual([]);
    // And no percentage, which is the other way a denominator gets
    // dropped -- one that looks like it kept it.
    expect(text).not.toMatch(/\d+(\.\d+)?\s*%/);
  });

  /// Singular and plural both keep the denominator; only the noun moves.
  it("keeps the denominator when the unit is singular", () => {
    expect(ofTotal(1, 1, "session")).toBe("1 of 1 session");
    expect(ofTotal(0, 12, "session")).toBe("0 of 12 sessions");
  });
});

/// The three absence cases stay three.
///
/// "Not measured", "measured as zero" and "could not be read" are
/// different facts, and collapsing any pair is the defect this panel
/// exists to prevent. Asserted on the STRINGS being mutually distinct,
/// not merely on some text appearing: a test that only checked presence
/// would pass with all three set to the same word.
///
/// Sabotage: setting `ABSENCE.couldNotRead = ABSENCE.notMeasured` fails
/// "three distinct strings" and the unread-wording test. Confirmed.
describe("the three absence cases are distinguishable", () => {
  it("gives three distinct strings", () => {
    const all = [ABSENCE.notMeasured, ABSENCE.measuredZero, ABSENCE.couldNotRead];
    expect(new Set(all).size).toBe(3);
    // Nor may one be a substring of another: "Not measured" inside
    // "Not measured yet" would let a reader's eye — and a test — take
    // one for the other.
    for (const a of all) {
      for (const b of all) {
        if (a !== b) expect(a.includes(b)).toBe(false);
      }
    }
  });

  /// An unread row says "could not be read" and NOT "not measured".
  /// A read that failed is short by an unknown amount; a reading never
  /// taken is short by a known one.
  it("words a failed read differently from a reading never taken", () => {
    render(
      <CoverageBody
        data={coverage({
          measurements: [
            measurement({ reach: { measured: 88, outOfScope: 1300, unread: 65 } }),
          ],
        })}
      />,
    );
    expect(screen.getByText(/Could not be read: 65 of 1,453 sessions/)).toBeTruthy();
    expect(screen.getByText(/short by an unknown amount/)).toBeTruthy();
  });

  /// An empty corpus is "not measured", never "0 of 0".
  ///
  /// With no denominator there is no ratio to state, and "0 of 0
  /// sessions" invites the eye to read a coverage figure off an empty
  /// corpus.
  it("says not measured rather than a ratio over nothing", () => {
    render(
      <CoverageBody
        data={coverage({
          sessions: 0,
          measurements: [measurement({ reach: { measured: 0, outOfScope: 0, unread: 0 } })],
        })}
      />,
    );
    expect(screen.getByText(/Not measured — no sessions stored yet/)).toBeTruthy();
    expect(screen.queryByText(/0 of 0/)).toBeNull();
  });
});

/// A zero in a normal category renders as nothing, never as a defect.
///
/// The ~1,365 sessions with no cost record are NORMAL, exactly as
/// `overview.rs` insists the archived majority is "the NORMAL state and
/// must not be rendered as damage". A panel that printed "0 could not be
/// read" on every healthy row would give normal rows a line of failure
/// vocabulary.
///
/// Sabotage: removing the `unread > 0` guard in `Row` makes a healthy
/// row print "Could not be read: 0 of 1,453 sessions" and fails both
/// assertions. Confirmed.
describe("a normal absence is not rendered as damage", () => {
  it("prints no failure line when nothing failed", () => {
    const { container } = render(<CoverageBody data={coverage()} />);
    expect(screen.queryByText(/Could not be read/)).toBeNull();
    expect(container.textContent).not.toMatch(/0 of 1,453/);
  });

  it("frames the uncovered majority as scope rather than as a fault", () => {
    const { container } = render(<CoverageBody data={coverage()} />);
    const text = container.textContent ?? "";
    expect(text).toMatch(/not a\s+fault in it/);
    // The defect vocabulary, absent from rendered text by construction.
    for (const word of ["missing", "incomplete", "failed", "broken", "error"]) {
      expect(text.toLowerCase()).not.toContain(word);
    }
  });

  /// No warning iconography and no alert role, so assistive technology
  /// does not announce a statement of scope as something to act on.
  it("uses no alert role and no warning colour", () => {
    const { container } = render(
      <CoverageBody
        data={coverage({
          measurements: [
            measurement({ reach: { measured: 88, outOfScope: 1300, unread: 65 } }),
          ],
          truncatedMeasurements: 9,
        })}
      />,
    );
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector("svg")).toBeNull();
    // Red and amber, the two tones this repo uses for damage.
    expect(container.innerHTML).not.toMatch(/#f85149|#d29922|text-red|text-amber/);
  });
});

/// The panel carries no rolled-up grade.
///
/// Structural, over the source: a score, a percentage or an "N issues"
/// summary is precisely the fabrication the counts exist to prevent, and
/// a future edit that added one would have to defeat this first. Mirrors
/// `the_report_carries_no_grade` in `claude/coverage.rs`, one layer up.
describe("the panel is counts with denominators and not a score", () => {
  it("renders no score, grade or percentage", () => {
    const { container } = render(
      <CoverageBody data={coverage({ truncatedMeasurements: 3 })} />,
    );
    const text = (container.textContent ?? "").toLowerCase();
    // A score, a grade or an "N issues" summary as a FIGURE. The closing
    // sentence names the word "score" in order to say the panel does not
    // carry one, which is the opposite of the defect -- so the check is
    // for the word next to a number, which is what a displayed grade
    // looks like.
    for (const word of ["score", "grade", "health", "rating"]) {
      expect(text).not.toMatch(new RegExp(`[\\d]\\s*${word}|${word}\\s*[:=]?\\s*[\\d]`));
    }
    expect(text).not.toMatch(/\d+(\.\d+)?\s*%/);
    expect(text).not.toMatch(/\d+\s+issues?\b/);
  });

  /// No arithmetic that combines rows. Division is how a ratio is born
  /// and the rows are not commensurable, so the component does none.
  it("does no division in the component source", () => {
    // Comments stripped first, so prose about ratios does not trip it.
    const code = panelSource
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^[ \t]*\/\/.*$/gm, "");
    expect(code).not.toMatch(/\/\s*(total|sessions|denominator)\b/);
    expect(code).not.toContain("Math.round");
    expect(code).not.toContain("toFixed");
  });
});
