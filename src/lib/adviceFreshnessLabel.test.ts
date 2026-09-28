import { describe, expect, it } from "vitest";
import { freshnessLabel } from "./adviceFreshnessLabel";
import type { ClaudeMdAdviceFreshness } from "@/types/pr";

const NOW = new Date("2026-01-01T12:00:00Z");
const AN_HOUR_AGO = "2026-01-01T11:00:00Z";

const label = (f: ClaudeMdAdviceFreshness, refreshing = false) =>
  freshnessLabel(f, AN_HOUR_AGO, refreshing, NOW);

describe("freshnessLabel", () => {
  /// Only `"fresh"` may say a report is up to date, and it says so the
  /// same way whether the producers ran or the fingerprint was verified
  /// -- both mean every tracked input was read.
  it("calls a fresh report up to date, however it became fresh", () => {
    expect(label({ state: "fresh", recomputed: true })).toMatchObject({
      text: "Up to date",
      tone: "current",
    });
    expect(label({ state: "fresh", recomputed: false })).toMatchObject({
      text: "Up to date",
      tone: "current",
    });
    // And the DETAIL distinguishes them, because "we just ran" and "we
    // checked that nothing changed" are different evidence for the same
    // claim.
    expect(label({ state: "fresh", recomputed: true }).detail).toContain("every check ran");
    expect(label({ state: "fresh", recomputed: false }).detail).toContain("nothing has changed");
  });

  /// #1424: "recomputed" is how the report became fresh, not a second
  /// time stamp. It once appended ", just now" to `relativeTime`, which
  /// read "Checked just now, just now" on a new report and "Checked 1
  /// hour ago, just now" -- a contradiction -- on an older one.
  it("states a recomputed report's time once, and never contradicts it", () => {
    const hourOld = label({ state: "fresh", recomputed: true }).detail;
    expect(hourOld).not.toContain("just now");
    const brandNew = freshnessLabel(
      { state: "fresh", recomputed: true },
      NOW.toISOString(),
      false,
      NOW,
    ).detail;
    expect(brandNew.match(/just now/g)?.length ?? 0).toBeLessThanOrEqual(1);
  });

  /// An unchanged cached report is current in substance and says where
  /// it came from anyway: serving from cache is a fact the user is
  /// entitled to.
  it("says a cached unchanged report came from the last check", () => {
    expect(label({ state: "cached", stale: false })).toMatchObject({
      text: "From the last check",
      tone: "current",
    });
  });

  /// A stale cached report is explicitly NOT current, and the age it
  /// carries is when the PRODUCERS ran, not when the call answered.
  it("says a stale cached report is out of date, with its age", () => {
    const l = label({ state: "cached", stale: true });
    expect(l.text).toMatch(/Out of date/);
    expect(l.tone).toBe("stale");
    expect(l.detail).toBe("Checked 1 hour ago");
  });

  /// THE composed sentence. It is a claim about this client's own
  /// in-flight request, which is why it can be said at all -- the
  /// backend has no `"refreshing"` freshness to hand out.
  it("says a new check is running over a stale report", () => {
    expect(label({ state: "cached", stale: true }, true).text).toBe(
      "Out of date — showing the last check while a new one runs",
    );
  });

  /// `"unverified"` is not fresh with a footnote (#1042). The word
  /// "current" must not appear in its claim, in any form -- a softened
  /// currency assertion over a report whose inputs were not all read is
  /// the exact lie the three states exist to prevent.
  it("never calls an unverified report current, in either direction", () => {
    for (const refreshing of [false, true]) {
      for (const recomputed of [false, true]) {
        const l = label({ state: "unverified", reason: "~/.claude: denied", recomputed }, refreshing);
        expect(l.tone).toBe("unknown");
        expect(l.text).toMatch(/Currency unknown/);
        expect(l.text).not.toMatch(/current(?!cy)/i);
        expect(l.text).not.toMatch(/up to date|from the last check/i);
        // The producer's own reason, verbatim: a permission wall and a
        // deleted file send the reader to different places.
        expect(l.detail).toContain("~/.claude: denied");
      }
    }
  });

  /// A refresh in flight downgrades the tone away from `"current"`
  /// wherever it applies, so nothing reads as a settled green claim
  /// while it is being re-decided.
  it("is never toned current while a refresh is in flight", () => {
    const states: ClaudeMdAdviceFreshness[] = [
      { state: "fresh", recomputed: true },
      { state: "fresh", recomputed: false },
      { state: "cached", stale: false },
      { state: "cached", stale: true },
      { state: "unverified", reason: "r", recomputed: false },
    ];
    for (const s of states) {
      expect(label(s, true).tone).not.toBe("current");
      expect(label(s, true).text).toMatch(/re-?checking now|new one runs/i);
    }
  });
});
