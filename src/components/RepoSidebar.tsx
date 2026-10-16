import type { PullRequest } from "@/types/pr";
import { type View, useActiveFilters, useFilters } from "@/store/filters";
import { ViewSwitcher } from "@/components/ViewSwitcher";
import { current } from "@/lib/ariaCurrent";
import { repoCounts } from "@/lib/repos";

export function RepoSidebar({
  prs,
  viewCounts,
}: {
  prs: PullRequest[];
  /// Badge counts for the switcher, e.g. how many PRs await review.
  viewCounts?: Partial<Record<View, number>>;
}) {
  const filters = useActiveFilters();
  const { setFilter, view } = useFilters();
  const counts = repoCounts(prs);

  const rowClass = (active: boolean) =>
    `flex w-full items-center justify-between rounded px-3 py-2 text-sm ${
      active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
    }`;

  // `current` from `@/lib/ariaCurrent` (#852, shared in #977). The blue was
  // carrying the selection on its own, so a screen reader reading a list of
  // repository names had no way to know which one was open. The helper's
  // own doc comment carries the `"true"`-not-`"page"` and
  // `undefined`-not-`"false"` reasoning that used to live here; it was
  // moved because four sidebars each had a private copy of it and the two
  // newest lists reached for `aria-pressed` instead.

  const repoActive = view === "my-prs";

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <button
          type="button"
          onClick={() => setFilter("repo", undefined)}
          aria-current={current(repoActive && !filters.repo)}
          className={rowClass(repoActive && !filters.repo)}
        >
          <span>All repositories</span>
          <span>{prs.length}</span>
        </button>
        {counts.map(({ repo, count }) => (
          <button
            type="button"
            key={repo}
            onClick={() => setFilter("repo", repo)}
            aria-current={current(repoActive && filters.repo === repo)}
            className={rowClass(repoActive && filters.repo === repo)}
          >
            <span className="truncate">{repo}</span>
            <span className="ml-2 shrink-0">{count}</span>
          </button>
        ))}
      </div>
    </nav>
  );
}
