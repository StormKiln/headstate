import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SessionsChart } from "./SessionsChart";
import type { ClaudeDayCount } from "@/types/pr";

/// A complete window, the way the backend always returns it: one bucket
/// per day INCLUDING the empty ones.
const window30 = (): ClaudeDayCount[] =>
  Array.from({ length: 30 }, (_, i) => ({
    day: `2026-08-${String(i + 1).padStart(2, "0")}`,
    // Mostly zero, one spike. The zeros are the interesting part.
    started: i === 29 ? 34 : 0,
  }));

describe("SessionsChart", () => {
  it("names what it counts and over what window", () => {
    render(<SessionsChart points={window30()} days={30} />);
    expect(screen.getByText("Sessions started per day")).toBeTruthy();
    expect(screen.getByText(/the last 30 days/)).toBeTruthy();
  });

  /// The buckets are UTC days and the label has to say so: a 9pm Pacific
  /// session lands in the next day's column, and an undisclosed
  /// off-by-one-day chart is worse than a labelled one.
  ///
  /// `ActivityChart` discloses the same boundary for the same reason.
  it("discloses that the days are UTC", () => {
    render(<SessionsChart points={window30()} days={30} />);
    expect(screen.getByText(/\(UTC\)/)).toBeTruthy();
  });

  /// The subtitle states the INTENDED window, not the array length.
  ///
  /// So a backend that returned a short window renders a visible mismatch
  /// between the words and the bars rather than a label that papers over
  /// it. Sabotage: `days={points.length}` inside the component makes a
  /// 14-bucket response silently read as "the last 14 days", and the
  /// mirrored-constant drift `ACTIVITY_DAYS` guards against becomes
  /// undetectable.
  it("names the intended window rather than the array it got", () => {
    render(<SessionsChart points={window30().slice(0, 14)} days={30} />);
    expect(screen.getByText(/the last 30 days/)).toBeTruthy();
  });

  /// An EMPTY series says no data arrived; it does not draw a bare axis.
  ///
  /// A chart with an axis and no bars reads as "no activity in this
  /// period", which is a measurement. An empty array from a backend that
  /// always zero-fills is a bug, and the two must not look the same.
  it("says no buckets were returned rather than drawing an empty axis", () => {
    render(<SessionsChart points={[]} days={30} />);
    expect(screen.getByText(/No daily buckets were returned/)).toBeTruthy();
  });

  /// A window of all zeros is a MEASURED quiet month and IS drawn.
  ///
  /// The other side of the test above, and the one that keeps the fix from
  /// becoming "refuse to draw quiet periods". Sabotage: treating an
  /// all-zero window as empty would hide a real answer -- and, worse,
  /// would make a genuinely quiet month indistinguishable from a failure,
  /// which is the confusion the two states exist to separate.
  it("draws a window of measured zeroes rather than calling it absent", () => {
    const quiet = window30().map((d) => ({ ...d, started: 0 }));
    render(<SessionsChart points={quiet} days={30} />);
    expect(screen.queryByText(/No daily buckets were returned/)).toBeNull();
    expect(screen.getByText("Sessions started per day")).toBeTruthy();
  });
});
