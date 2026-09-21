import { cleanup, fireEvent, render, screen } from "@testing-library/react";
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
  const at = (number: number, created_at: string): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    created_at,
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

  it("defaults to oldest opened first, without touching the store", () => {
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
    expect(trigger.textContent).toContain("Oldest opened first");
    // "Oldest first" alone is the ambiguity #1277 was filed about.
    expect(trigger.textContent).toMatch(/opened/i);
  });

  it("switches to newest opened first", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest opened first" }));
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    expect(useFilters.getState().filtersByView["to-review"].readySort).toBe("newest-opened");
  });

  // The other half of the round trip. A separate render rather than
  // reopening the menu in the test above: this dropdown closes on select
  // and does not reopen within one synchronous `fireEvent` pass, so
  // chaining the two would be testing the menu's animation rather than
  // the ordering. Starting from `newest-opened` in the store is the state
  // the previous test leaves a real user in.
  it("switches back to oldest opened first", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readySort: "newest-opened" } },
      view: "to-review",
    });
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Oldest opened first" }));
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
  it("does not let an unparseable created_at sort as the oldest", () => {
    render(<ReadyStrip prs={[at(9, "not a date"), ...NEWEST_FIRST]} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3", "PR 9"]);
  });

  // The strip is the only list #1277 changes. Reordering it must not
  // touch `sort`, which the PR list below it reads and which stays
  // newest-first.
  it("leaves the main list's sort alone", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest opened first" }));
    expect(useFilters.getState().filtersByView["to-review"].sort).toBeUndefined();
  });
});
