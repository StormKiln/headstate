import { describe, expect, it } from "vitest";
import type { UpdateAllReport, RepoUpdateOutcome } from "@/api/tauri";
import { summarise, tally, updateAllLabel } from "./updateAll";

const out = (path: string, result: RepoUpdateOutcome["result"]): RepoUpdateOutcome => ({
  path,
  result,
});

const report = (over: Partial<UpdateAllReport> = {}): UpdateAllReport => ({
  outcomes: [],
  cancelled: false,
  timedOut: false,
  unreadable: [],
  ...over,
});

describe("tally", () => {
  /// THE four-outcomes test on the frontend half (#1014).
  ///
  /// The whole point of the shape: a skip is not a failure and an
  /// already-level repository is not one either. These are the two
  /// miscounts that make the summary useless in opposite directions.
  it("keeps the four outcomes distinct rather than collapsing them", () => {
    const t = tally(
      report({
        outcomes: [
          out("/a", { state: "updated", message: "Updating abc..def" }),
          out("/b", { state: "alreadyLevel" }),
          out("/c", { state: "skipped", reason: "on feature/x, not the default branch" }),
          out("/d", { state: "skipped", reason: "2 uncommitted changes -- commit or stash first" }),
          out("/e", { state: "failed", error: "could not resolve host github.com" }),
        ],
      }),
    );

    expect(t).toEqual({
      updated: 1,
      alreadyLevel: 1,
      skipped: 2,
      failed: 1,
      notAttempted: 0,
      total: 5,
      unreadable: 0,
    });
    // Stated as its own assertion because it is the invariant, not an
    // incidental consequence of the numbers above: could-not is one row,
    // did-not is three, and nothing sums across that line.
    expect(t.failed).toBe(1);
    expect(t.skipped + t.alreadyLevel).toBe(3);
  });

  /// Unreadable directories are NOT repositories (#1025). Summing them
  /// into a per-repository outcome would invent rows for things that
  /// were never enumerated.
  it("counts unreadable directories separately from every repository outcome", () => {
    const t = tally(
      report({
        outcomes: [out("/a", { state: "alreadyLevel" })],
        unreadable: ["/work: permission denied", "/mnt/x: not a directory"],
      }),
    );
    expect(t.total).toBe(1);
    expect(t.unreadable).toBe(2);
    expect(t.failed).toBe(0);
    expect(t.skipped).toBe(0);
  });
});

describe("summarise", () => {
  /// The line #1014 asks for, verbatim in shape: "Updated 12 · 21
  /// already level · 9 skipped · 3 could not be reached", never
  /// "Updated 12 of 45".
  it("distinguishes could-not from did-not rather than giving one ratio", () => {
    const outcomes: RepoUpdateOutcome[] = [];
    for (let i = 0; i < 12; i++) outcomes.push(out(`/u${i}`, { state: "updated", message: "ok" }));
    for (let i = 0; i < 21; i++) outcomes.push(out(`/l${i}`, { state: "alreadyLevel" }));
    for (let i = 0; i < 9; i++) outcomes.push(out(`/s${i}`, { state: "skipped", reason: "branch" }));
    for (let i = 0; i < 3; i++) outcomes.push(out(`/f${i}`, { state: "failed", error: "host" }));

    const line = summarise(report({ outcomes }));

    expect(line).toBe("Updated 12 · 21 already level · 9 skipped · 3 could not be reached");
    // And explicitly NOT the aggregate #941 spent a day removing.
    expect(line).not.toContain("12 of 45");
    expect(line).not.toMatch(/\d+ of \d+/);
  });

  /// The happy-path pair to the test above: a clean run must not be made
  /// noisy by dragging zero-valued states along.
  it("omits states that did not occur", () => {
    const line = summarise(
      report({
        outcomes: [
          out("/a", { state: "updated", message: "ok" }),
          out("/b", { state: "alreadyLevel" }),
        ],
      }),
    );
    expect(line).toBe("Updated 1 · 1 already level");
    expect(line).not.toContain("skipped");
    expect(line).not.toContain("could not be reached");
  });

  /// A partial scan must not be presented as "all" (#1025). A user who
  /// reads only the result still learns the set was short.
  it("says the census was short when the scan could not read everything", () => {
    const line = summarise(
      report({
        outcomes: [out("/a", { state: "updated", message: "ok" })],
        unreadable: ["/work: permission denied"],
      }),
    );
    expect(line).toContain("1 directory could not be read");
    expect(line).toContain("not every repository");
  });

  /// A cancelled run says so and still reports what it managed, rather
  /// than discarding it (#1016).
  it("reports a cancelled run's work rather than discarding it", () => {
    const line = summarise(
      report({
        cancelled: true,
        outcomes: [
          out("/a", { state: "updated", message: "ok" }),
          out("/b", { state: "notAttempted" }),
        ],
      }),
    );
    expect(line).toContain("Stopped");
    expect(line).toContain("Updated 1");
    expect(line).toContain("1 not attempted");
  });

  it("distinguishes a timeout from a cancellation", () => {
    const line = summarise(report({ timedOut: true, outcomes: [out("/b", { state: "notAttempted" })] }));
    expect(line).toContain("Timed out");
    expect(line).not.toContain("Stopped");
  });

  it("says so rather than rendering an empty line when there was nothing to do", () => {
    expect(summarise(report())).toBe("No repositories to update");
  });
});

describe("updateAllLabel", () => {
  /// The button will not say "All" over a short census (#1025).
  it("names the number it can see when the scan fell short", () => {
    expect(updateAllLabel(42, ["/work: permission denied"])).toBe(
      "Update 42 readable repositories",
    );
    expect(updateAllLabel(42, [])).toBe("Update All Repositories");
  });

  it("keeps the plain label when the scan was complete", () => {
    expect(updateAllLabel(45, [])).toBe("Update All Repositories");
  });

  it("agrees with itself in the singular", () => {
    expect(updateAllLabel(1, ["/x: denied"])).toBe("Update 1 readable repository");
  });
});
