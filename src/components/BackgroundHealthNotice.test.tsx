import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BackgroundHealthNotice } from "./BackgroundHealthNotice";
import type { TaskHealth } from "@/api/tauri";

const NOW = 1_700_000_000_000;

const task = (over: Partial<TaskHealth> = {}): TaskHealth => ({
  task: "health-sampler",
  consecutive_failures: 0,
  total_failures: 0,
  last_error: null,
  last_success_ms: NOW - 60_000,
  degraded: false,
  ...over,
});

describe("BackgroundHealthNotice", () => {
  it("renders nothing while the loops are working", () => {
    // A permanent "the sampler is fine" line is the kind of caveat
    // nobody reads, which makes the one that matters invisible too.
    const { container } = render(<BackgroundHealthNotice tasks={[task()]} now={NOW} />);
    expect(container.textContent).toBe("");
  });

  it("renders nothing before the query answers", () => {
    // "Not asked yet" must not paint the alarming state.
    const { container } = render(<BackgroundHealthNotice tasks={undefined} now={NOW} />);
    expect(container.textContent).toBe("");
  });

  it("stays silent on a single failure", () => {
    // One blip is weather. The threshold lives in Rust so this and any
    // future notifier cannot disagree about what counts -- this asserts
    // the component honours `degraded` rather than the raw count.
    render(
      <BackgroundHealthNotice
        tasks={[task({ consecutive_failures: 1, degraded: false, last_error: "blip" })]}
        now={NOW}
      />,
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says the app was RUNNING, which is the whole point", () => {
    // THE correction. The page otherwise tells the user a gap means the
    // app was closed -- the reassuring reading of two identical-looking
    // states (#1042's shape).
    render(
      <BackgroundHealthNotice
        tasks={[
          task({ consecutive_failures: 12, degraded: true, last_error: "disk full" }),
        ]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/The app has been running/)).toBeTruthy();
    expect(screen.getByText(/missing data rather than time the app was closed/)).toBeTruthy();
  });

  it("names the loop and quotes the reason verbatim", () => {
    // Without the reason the user knows only that something is wrong,
    // which is where they were before this existed.
    render(
      <BackgroundHealthNotice
        tasks={[task({ degraded: true, consecutive_failures: 3, last_error: "database is locked" })]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/The health sampler has failed 3 times in a row/)).toBeTruthy();
    expect(screen.getByText("database is locked")).toBeTruthy();
  });

  it("distinguishes never-succeeded from a stale success", () => {
    // `null` with a non-zero failure count is a third state (#1042):
    // not working, not unstarted, but never once succeeded. Rendering
    // it as a time would be a confident wrong answer.
    render(
      <BackgroundHealthNotice
        tasks={[task({ degraded: true, consecutive_failures: 5, last_success_ms: null })]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/has not succeeded once since the app started/)).toBeTruthy();
  });

  it("says how long ago a stale success was", () => {
    render(
      <BackgroundHealthNotice
        tasks={[
          task({ degraded: true, consecutive_failures: 5, last_success_ms: NOW - 2 * 3_600_000 }),
        ]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/Last succeeded 2 hours ago/)).toBeTruthy();
  });

  it("uses the Claude loop's own consequence, not the chart's", () => {
    // The sampler's failure means a gap in the chart; the live pass's
    // means stale session history. One sentence for both would be wrong
    // for whichever it was not written about.
    render(
      <BackgroundHealthNotice
        tasks={[task({ task: "claude-live", degraded: true, consecutive_failures: 4 })]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/The Claude Code session reader/)).toBeTruthy();
    expect(screen.getByText(/Session history and crash detection/)).toBeTruthy();
    expect(screen.queryByText(/gap in the chart/)).toBeNull();
  });

  it("reports both loops when both are broken", () => {
    render(
      <BackgroundHealthNotice
        tasks={[
          task({ degraded: true, consecutive_failures: 2 }),
          task({ task: "claude-live", degraded: true, consecutive_failures: 9 }),
        ]}
        now={NOW}
      />,
    );
    expect(screen.getByText(/The health sampler/)).toBeTruthy();
    expect(screen.getByText(/The Claude Code session reader/)).toBeTruthy();
  });
});
