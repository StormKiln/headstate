import { describe, expect, it } from "vitest";
import {
  NAMED_DAYS,
  classifyFailedDays,
  formatPct,
  unmeasuredMessage,
  namedDaysText,
  pctChange,
  percentile,
} from "./stats";

describe("pctChange", () => {
  it("computes a rise", () => {
    expect(pctChange(183, 110)).toBeCloseTo(66.4, 1);
  });
  it("computes a decline", () => {
    expect(pctChange(110, 183)).toBeCloseTo(-39.9, 1);
  });
  // Guards the divide-by-zero that a naive ((c-p)/p) would hit.
  it("reports Infinity when there is no prior activity", () => {
    expect(pctChange(5, 0)).toBe(Infinity);
  });
  it("reports null when both periods are empty", () => {
    expect(pctChange(0, 0)).toBeNull();
  });
});

describe("percentile", () => {
  const ten = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
  // Nearest rank: ceil(10 * 0.5) = 5th value = 5, not the 6th.
  it("takes p50", () => expect(percentile(ten, 0.5)).toBe(5));
  it("takes p90", () => expect(percentile(ten, 0.9)).toBe(9));
  // The case that made the old floor() implementation wrong in production:
  // n*p is an integer at both call sites the Stats page uses.
  it("does not overshoot by one rank on a round sample", () => {
    const hundred = Array.from({ length: 100 }, (_, i) => i + 1);
    expect(percentile(hundred, 0.5)).toBe(50);
    expect(percentile(hundred, 0.9)).toBe(90);
  });
  it("clamps at p0 instead of indexing -1", () => {
    expect(percentile(ten, 0)).toBe(1);
  });
  // floor(n*p) can equal n; the index must be clamped.
  it("does not read past the end on a single sample", () => {
    expect(percentile([7], 0.9)).toBe(7);
  });
  // p=1.0 is the only input where floor(n*p) === n, so this is the case
  // that actually exercises the clamp. Without it the guard is untested.
  it("clamps at p100 instead of returning undefined", () => {
    expect(percentile([1, 2, 3], 1.0)).toBe(3);
  });
  it("returns 0 for an empty series rather than NaN", () => {
    expect(percentile([], 0.5)).toBe(0);
  });
});

describe("formatPct", () => {
  it("signs a rise", () => expect(formatPct(66.4)).toBe("+66%"));
  it("signs a decline", () => expect(formatPct(-39.9)).toBe("-40%"));
  it("labels a start from zero", () => expect(formatPct(Infinity)).toBe("new"));
  it("labels no activity", () => expect(formatPct(null)).toBe("--"));
});

describe("classifyFailedDays", () => {
  /// Nothing missing is not a partial, and must not render a warning at all.
  it("reports none when every day was measured", () => {
    expect(classifyFailedDays([], 30)).toEqual({ kind: "none" });
  });

  /// #1045's bug. Thirty of thirty is not an annotation on a chart -- there
  /// is no chart -- so it gets its own kind, and the UI gets to say the
  /// measurement did not complete rather than listing every date in the
  /// window.
  it("reports a total failure when no day could be measured", () => {
    const all = Array.from({ length: 30 }, (_, i) => `2026-08-${i + 1}`);
    expect(classifyFailedDays(all, 30)).toEqual({ kind: "total", count: 30 });
  });

  /// Off-by-one protection. The window is clamped server-side, so a boundary
  /// can hand back one more failed day than the client asked for; an
  /// equality test would drop that through to the partial branch and print
  /// the wall of dates.
  it("still reports a total failure when more days failed than were asked for", () => {
    const all = Array.from({ length: 31 }, (_, i) => `2026-08-${i + 1}`);
    expect(classifyFailedDays(all, 30).kind).toBe("total");
  });

  /// The behaviour #1045 explicitly asks be KEPT. Naming 2 of 30 is the most
  /// informative thing available, and the reasoning in `StatsPage`'s comment
  /// stands.
  it("names the days on a genuine partial", () => {
    expect(classifyFailedDays(["2026-08-20", "2026-08-21"], 30)).toEqual({
      kind: "partial",
      count: 2,
      named: ["2026-08-20", "2026-08-21"],
      rest: 0,
    });
  });

  /// Bounded even when it is real. Past a handful the list stops being a set
  /// of days to go and check and becomes a paragraph to skip.
  it("caps the enumeration and counts the rest", () => {
    const days = Array.from({ length: 12 }, (_, i) => `2026-08-${i + 1}`);
    expect(classifyFailedDays(days, 30)).toEqual({
      kind: "partial",
      count: 12,
      named: days.slice(0, NAMED_DAYS),
      rest: 12 - NAMED_DAYS,
    });
  });

  /// A zero-day window cannot say anything about proportion, so it is
  /// treated as a partial rather than as a total failure -- the safer of the
  /// two, since a partial still names what is missing.
  it("does not call anything total when the window is zero days", () => {
    expect(classifyFailedDays(["2026-08-20"], 0).kind).toBe("partial");
  });
});

describe("namedDaysText", () => {
  it("lists a short set in full", () => {
    expect(namedDaysText(["2026-08-17", "2026-08-18"], 0)).toBe(
      "2026-08-17, 2026-08-18",
    );
  });

  /// The remainder is COUNTED rather than dropped: shortening the list must
  /// not shorten the claim.
  it("counts the remainder rather than dropping it", () => {
    expect(namedDaysText(["2026-08-17", "2026-08-18"], 9)).toBe(
      "2026-08-17, 2026-08-18, and 9 others",
    );
  });

  it("says one other in the singular", () => {
    expect(namedDaysText(["2026-08-17"], 1)).toBe("2026-08-17, and 1 other");
  });
});

describe("unmeasuredMessage (#1050)", () => {
  const NOW = new Date("2026-09-15T16:20:00Z");

  /// THE regression test for #1050.
  ///
  /// The reported symptom was a stats page that said "GitHub did not answer
  /// the daily documents, which usually clears on its own" when GitHub had
  /// never been asked -- the process refused the load because its rate-limit
  /// budget was under the reserve. Both halves of that sentence were wrong.
  it("names the budget rather than blaming GitHub, and offers no retry", () => {
    const { message, canRetry } = unmeasuredMessage(
      30,
      0,
      {
        kind: "budgetExhausted",
        remaining: 499,
        reserve: 500,
        resetAt: "2026-09-15T17:00:00Z",
      },
      NOW,
    );
    expect(message).toContain("budget is exhausted");
    expect(message).toContain("never issued");
    expect(message).toContain("499");
    // The actionable half: a time to come back, not merely a diagnosis.
    expect(message).toContain("40 minutes");
    expect(message).not.toContain("did not answer");
    expect(message).not.toContain("clears on its own");
    // A retry cannot succeed until the window rolls over, and offering one
    // invites exactly the loop the user was stuck in.
    expect(canRetry).toBe(false);
  });

  it("still blames SAML when GitHub refused fields", () => {
    const { message, canRetry } = unmeasuredMessage(30, 4, undefined, NOW);
    expect(message).toContain("SAML");
    expect(message).toContain("4 fields");
    expect(canRetry).toBe(false);
  });

  /// The one case a retry genuinely helps, and the only one that keeps the
  /// original wording -- which was correct for it all along.
  it("keeps the retryable message when GitHub simply did not answer", () => {
    const { message, canRetry } = unmeasuredMessage(30, 0, undefined, NOW);
    expect(message).toContain("did not answer");
    expect(message).toContain("clears on its own");
    expect(canRetry).toBe(true);
  });

  /// Absent is not a time. An unknown reset says so rather than inventing
  /// one, the same rule this codebase states as absent-is-not-zero.
  it("says the window resets without naming a time it does not know", () => {
    const { message } = unmeasuredMessage(
      30,
      0,
      { kind: "budgetExhausted", remaining: null, reserve: 500, resetAt: null },
      NOW,
    );
    expect(message).toContain("when the hourly window resets");
    expect(message).toContain("not reported");
    expect(message).not.toMatch(/\bin \d+ minute/);
  });
});
