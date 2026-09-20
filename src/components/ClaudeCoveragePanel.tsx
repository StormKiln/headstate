import { useClaudeCoverage } from "../api/hooks";
import { errorMessage } from "./QueryError";
import { Card } from "@/components/ui/card";
import type { ClaudeCoverage, ClaudeMeasurement } from "@/types/pr";

/// What the app has READ, against what it HOLDS (#1212, epic #1121).
///
/// # The one place the scope is stated
///
/// Every figure Headstate shows about the Claude corpus is bounded, and
/// each bound used to be re-explained beside the figure it bounded. That
/// does not scale: as panels land the footnotes multiply, and footnotes
/// written at different times about the same corpus drift apart. This
/// states the scope ONCE, so the panels above it can show their numbers
/// and point here.
///
/// # Why there is no score anywhere in this file
///
/// `CLAUDE.md`'s rule is "qualify, or suppress" — and qualifying means
/// "28 of 37", never "76%" and never a grade. The rows here answer
/// DIFFERENT questions over the same denominator: which sessions have a
/// token measurement, which ones a hook watched, which ones carry a
/// transcript path. Averaging them would produce a single figure that
/// answers none of the three while looking like it answers all of them.
/// That fabrication is exactly what the counts exist to prevent, so
/// there is no total, no percentage, no letter and no colour-coded
/// status in this component.
///
/// The backend refuses the same thing structurally — see
/// `claude/coverage.rs` and its `the_report_carries_no_grade` test.
///
/// # Why absence here is scope and not damage
///
/// `overview.rs` says of the archived majority that it is "the NORMAL
/// state and must not be rendered as damage". The uncovered sessions in
/// this panel are the same kind of thing: a session that ran before the
/// usage importer existed has no cost row, and nothing about that is a
/// fault — it is the boundary of what was being measured at the time.
///
/// So this panel is grey. No red, no amber, no warning triangle, and no
/// word from the defect vocabulary ("missing", "incomplete", "failed")
/// anywhere in its rendered text. The only figure that gets any emphasis
/// at all is `unread`, which is a real failure, and it is emphasised by
/// APPEARING — a row with nothing unread says nothing about it rather
/// than printing a reassuring zero.
///
/// # Surface class: Read
///
/// `claude_coverage`, three COUNTs over Headstate's own cache. It
/// renders identically on the phone, where it matters more: away from
/// the desk, every corpus figure is the desktop's measurement and the
/// reader has even less context for what one covers.

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

/// One measurement's row.
///
/// Reads as a sentence about scope: what was measured, of how many, and
/// why the rest are out of scope. The out-of-scope clause is not
/// optional and not a tooltip — a count of uncovered rows with no reason
/// beside it is a defect list, which is what this panel must not be.
function Row({ m }: { m: ClaudeMeasurement }) {
  const total = m.reach.measured + m.reach.outOfScope + m.reach.unread;

  return (
    <div className="border-t border-[#21262d] py-3 first:border-t-0">
      <div className="text-sm text-[#e6edf3]">{m.label}</div>
      <div className="mt-1 text-sm tabular-nums text-[#e6edf3]">
        {total === 0 ? (
          // No denominator means no ratio to state. Words, not "0 of 0",
          // which invites the eye to read a coverage figure off an empty
          // corpus.
          <span className="text-[#8b949e]">{ABSENCE.notMeasured} — no sessions stored yet</span>
        ) : (
          <>
            {ofTotal(m.reach.measured, total, m.unit)}
            <span className="text-[#8b949e]"> carry this measurement</span>
          </>
        )}
      </div>
      {m.reach.outOfScope > 0 ? (
        // The scope clause. Grey and in the same size as the figure
        // above it, deliberately: it is part of the statement, not a
        // caveat appended to it.
        <div className="mt-1 text-xs text-[#8b949e]">
          The other {ofTotal(m.reach.outOfScope, total, m.unit)} {m.scopeNote}. That is
          the expected shape of a corpus older than the measurement, not a
          fault in it.
        </div>
      ) : null}
      {/* Only when there IS one. A row with nothing unread says nothing
          about it: printing "0 could not be read" would give a normal row
          a line of failure vocabulary, and a zero that is reassuring is
          still a zero the reader has to parse as such. */}
      {m.reach.unread > 0 ? (
        <div className="mt-1 text-xs text-[#8b949e]">
          {ABSENCE.couldNotRead}: {ofTotal(m.reach.unread, total, m.unit)}. The figure
          above is short by an unknown amount rather than by a known one.
        </div>
      ) : null}
    </div>
  );
}

/// The panel body, separated from the query so it can be tested with
/// data rather than with a mocked transport.
export function CoverageBody({ data }: { data: ClaudeCoverage }) {
  return (
    <Card className="px-4 py-3">
      <div className="text-sm font-semibold text-[#e6edf3]">
        What these numbers cover
      </div>
      {/* The framing sentence. "Holds" and "read" rather than "has" and
          "missing": the corpus is not incomplete, it is larger than what
          any one measurement was taken over. */}
      <p className="mt-1 text-xs text-[#8b949e]">
        Headstate holds more sessions than any single measurement was taken
        over. Each row says how far that measurement reaches, against the same
        denominator.
      </p>
      <div className="mt-2">
        {data.measurements.map((m) => (
          <Row key={m.id} m={m} />
        ))}
      </div>
      {data.truncatedMeasurements > 0 ? (
        // Beside the rows, not inside one. A truncated read is a
        // different KIND of shortfall: those sessions ARE measured and
        // counted as such, and what they contributed is a floor.
        <div className="mt-3 border-t border-[#21262d] pt-3 text-xs text-[#8b949e]">
          {ofTotal(data.truncatedMeasurements, data.sessions, "session")} stopped at
          the read budget, so their token figures are floors rather than totals.
          They are counted as measured above, because what they contributed is
          real.
        </div>
      ) : null}
      {/* The rule, stated where a reader can check the panel against it.
          Not decoration: it is why there is no single figure here, and a
          reader who wants one deserves to know it was refused on
          purpose. */}
      <p className="mt-3 text-xs text-[#8b949e]">
        These rows answer different questions and are not combined into one
        figure. {ABSENCE.notMeasured} and {ABSENCE.measuredZero} are different
        findings, and a single score would hide which one you are looking at.
      </p>
    </Card>
  );
}

/// What the app has read, against what it holds.
export function ClaudeCoveragePanel() {
  const { data, isLoading, isError, error } = useClaudeCoverage(true);

  // ERROR FIRST, before loading and before the body, for the ordering
  // `ClaudeOverviewPage` states: a rejected query must never reach an arm
  // where `data` is undefined and every figure would read as
  // absent-or-zero. Here that would be the panel about honest
  // denominators printing dishonest ones.
  if (isError) {
    return (
      <Card className="px-4 py-3">
        <div className="text-sm font-semibold text-[#e6edf3]">
          What these numbers cover
        </div>
        <p className="mt-1 text-xs text-[#8b949e]">
          {ABSENCE.couldNotRead}: {errorMessage(error)}. No coverage figures are
          shown rather than zeroes, which would claim a scope nothing
          established.
        </p>
      </Card>
    );
  }

  if (isLoading || !data) {
    return <Card className="min-h-24 px-4 py-3" aria-busy="true" />;
  }

  return <CoverageBody data={data} />;
}
