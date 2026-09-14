import type { PullRequest } from "@/types/pr";
import { type View, useActiveFilters, useFilters } from "@/store/filters";
import { ViewSwitcher } from "@/components/ViewSwitcher";
import { current } from "@/lib/ariaCurrent";
import { repoCounts } from "@/lib/repos";

/// Repos where the user currently has open PRs, busiest first, plus an
/// always-first "All repositories" entry that is the default (no `repo`
/// filter set). Selecting a repo writes through the shared filter store --
/// this component holds no filter state of its own.
///
/// This is the sidebar for My PRs, and the fall-through for any future
/// view that has no column of its own. Stats used to own a pinned row at
/// the bottom of it, outside the scroll area so it stayed reachable
/// however many repos the list grew to. That row is gone: the destination
/// moved into `ViewSwitcher`, which is where the rest of the app's
/// navigation already lived.
///
/// It also SERVED PR Stats between #794 and #825, with the repo rows live
/// but read by nothing. That is over: #825 gave PR Stats its own
/// `StatsSidebar`, a GitHub-sourced hierarchy of organisations,
/// repositories and members, because a list of repositories where the
/// viewer has an open PR cannot hold an organisation or a person and so
/// could not express "how is my team doing?". The `repoActive` guard below
/// is what is left of the arrangement, and it is still worth keeping --
/// see its comment.
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

  // My PRs is the only view whose `filters.repo` this column both writes
  // and has read back. PR Stats was the other until #825 gave it
  // `StatsSidebar`, and it is dropped from this list rather than left in:
  // a highlight here would be for a filter key that view no longer uses
  // (its scope lives in `statsScopeKind` / `statsScopeValue` now).
  //
  // Guarded on `view` at all because this component is the FALLBACK
  // sidebar in `App.tsx`: any future view that falls through to it would
  // otherwise show a repo row highlighted for a page that never reads
  // `filters.repo` -- which is exactly the state PR Stats was in for a
  // release.
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
