import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { PullRequest } from "@/types/pr";
import { PR_FIXTURES } from "@/fixtures/prs";
import { applyFilters, needsMyReview } from "@/lib/derive";
import { useFilters } from "@/store/filters";
import { ReviewChips } from "./ReviewChips";

const EMPTY = { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {} } as const;
const pr = (over: Partial<PullRequest>): PullRequest => ({ ...PR_FIXTURES[0], ...over });

const PRS: PullRequest[] = [
  pr({ number: 1, review: "none", is_draft: false }),
  pr({ number: 2, review: "approved", is_draft: false }),
  pr({ number: 3, review: "changes_requested", is_draft: false }),
  pr({ number: 4, review: "none", is_draft: true }),
];

describe("needsMyReview", () => {
  // The whole point: this is NOT needsAttention. Someone else's red CI is
  // not the reviewer's problem, and counting it would badge the tray with
  // other people's broken builds.
  it("ignores CI and merge state entirely", () => {
    expect(needsMyReview(pr({ ci: "failure", merge: "conflicted", review: "none" }))).toBe(true);
  });

  it("is satisfied once I have given a verdict", () => {
    expect(needsMyReview(pr({ review: "approved" }))).toBe(false);
    expect(needsMyReview(pr({ review: "changes_requested" }))).toBe(false);
  });

  it("excludes drafts, which are not ready for review", () => {
    expect(needsMyReview(pr({ review: "none", is_draft: true }))).toBe(false);
  });
});

describe("ReviewChips", () => {
  beforeEach(() =>
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" }),
  );

  it("counts only PRs still awaiting my verdict", () => {
    render(<ReviewChips prs={PRS} />);
    const btn = screen.getByRole("button", { name: /awaiting my review/i });
    expect(btn.textContent).toContain("1");
  });

  it("filters to them on click", () => {
    render(<ReviewChips prs={PRS} />);
    fireEvent.click(screen.getByRole("button", { name: /awaiting my review/i }));
    const s = useFilters.getState();
    expect(s.filtersByView[s.view].needsMyReviewOnly).toBe(true);
  });

  // Same invariant the author-side chips carry: a chip must never open a
  // list that disagrees with the number on it.
  it("every chip's count equals what its own filter yields", () => {
    render(<ReviewChips prs={PRS} />);
    for (const btn of screen.getAllByRole("button")) {
      const shown = Number(btn.textContent?.match(/\d+/)?.[0]);
      fireEvent.click(btn);
      const s = useFilters.getState();
      expect(applyFilters(PRS, s.filtersByView[s.view]).length).toBe(shown);
      useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" });
    }
  });

  /// The assertion this file did not have, which is why #971 survived
  /// here after `TriageChips` was fixed for exactly this.
  ///
  /// `applyPreset` replaces the filter set; `setFilter` merges into it.
  /// So clicking a chip and then typing a search leaves the chip's own
  /// keys set, and a subset test kept reading pressed while the list had
  /// been narrowed by something else entirely. For a screen reader that
  /// is a false statement about the current view, and clicking to
  /// un-press silently discarded the search term.
  it("is not pressed once another filter also narrows the list", () => {
    render(<ReviewChips prs={PRS} />);
    const name = /awaiting my review/i;
    fireEvent.click(screen.getByRole("button", { name }));
    expect(screen.getByRole("button", { name }).getAttribute("aria-pressed")).toBe("true");

    // A search arrives from the filter bar, merged into the same set.
    act(() => useFilters.getState().setFilter("query", "retry"));
    expect(screen.getByRole("button", { name }).getAttribute("aria-pressed")).toBe("false");
  });

  /// The repo is navigation, not a filter -- the same rule
  /// `hasActiveFilters` and the store's `reset` apply -- so being on a
  /// repo page must not un-press the chip.
  it("stays pressed while scoped to a repository", () => {
    render(<ReviewChips prs={PRS} />);
    const name = /awaiting my review/i;
    fireEvent.click(screen.getByRole("button", { name }));
    act(() => useFilters.getState().setFilter("repo", "octocat/hello-world"));
    expect(screen.getByRole("button", { name }).getAttribute("aria-pressed")).toBe("true");
  });

  /// A chip for a DIFFERENT preset must not read as pressed either --
  /// the two review chips are both members of `OTHER_FILTERS`, so each
  /// sits in the other's blast radius.
  it("un-presses when the sibling chip's filter is also set", () => {
    render(<ReviewChips prs={PRS} />);
    fireEvent.click(screen.getByRole("button", { name: /awaiting my review/i }));
    act(() => useFilters.getState().setFilter("draftsOnly", true));
    expect(
      screen.getByRole("button", { name: /awaiting my review/i }).getAttribute("aria-pressed"),
    ).toBe("false");
  });

  it("renders nothing when there is nothing to triage", () => {
    const { container } = render(<ReviewChips prs={[pr({ review: "approved" })]} />);
    expect(container.querySelectorAll("button")).toHaveLength(0);
  });
});
