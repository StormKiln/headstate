import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { PrRow } from "./PrRow";
import { useFilters } from "@/store/filters";
import { PR_FIXTURES } from "../fixtures/prs";

afterEach(() => {
  cleanup();
  useFilters.setState({ density: "comfortable" });
});

function show(pr = PR_FIXTURES[0]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrap = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return render(<PrRow pr={pr} />, { wrapper: wrap });
}

const manyLabels = {
  ...PR_FIXTURES[0],
  labels: [
    { name: "bug", color: "d73a4a" },
    { name: "area:ci", color: "0e8a16" },
    { name: "priority", color: "1d76db" },
    { name: "needs-triage", color: "fbca04" },
    { name: "stale", color: "cfd3d7" },
  ],
  // GitHub's own count, and equal to the list: this PR carries five
  // labels and the query returned all five. The "+N" is what DENSE mode
  // hides, not what the query cut -- the truncated case is a different
  // fixture below, and conflating them would let one pass for the other.
  labels_total: 5,
};

/// Five labels on the row, but GitHub says there are 28 -- MEASURED as
/// the real ceiling on open kubernetes/kubernetes pull requests against
/// this query's window of 20.
///
/// Before #1089 the "+N" was `labels.length - shown`, computed from a
/// list the query had already cut, so a row with 28 labels claimed the
/// same "+3" as one with 8. The count now comes from `labels_total`.
const truncatedLabels = { ...manyLabels, labels_total: 28 };

/// Rows are two lines at py-3, so roughly 8-11 fit on screen and a 25-PR
/// list needs 2-3 screens -- the 13 attention items are never
/// co-visible. Worktree and Docker rows already use py-2.5; the PR list,
/// the most item-heavy surface in the app, was the LEAST dense.
describe("density", () => {
  it("renders both lines when comfortable", () => {
    show();
    expect(screen.getByText(/opened/)).toBeTruthy();
  });

  // Dense keeps every DECISIVE signal -- CI, review, title, number --
  // and drops only the prose line, which repeats what the glyphs say.
  it("keeps the decisive signals when dense", () => {
    useFilters.setState({ density: "dense" });
    show();
    expect(screen.getByText(PR_FIXTURES[0].title)).toBeTruthy();
    expect(screen.getByText(new RegExp(`#${PR_FIXTURES[0].number}`))).toBeTruthy();
  });

  it("drops the prose metadata line when dense", () => {
    useFilters.setState({ density: "dense" });
    show();
    expect(screen.queryByText(/opened .* by /)).toBeNull();
  });

  // Unbounded labels can wrap the flex line and push decisive state off
  // the row, which is exactly the density being reclaimed.
  it("caps labels and says how many are hidden", () => {
    useFilters.setState({ density: "dense" });
    show(manyLabels);
    expect(screen.getByText("bug")).toBeTruthy();
    expect(screen.getByText("+3")).toBeTruthy();
    expect(screen.queryByText("stale")).toBeNull();
  });

  /// #1089: the "+N" counted against the list the query returned, which
  /// the query had itself capped at 20. A pull request with 28 labels
  /// therefore claimed the same "+3" as one with 8 -- an exact-looking
  /// number computed from truncated input.
  ///
  /// 28 is MEASURED, not invented: open kubernetes/kubernetes pull
  /// requests reach it against this query's window.
  it("counts hidden labels against GitHub's total, not the list it was sent", () => {
    useFilters.setState({ density: "dense" });
    show(truncatedLabels);
    // 28 exist, 2 are shown.
    expect(screen.getByText("+26")).toBeTruthy();
    // And NOT the figure the cut list would have produced.
    expect(screen.queryByText("+3")).toBeNull();
  });

  /// Comfortable mode renders every label it HAS, which is exactly when
  /// the reader has no other clue that more exist -- so the shortfall
  /// must show there too. Before #1089 this line drew only when dense,
  /// so the fullest view was the least honest one.
  it("says what the query cut even when every fetched label is shown", () => {
    useFilters.setState({ density: "comfortable" });
    show(truncatedLabels);
    expect(screen.getByText("stale")).toBeTruthy();
    // Five shown of 28.
    expect(screen.getByText("+23")).toBeTruthy();
  });

  /// ABSENT IS NOT ZERO, and here the clamp is genuinely load-bearing:
  /// with nothing cut there must be no "+N" at all. A row that grew a
  /// "+0" would be claiming a shortfall it does not have.
  it("shows no shortfall when the labels arrived whole", () => {
    useFilters.setState({ density: "comfortable" });
    show(manyLabels);
    expect(screen.queryByText(/^\+\d+$/)).toBeNull();
  });

  // Density is a VIEW preference, not a filter: it changes how rows
  // look, never which ones are shown. "Clear filters" must not reset it.
  it("survives clearing the filters", () => {
    useFilters.setState({ density: "dense" });
    useFilters.getState().reset();
    expect(useFilters.getState().density).toBe("dense");
  });

  it("shows every label when comfortable", () => {
    show(manyLabels);
    expect(screen.getByText("stale")).toBeTruthy();
    expect(screen.queryByText("+3")).toBeNull();
  });
});
