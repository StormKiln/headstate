import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useFilters } from "@/store/filters";
import { RepoTable } from "./RepoTable";

const repos = [
  { repo: "acme/alpha", merged: 48 },
  { repo: "acme/beta", merged: 12 },
];

describe("RepoTable", () => {
  beforeEach(() => {
    useFilters.setState({ filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
  });

  it("lists repos with counts", () => {
    render(<RepoTable repos={repos} />);
    expect(screen.getByText("acme/alpha")).toBeTruthy();
    expect(screen.getByText("48")).toBeTruthy();
  });

  it("shows each repo's share of the total", () => {
    render(<RepoTable repos={repos} />);
    expect(screen.getByText("80%")).toBeTruthy(); // 48 of 60
  });

  it("shows an empty state", () => {
    render(<RepoTable repos={[]} />);
    expect(screen.getByText(/no merged pull requests/i)).toBeTruthy();
  });

  /// The table's purpose is navigation, not just display -- and it was a
  /// DEAD CLICK (#852).
  ///
  /// This test is how that shipped green. It asserted `panel === "list"`,
  /// which was already the default and stayed true whether or not the
  /// handler ran, and it read the repo out of
  /// `filtersByView[getState().view]` -- the view the click had NOT
  /// changed. So it passed while the user, on PR Stats, clicked a bar and
  /// nothing happened: `setView` was never called, and because `setFilter`
  /// writes into the ACTIVE view's set the repo landed in `pr-stats`' slot,
  /// which `StatsPage` does not read.
  ///
  /// Both halves are now named explicitly, and the view is asserted by
  /// LITERAL rather than by `getState().view`: reading the destination out
  /// of the store after the click is what let a click that moved nowhere
  /// look like one that arrived.
  it("switches to My PRs and scopes it to the repo when a row is clicked", () => {
    useFilters.setState({ view: "pr-stats" } as never);
    render(<RepoTable repos={repos} />);
    fireEvent.click(screen.getByText("acme/alpha"));
    expect(useFilters.getState().view).toBe("my-prs");
    expect(useFilters.getState().filtersByView["my-prs"].repo).toBe("acme/alpha");
  });

  /// The ORDER is load-bearing: `setFilter` writes to whichever view is
  /// active when it runs, so calling it before `setView` would put the repo
  /// in the stats view's own slot -- the original bug with an extra call.
  /// Asserted by checking the view the user CAME FROM is untouched.
  it("does not leave the repo in the stats view's filter set", () => {
    useFilters.setState({ view: "pr-stats" } as never);
    render(<RepoTable repos={repos} />);
    fireEvent.click(screen.getByText("acme/alpha"));
    expect(useFilters.getState().filtersByView["pr-stats"].repo).toBeUndefined();
  });
});
