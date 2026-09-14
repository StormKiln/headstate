import { useWorktreeDirs, useWorktrees } from "../api/hooks";
import { type View, useActiveFilters, useFilters } from "../store/filters";
import { current } from "../lib/ariaCurrent";
import { isOrphaned, ORPHAN_FILTER } from "../lib/worktrees";
import { NarrowQueryError } from "./QueryError";
import { PartialScanNotice } from "./PartialScanNotice";
import { ViewSwitcher } from "./ViewSwitcher";

/// Repos that have worktrees.
///
/// A separate component from `RepoSidebar` rather than a mode of it: that
/// one is driven by a `PullRequest[]` and counts open PRs, which has no
/// meaning here. Sharing the shell and the switcher is enough.
export function WorktreeSidebar({
  viewCounts,
}: {
  viewCounts?: Partial<Record<View, number>>;
}) {
  // `isLoading` and `isError`, not just the data (#846).
  //
  // This column had ZERO loading or error handling. On a failed
  // `useWorktrees`, `repos` is `undefined` -- so both guards at the
  // bottom of the list fail (`repos !== undefined && …` and
  // `repos?.length === 0`), `allCount` reduces over `[]`, and the column
  // rendered exactly one thing: "All repositories  0". A failure as a
  // confident zero, in the one place the user looks to ask where the disk
  // went. `RepoPickerSidebar` -- the plain list this one is a decorated
  // version of -- has always distinguished the two, and states why:
  // "'We have not looked yet' and 'we looked and there is nothing' are
  // opposite answers".
  //
  // `unreadable` since #951: the scan can come back SHORT without
  // rejecting, and every count in this column is then a floor. See the
  // orphan section below for the one number where that matters most.
  const {
    data: repos,
    isLoading,
    isError,
    refetch,
    // `= []` for the same reason `WorktreesPage` states: a test double
    // predating #951 mocks only the fields it cares about, and absent
    // means the same as empty -- no shortfall reported.
    unreadable = [],
  } = useWorktrees();
  const partial = unreadable.length > 0;
  // The scan's INPUT (#952), so the empty arm below can tell "nowhere to
  // look" from "nothing there". One cache hit: `staleTime: Infinity`.
  const { dirs } = useWorktreeDirs();
  const filters = useActiveFilters();
  const { setFilter } = useFilters();

  const rowClass = (active: boolean) =>
    `flex w-full items-center justify-between rounded px-3 py-2 text-sm ${
      active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
    }`;

  // `current` from `@/lib/ariaCurrent` (#852, shared in #977). It covers the
  // Orphaned row too, which is the one that most needs it: its only other
  // distinguishing mark is its amber text, so a reader who cannot see colour
  // had nothing at all.

  // Worktree count EXCLUDING the main checkout: it is not a worktree you
  // would ever remove, and counting it inflates every repo by one.
  // Counted by EXCLUDING the main checkout, not by subtracting one.
  //
  // `n - 1` assumed every repository has a main checkout. An ORPHANED
  // worktree has exactly one entry and no main, so `n - 1` made it zero
  // -- which both undercounted the total and hid the row entirely.
  // MEASURED on a real machine: sidebar 121 against a rollup of 124,
  // with exactly 3 repositories having no main checkout.
  //
  // The rollup already counts this way (`rollup.ts` skips `is_main`),
  // so this makes one number out of what were two methods for the same
  // thing.
  const removable = (r: { worktrees: { is_main: boolean }[] }) =>
    r.worktrees.filter((w) => !w.is_main).length;
  // Same rule as the per-repo counts: the main checkout is not a
  // worktree anyone would remove, so it is not counted as one.
  const allCount = (repos ?? []).reduce((n, r) => n + removable(r), 0);
  // Repositories with nothing to act on are hidden. Most repositories
  // have no worktrees at any given moment, so listing them all made the
  // sidebar mostly rows that cannot be clicked usefully.
  //
  // The TOTAL is deliberately not filtered: it is already built from
  // `removable`, so hiding rows cannot change it -- and a total that
  // moved when rows were hidden would be a different number pretending
  // to be the same one.
  // Orphans are excluded here and given their own section below: an
  // orphan record IS a single worktree with no main checkout, so
  // without this it would appear both as a repository row and in the
  // Orphaned section -- counted once but listed twice.
  const withWorktrees = (repos ?? []).filter(
    (r) => removable(r) > 0 && !r.worktrees.some((w) => isOrphaned(w.safety)),
  );

  // Orphans get their own section rather than sitting among the repos.
  //
  // They are not a repository -- the repository is exactly what is
  // gone -- so listing them alongside real ones invites reading them as
  // ordinary. Grouped, they answer a question the per-repo rows cannot:
  // "what is on disk that nothing owns any more".
  const orphans = (repos ?? []).filter((r) =>
    r.worktrees.some((w) => isOrphaned(w.safety)),
  );
  const orphanCount = orphans.reduce(
    (n, r) => n + r.worktrees.filter((w) => isOrphaned(w.safety)).length,
    0,
  );

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* The scan's state, ABOVE the rows, and a failure is not a zero
            (#846).

            `RepoPickerSidebar`'s wording and reasoning, because this is
            the same scan answering the same question and two phrasings of
            it would be two claims. The retry is here rather than only on
            the page body because this column is what a user stares at
            when the page looks empty -- and with no error and no retry,
            the failure was not merely unreported but reassuring.

            It took the wording and not the ORDERING, which was the bug
            (#974): `isLoading` was checked first, so hitting "Try again"
            after a failed scan flipped this column to "Looking for
            repositories…" and the error read as having resolved itself. A
            retry leaves both flags true for a moment. Both sibling
            consumers of this same hook name that ordering as the thing to
            avoid, and this was the third. The failure arm now comes
            first, as it does there. */}
        {isError ? (
          <NarrowQueryError
            message="Could not scan for worktrees."
            onRetry={() => void refetch()}
          />
        ) : isLoading ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">Looking for repositories…</p>
        ) : null}
        {/* And the third answer, above the counts it qualifies (#951).
            Every number in this column -- the all-repositories total, the
            per-repo counts, and the orphan count most of all -- is taken
            over whatever the walk managed to read, so the reason it read
            less has to sit where those numbers are read. */}
        {!isLoading && !isError ? (
          <PartialScanNotice
            unreadable={unreadable}
            consequence="the counts below are floors rather than totals."
          />
        ) : null}
        {/* Missing entirely before: with no "all" entry, clearing the
            repo filter fell through to `repos?.[0]` and silently showed
            the FIRST repo -- so across 37 repos there was no way to ask
            where the disk went. RepoSidebar has had a real all-repos row
            all along. */}
        <button
          type="button"
          onClick={() => setFilter("repo", undefined)}
          aria-current={current(!filters.repo)}
          className={rowClass(!filters.repo)}
        >
          <span>All repositories</span>
          {/* An em dash until the scan has ANSWERED (#846). `allCount`
              reduces over `[]` whether the scan has not run, is running,
              or rejected -- so the row printed a confident "0" in all
              three cases, and "All repositories  0" was the entire
              content of this column on a failure.

              The same rule the size cells on the worktree page follow: an
              unknown renders as a dash, never as a number, because zero
              is an ANSWER and this has none. */}
          <span className="ml-2 shrink-0">
            {repos === undefined ? "—" : partial ? `≥ ${allCount}` : allCount}
          </span>
        </button>
        {withWorktrees.map((r) => (
          <button
            type="button"
            key={r.path}
            onClick={() => setFilter("repo", r.path)}
            aria-current={current(filters.repo === r.path)}
            className={rowClass(filters.repo === r.path)}
          >
            <span className="truncate">{r.name}</span>
            <span className="ml-2 shrink-0">{removable(r)}</span>
          </button>
        ))}
        {/* Distinct from "no repositories found": the scan worked and
            found repositories, they simply have no worktrees. Rendering
            a blank list instead would read as a failed scan, which is a
            different and more alarming thing. */}
        {repos !== undefined && repos.length > 0 && withWorktrees.length === 0 ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">
            No worktrees in any scanned repository.
          </p>
        ) : null}
        {/* Below the repositories, with a rule: it is a different
            KIND of thing, not another repo. Absent entirely when there
            are none, since a permanent empty heading trains the eye to
            skip it. */}
        {orphanCount > 0 ? (
          <div className="mt-2 border-t border-[#30363d] pt-2">
            <button
              type="button"
              onClick={() => setFilter("repo", ORPHAN_FILTER)}
              aria-current={current(filters.repo === ORPHAN_FILTER)}
              className={rowClass(filters.repo === ORPHAN_FILTER)}
            >
              <span className="truncate text-[#d29922]">Orphaned</span>
              {/* "at least N" on a partial scan (#951), and the reasoning
                  is the same as the size total's on `WorktreesPage`: a
                  count that silently treats what it could not read as
                  zero is a confident wrong answer.

                  QUALIFIED rather than suppressed, which is the decision
                  that matters here and is argued in full at the orphan
                  section of `WorktreesPage`. Short version: each orphan
                  is a POSITIVE finding, so a short walk cannot invent
                  one -- only miss one. The direction of the error is
                  "too few", and the honest word for that is "at least". */}
              <span className="ml-2 shrink-0">
                {partial ? `≥ ${orphanCount}` : orphanCount}
              </span>
            </button>
          </div>
        ) : null}
        {/* `repos?.length === 0`, and the optional chain is load-bearing
            in the OTHER direction now (#846): it is false on `undefined`,
            so this DIAGNOSIS -- "check the scanned directories" -- is
            correctly silent while the scan has not answered. That is the
            `RepoPickerSidebar` rule ("sends someone to fix something that
            is not broken"), and it was already satisfied here.

            What was missing is the positive half: a silent column said
            nothing at all on a failure. The arm at the top of this list
            is that half, and this stays exactly as it was.

            #952 splits it in two. "Check the scanned directories" asks
            the user to check a list that, on a machine without `~/code`,
            is EMPTY -- so the instruction sends them to verify nothing.
            The same distinction `RepoPickerSidebar` draws: having nowhere
            to look is a task, and finding nothing in three real folders
            is a finding. */}
        {repos?.length === 0 ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">
            {dirs.length === 0
              ? "Headstate does not know where your repositories are yet. Set the directories to scan in Settings."
              : "No repositories found. Check the scanned directories in Settings."}
          </p>
        ) : null}
      </div>

      {/* The pinned Stats row is gone (#794), for the same reason as in
          `DockerSidebar`. It had to set `view` AND `panel` together
          because Stats was a sub-page of a different view; PR Stats is a
          view, so `ViewSwitcher` above reaches it directly and the
          sidebar it renders beside is its own `RepoSidebar` rather than
          this list of worktree repos. */}
    </nav>
  );
}
