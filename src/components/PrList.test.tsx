import { renderWithQuery as render } from "@/test-utils";
import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { PrList } from "@/components/PrList";
import { prKey } from "@/lib/prIdentity";
import type { PullRequest } from "@/types/pr";
import { useFilters } from "@/store/filters";

describe("PrList", () => {
  it("renders every PR with its number and title", () => {
    render(<PrList prs={PR_FIXTURES} />);
    expect(screen.getByText("Add retry to the fetch client")).toBeDefined();
    expect(screen.getByText(/#42/)).toBeDefined();
    expect(screen.getByText(/#43/)).toBeDefined();
  });

  /// #977: the list is the only thing that reads `selectedPr`, so it is
  /// the list that tells each row whether it is the open one -- the same
  /// division `cursored` and `stackedOn` already follow.
  it("marks exactly the open row as current", () => {
    const open = PR_FIXTURES[1];
    useFilters.setState({ selectedPr: { repo: open.repo, number: open.number } });
    const { container } = render(<PrList prs={PR_FIXTURES} onOpen={() => {}} />);
    const rows = [...container.querySelectorAll('[role="button"][aria-current]')];
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain(`#${open.number}`);
    useFilters.setState({ selectedPr: null });
  });

  it("marks no row current when nothing is open", () => {
    useFilters.setState({ selectedPr: null });
    const { container } = render(<PrList prs={PR_FIXTURES} onOpen={() => {}} />);
    expect(container.querySelectorAll("[aria-current]")).toHaveLength(0);
  });

  it("renders label pills", () => {
    render(<PrList prs={PR_FIXTURES} />);
    expect(screen.getByText("enhancement")).toBeDefined();
    expect(screen.getByText("bug")).toBeDefined();
  });

  it("marks drafts", () => {
    render(<PrList prs={PR_FIXTURES} />);
    expect(screen.getByText(/draft/i)).toBeDefined();
  });

  it("shows the open count in the header", () => {
    render(<PrList prs={PR_FIXTURES} />);
    expect(screen.getByText(/3 Open/)).toBeDefined();
  });

  // The copy is now condition-aware and spans two elements, so match the
  // headline rather than a substring that straddles both.
  it("renders an empty state rather than a bare list", () => {
    render(<PrList prs={[]} />);
    expect(screen.getByText(/no open pull requests/i)).toBeDefined();
  });

  /// Sorting moved out of PrList and into `sortPrs` (src/lib/derive.ts) --
  /// PrList now renders whatever order it's handed. The ordering test moved
  /// with it; see derive.test.ts's "orders newest first even when handed
  /// the list reversed".
  it("renders PRs in the exact order it is given, without re-sorting", () => {
    const reversed = [...PR_FIXTURES].reverse();
    render(<PrList prs={reversed} />);

    const rendered = screen.getAllByRole("link").map((el) => el.textContent);
    expect(rendered).toEqual(reversed.map((pr) => pr.title));
  });
});

// Deliberately reuse both provider-local node id and IID. Neither is a row key.
it("selects and checks only the matching source/project among overlapping IIDs", () => {
  const base = PR_FIXTURES[0];
  const prs: PullRequest[] = [
    { ...base, title: "GitHub row" },
    { ...base, title: "GitLab row", source: { provider: "gitlab", host: "gitlab.com" } },
    { ...base, title: "Other host", source: { provider: "gitlab", host: "gitlab.example" } },
    { ...base, title: "Nested project", repo: "group/subgroup/project", source: { provider: "gitlab", host: "gitlab.com" } },
  ];
  useFilters.setState({ selectedPr: prs[1], checked: [] });
  const { container } = render(<PrList prs={prs} selectable onOpen={() => {}} />);
  const current = container.querySelectorAll('[role="button"][aria-current]');
  expect(current).toHaveLength(1);
  expect(current[0].textContent).toContain("GitLab row");
  fireEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
  expect(useFilters.getState().checked).toEqual(prs.map(prKey));
  expect(new Set(useFilters.getState().checked).size).toBe(4);
  useFilters.setState({ selectedPr: null, checked: [] });
});
