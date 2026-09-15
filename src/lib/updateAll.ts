import type { UpdateAllReport } from "@/api/tauri";

/// Reading the result of one click across 45 repositories (#1014).
///
/// # Why this is four numbers and not "12 of 45"
///
/// "Updated 12 of 45" tells a user nothing actionable. Which 12? Of the
/// other 33, how many were already level (nothing to do), how many were
/// skipped because they sit on a feature branch (fine), and how many
/// genuinely could not be read or reached (a problem)? A single aggregate
/// hides the only distinction that matters: **could-not versus did-not**.
///
/// #941 spent a day removing aggregate numbers of exactly this shape, and
/// this codebase already implements the alternative for every comparable
/// bulk action. `RemovalOutcome`'s doc states the rule in prose -- "every
/// input gets an outcome", "partial failure is the normal case here, not
/// the exception" -- and both are MORE true here: on a working machine
/// most repositories sit on a feature branch and are skipped by design,
/// so partial completion is not merely normal, it is the expected
/// majority.
///
/// So this counts each state separately and nothing is ever summed across
/// the could-not/did-not line. `alreadyLevel` is never counted as a
/// failure and `skipped` is never counted as a success -- the two
/// miscounts that make the summary useless in opposite directions.
export interface UpdateAllTally {
  /// Fast-forwarded. The only rows that moved.
  updated: number;
  /// Nothing to do. A success, and never a failure row.
  alreadyLevel: number;
  /// A deliberate non-action with a named reason. Not a malfunction.
  skipped: number;
  /// Could not be read or reached. **The rows that need attention.**
  failed: number;
  /// The run ended before reaching these. Cancelled, or the ceiling.
  notAttempted: number;
  /// Every repository the run enumerated. Never a denominator for
  /// `updated` on its own -- see `summarise`.
  total: number;
  /// Directories the SCAN could not read (#1025).
  ///
  /// NOT repositories, and never added to any count above: they are a
  /// hole in the list, not rows in it. Carried so the summary can say the
  /// census was short.
  unreadable: number;
}

export function tally(report: UpdateAllReport): UpdateAllTally {
  const t: UpdateAllTally = {
    updated: 0,
    alreadyLevel: 0,
    skipped: 0,
    failed: 0,
    notAttempted: 0,
    total: report.outcomes.length,
    unreadable: report.unreadable.length,
  };
  for (const o of report.outcomes) {
    switch (o.result.state) {
      case "updated":
        t.updated += 1;
        break;
      case "alreadyLevel":
        t.alreadyLevel += 1;
        break;
      case "skipped":
        t.skipped += 1;
        break;
      case "failed":
        t.failed += 1;
        break;
      // Listed explicitly rather than left to a default, so a sixth
      // state added to `UpdateResult` is a type error here rather than a
      // silent assignment to one side of the line.
      case "notAttempted":
        t.notAttempted += 1;
        break;
    }
  }
  return t;
}

/// The completion line.
///
/// **"Updated 12 · 21 already level · 9 skipped · 3 could not be
/// reached"**, not "Updated 12 of 45". The three at the end are the whole
/// point of reading it.
///
/// Zero-valued states are omitted, so a clean run reads "Updated 12 · 33
/// already level" rather than dragging two zeroes along. `failed` is the
/// exception and is ALWAYS shown when non-zero, first among the tail,
/// because it is the reason to read the line at all.
///
/// The unreadable count is appended as its own clause rather than folded
/// into a number (#1025). A user who dismissed `PartialScanNotice` before
/// clicking -- or who reads only the result -- must still learn the set
/// was short, and it must be visibly not a repository count.
export function summarise(report: UpdateAllReport): string {
  const t = tally(report);
  const parts: string[] = [];
  if (t.updated > 0) parts.push(`Updated ${t.updated}`);
  if (t.alreadyLevel > 0) parts.push(`${t.alreadyLevel} already level`);
  if (t.skipped > 0) parts.push(`${t.skipped} skipped`);
  if (t.failed > 0) {
    parts.push(`${t.failed} could not be reached`);
  }
  if (t.notAttempted > 0) parts.push(`${t.notAttempted} not attempted`);
  // A run over no repositories at all says so, rather than rendering an
  // empty string that reads as a success with no detail.
  if (parts.length === 0) parts.push("No repositories to update");

  let line = parts.join(" · ");
  if (report.cancelled) line = `Stopped. ${line}`;
  else if (report.timedOut) line = `Timed out. ${line}`;
  if (t.unreadable > 0) {
    line += `. ${t.unreadable} ${
      t.unreadable === 1 ? "directory" : "directories"
    } could not be read, so this was not every repository`;
  }
  return line;
}

/// The button's own label, which must not say "All" over a short census
/// (#1025).
///
/// "Update All Repositories" over a scan that could not read three
/// directories is a lie about scope, and nothing in the sequence is
/// false: the table lists 42, the button updates 42, the report says 42
/// of 42, and the user closes the app believing all 45 are level. They
/// have been given a reason to stop checking.
///
/// So the button names the number it can actually SEE when the scan fell
/// short, and keeps the plain label when it did not. It stays ENABLED
/// either way: disabling it would let one unreadable directory block
/// updating 42 perfectly good repositories, trading a lie for a broken
/// feature.
export function updateAllLabel(count: number, unreadable: readonly string[]): string {
  if (unreadable.length === 0) return "Update All Repositories";
  return `Update ${count} readable ${count === 1 ? "repository" : "repositories"}`;
}
