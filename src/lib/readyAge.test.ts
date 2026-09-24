import { describe, expect, it } from "vitest";
import { readyAge } from "./readyAge";

const NOW = new Date("2026-09-10T12:00:00Z");
const hoursAgo = (h: number) => new Date(NOW.getTime() - h * 3_600_000).toISOString();

describe("readyAge", () => {
  it("colours by elapsed time since ready: green to 24h, yellow to 48h, red after", () => {
    expect(readyAge(hoursAgo(3), NOW)).toMatchObject({ text: "3h", tone: "fresh" });
    expect(readyAge(hoursAgo(30), NOW)).toMatchObject({ text: "1d 6h", tone: "aging" });
    expect(readyAge(hoursAgo(60), NOW)).toMatchObject({ text: "2d 12h", tone: "stale" });
  });

  // The boundaries are inclusive on the calmer side: "≤ 24 hours" is green.
  it("puts exactly 24h in green and exactly 48h in yellow", () => {
    expect(readyAge(hoursAgo(24), NOW).tone).toBe("fresh");
    expect(readyAge(hoursAgo(24.01), NOW).tone).toBe("aging");
    expect(readyAge(hoursAgo(48), NOW).tone).toBe("aging");
    expect(readyAge(hoursAgo(48.01), NOW).tone).toBe("stale");
  });

  it("writes a compact age", () => {
    expect(readyAge(hoursAgo(5), NOW).text).toBe("5h");
    expect(readyAge(hoursAgo(28), NOW).text).toBe("1d 4h");
    expect(readyAge(hoursAgo(24), NOW).text).toBe("1d");
    expect(readyAge(hoursAgo(72), NOW).text).toBe("3d");
    expect(readyAge(hoursAgo(75), NOW).text).toBe("3d");
    expect(readyAge(hoursAgo(0.75), NOW).text).toBe("45m");
    expect(readyAge(hoursAgo(0), NOW).text).toBe("<1m");
  });

  // Absent is not zero: an unknown age is never green.
  it("reads an absent or unparseable time as unknown, not fresh", () => {
    for (const bad of [undefined, null, "", "yesterday"]) {
      const age = readyAge(bad, NOW);
      expect(age.tone).toBe("unknown");
      expect(age.text).toBe("age unknown");
      expect(age.since).toBeNull();
    }
  });

  // A ready time a few seconds ahead of this machine's clock is skew,
  // not a negative age.
  it("clamps a time slightly in the future to under a minute", () => {
    expect(readyAge(hoursAgo(-0.01), NOW)).toMatchObject({ text: "<1m", tone: "fresh" });
  });

  // Further ahead than skew is a wrong time, and a wrong time must not
  // print as a confident green "<1m".
  it("reads a time well in the future as unknown", () => {
    expect(readyAge(hoursAgo(-2), NOW).tone).toBe("unknown");
  });

  it("carries the exact ready time for the accessible label", () => {
    expect(readyAge(hoursAgo(3), NOW).since?.toISOString()).toBe("2026-09-10T09:00:00.000Z");
  });
});
