import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: () => Promise.resolve() }));

import { ReadyStrip } from "./ReadyStrip";
import { PR_FIXTURES } from "../fixtures/prs";
import { useFilters } from "@/store/filters";
import type { PullRequest } from "@/types/pr";

afterEach(cleanup);

const EMPTY = { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, repositories: {}, "system-health": {} } as const;

beforeEach(() =>
  useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" }),
);

const ready: PullRequest = {
  ...PR_FIXTURES[0],
  title: "Ready one",
  is_draft: false,
  ci: "success",
  merge: "mergeable",
  review: "none",
  in_merge_queue: false,
};

describe("ReadyStrip", () => {
  it("lists what a reviewer can pick up", () => {
    render(<ReadyStrip prs={[ready]} onOpen={vi.fn()} />);
    expect(screen.getByText("Ready one")).toBeTruthy();
    expect(screen.getByText(/ready for review \(1\)/i)).toBeTruthy();
  });

  it("leaves out what is not ready", () => {
    render(<ReadyStrip prs={[{ ...ready, ci: "failure" }]} onOpen={vi.fn()} />);
    expect(screen.queryByText("Ready one")).toBeNull();
  });

  // Matches the attention strip: a section that shouts when there is
  // nothing in it stops being read.
  it("stays quiet when nothing is ready", () => {
    render(<ReadyStrip prs={[]} onOpen={vi.fn()} />);
    expect(screen.getByText(/nothing ready to review/i)).toBeTruthy();
    expect(screen.queryByText(/ready for review \(/i)).toBeNull();
  });

  it("opens the detail view when clicked", () => {
    const onOpen = vi.fn();
    render(<ReadyStrip prs={[ready]} onOpen={onOpen} />);
    fireEvent.click(screen.getByText("Ready one"));
    expect(onOpen).toHaveBeenCalledWith(ready);
  });

  it("is keyboard reachable, like the attention strip", () => {
    const onOpen = vi.fn();
    render(<ReadyStrip prs={[ready]} onOpen={onOpen} />);
    fireEvent.keyDown(screen.getByRole("button", { name: /ready one/i }), { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith(ready);
  });

  // With nothing to open, the entry must not look interactive.
  it("does not pretend to be clickable without a handler", () => {
    render(<ReadyStrip prs={[ready]} />);
    expect(screen.queryByRole("button", { name: /ready one/i })).toBeNull();
  });
});

/// #1277: working top to bottom through a review queue should mean
/// working through it in the order the pull requests arrived.
describe("ReadyStrip ordering", () => {
  // All opened at the same instant, so only `ready_at` can order them
  // (#1407).
  const at = (number: number, ready_at: string): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    created_at: "2026-08-01T00:00:00Z",
    ready_at,
  });

  // Handed in NEWEST-first order deliberately, so a component that does
  // not sort at all fails rather than passing on the input's shape.
  const NEWEST_FIRST = [
    at(3, "2026-09-03T00:00:00Z"),
    at(2, "2026-09-02T00:00:00Z"),
    at(1, "2026-09-01T00:00:00Z"),
  ];

  const titlesInOrder = () =>
    screen
      .getAllByRole("button", { name: /^PR \d/ })
      .map((el) => el.textContent?.match(/PR \d/)?.[0]);

  it("defaults to oldest ready first, without touching the store", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3"]);
    // The default is a DEFAULT, not a value written on first render: a
    // store key nobody chose would persist and then outlive a change to
    // what the default should be.
    const s = useFilters.getState();
    expect(s.filtersByView["to-review"].readySort).toBeUndefined();
  });

  // A default nobody can see is one nobody can trust, and this list
  // having a non-obvious default is the point of the issue.
  it("names the field it sorts on, not just the direction", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: /sort/i });
    expect(trigger.textContent).toContain("Oldest ready first");
    // "Oldest first" alone is the ambiguity #1277 was filed about.
    expect(trigger.textContent).toMatch(/ready/i);
  });

  it("switches to newest ready first", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest ready first" }));
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    expect(useFilters.getState().filtersByView["to-review"].readySort).toBe("newest-opened");
  });

  // The other half of the round trip. A separate render rather than
  // reopening the menu in the test above: this dropdown closes on select
  // and does not reopen within one synchronous `fireEvent` pass, so
  // chaining the two would be testing the menu's animation rather than
  // the ordering. Starting from `newest-opened` in the store is the state
  // the previous test leaves a real user in.
  it("switches back to oldest ready first", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readySort: "newest-opened" } },
      view: "to-review",
    });
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Oldest ready first" }));
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3"]);
    expect(useFilters.getState().filtersByView["to-review"].readySort).toBe("oldest-opened");
  });

  it("honours a readySort already in the store", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readySort: "newest-opened" } },
      view: "to-review",
    });
    render(<ReadyStrip prs={[...NEWEST_FIRST].reverse()} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
  });

  /// An undated row at the TOP would claim to be the longest-waiting work
  /// and push genuinely old pull requests down -- the one outcome this
  /// ordering exists to prevent.
  it("does not let an unparseable ready_at sort as the oldest", () => {
    render(<ReadyStrip prs={[at(9, "not a date"), ...NEWEST_FIRST]} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3", "PR 9"]);
  });

  // The strip is the only list #1277 changes. Reordering it must not
  // touch `sort`, which the PR list below it reads and which stays
  // newest-first.
  it("leaves the main list's sort alone", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest ready first" }));
    expect(useFilters.getState().filtersByView["to-review"].sort).toBeUndefined();
  });
});

/// #1407: each row says how long it has been ready for review, coloured
/// green to 24h, yellow to 48h, red after.
describe("ReadyStrip age", () => {
  const NOW = new Date("2026-09-10T12:00:00Z");
  const hoursAgo = (h: number) => new Date(NOW.getTime() - h * 3_600_000).toISOString();
  const row = (number: number, ready_at: string | null | undefined): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    // Opened a month earlier, so an age measured from `created_at` would
    // read red on every row and fail the green case below.
    created_at: "2026-08-10T12:00:00Z",
    ready_at,
  });
  const ageOf = (number: number) =>
    screen
      .getByRole("button", { name: new RegExp(`^PR ${number}(?!\\d)`) })
      .querySelector("[data-ready-age]") as HTMLElement;

  // Fake timers pin `Date` too, so no assertion depends on the machine
  // clock -- the Rust suite has burned a release on exactly that.
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows green, yellow and red ages, each with its text", () => {
    render(
      <ReadyStrip
        prs={[row(1, hoursAgo(3)), row(2, hoursAgo(30)), row(3, hoursAgo(60))]}
        onOpen={vi.fn()}
      />,
    );
    expect(ageOf(1).textContent).toBe("3h");
    expect(ageOf(1).dataset.readyAge).toBe("fresh");
    expect(ageOf(1).className).toContain("#3fb950");
    expect(ageOf(2).textContent).toBe("1d 6h");
    expect(ageOf(2).dataset.readyAge).toBe("aging");
    expect(ageOf(2).className).toContain("#d29922");
    expect(ageOf(3).textContent).toBe("2d 12h");
    expect(ageOf(3).dataset.readyAge).toBe("stale");
    expect(ageOf(3).className).toContain("#f85149");
  });

  // Absent is not zero: never green, and never a number.
  it("renders an unknown age as a neutral 'age unknown'", () => {
    render(
      <ReadyStrip prs={[row(1, undefined), row(2, null), row(3, "garbage")]} onOpen={vi.fn()} />,
    );
    for (const n of [1, 2, 3]) {
      expect(ageOf(n).textContent).toBe("age unknown");
      expect(ageOf(n).dataset.readyAge).toBe("unknown");
      expect(ageOf(n).className).not.toContain("#3fb950");
      expect(ageOf(n).className).toContain("#8b949e");
    }
  });

  it("gives the exact ready time in the accessible name and the title", () => {
    const at = hoursAgo(3);
    const since = new Date(at).toLocaleString();
    render(<ReadyStrip prs={[row(1, at)]} onOpen={vi.fn()} />);
    expect(
      screen.getByRole("button", { name: new RegExp(`ready for review since ${since}`) }),
    ).toBeTruthy();
    expect(ageOf(1).getAttribute("title")).toBe(`Ready for review since ${since}`);
    expect(ageOf(1).getAttribute("datetime")).toBe(at);
  });

  it("says in the accessible name when the ready time is unknown", () => {
    render(<ReadyStrip prs={[row(1, null)]} onOpen={vi.fn()} />);
    expect(screen.getByRole("button", { name: /ready-for-review time unknown/i })).toBeTruthy();
  });

  it("shows the age on a plain-link row too", () => {
    render(<ReadyStrip prs={[row(1, hoursAgo(3))]} />);
    expect(screen.getByText("3h")).toBeTruthy();
  });

  // The age keeps counting while the view is open, from the clock and not
  // from a re-fetch: the same `prs` prop crosses from green to yellow.
  it("stays current while the view is open", () => {
    render(<ReadyStrip prs={[row(1, hoursAgo(23.99))]} onOpen={vi.fn()} />);
    expect(ageOf(1).dataset.readyAge).toBe("fresh");
    act(() => {
      vi.advanceTimersByTime(2 * 60_000);
    });
    expect(ageOf(1).dataset.readyAge).toBe("aging");
    expect(ageOf(1).textContent).toBe("1d");
  });
});
