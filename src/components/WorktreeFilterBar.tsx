import { useFilters } from "@/store/filters";
import type { Filters } from "@/lib/derive";
import type { Worktree } from "@/types/pr";

/// The safety verdicts offered as facets, in the order they appear.
///
/// A fixed list rather than whatever the current rows happen to carry.
/// Deriving it from the data would make a facet vanish the moment it
/// selects nothing -- so the one click that would show you there are no
/// dirty worktrees is the click that is not there, and the bar's shape
/// would change as the list changes underneath it.
///
/// `main_checkout` is absent on purpose: it is the repository, never a
/// removal candidate, and `matchesWorktreeFilters` exempts it from
/// every filter anyway. A facet that cannot narrow anything is noise.
const FACETS: { kind: string; label: string }[] = [
  { kind: "safe", label: "Safe" },
  { kind: "merged_upstream_deleted", label: "Merged" },
  { kind: "detached_merged", label: "Detached" },
  // "Uncommitted", not "Dirty": the ROW says "2 uncommitted files", and
  // a facet whose word appears nowhere in the rows it selects is how a
  // user concludes the filter is broken. `WorktreeFilterBar.test.tsx`
  // pins this against `safetyReason`, which is how the mismatch was
  // found.
  { kind: "dirty", label: "Uncommitted" },
  { kind: "unpushed", label: "Unpushed" },
  { kind: "never_pushed", label: "Never pushed" },
  { kind: "unmerged", label: "Not merged" },
  { kind: "locked", label: "Locked" },
  { kind: "prunable", label: "Stale" },
  { kind: "orphaned", label: "Orphaned" },
];

/// Narrowing the worktree list (#1140).
///
/// A sibling of `FilterBar` rather than a prop on it: that one takes
/// `PullRequest[]`, derives its label facet from them, and its search
/// box says "Search pull requests". The two share a store and nothing
/// else.
///
/// Every control writes `undefined` rather than a falsy value when
/// cleared, the convention `inMergeQueueOnly` already follows -- so an
/// unticked box leaves no key behind for `activeFilterCount` to count
/// or `partialize` to persist.
export function WorktreeFilterBar({
  /// The UNFILTERED rows, for the per-facet counts.
  ///
  /// Counts come from the whole list, not the narrowed one: a facet
  /// showing "Dirty (0)" because Dirty is already selected would be
  /// telling the user their own selection back.
  rows,
  /// How many rows a Claude session is working in, for that facet's count.
  occupiedCount,
}: {
  rows: Worktree[];
  occupiedCount: number;
}) {
  const filters = useFilters((s) => s.filtersByView[s.view]) as Filters;
  const setFilter = useFilters((s) => s.setFilter);

  const chosen = filters.safety ?? [];
  const toggleFacet = (kind: string) => {
    const next = chosen.includes(kind)
      ? chosen.filter((k) => k !== kind)
      : [...chosen, kind];
    // `undefined` for empty, not `[]`: both mean "everything" to the
    // predicate, and leaving an empty array behind would persist a key
    // that reads as a filter to anything counting them.
    setFilter("safety", next.length > 0 ? next : undefined);
  };

  const counts = new Map<string, number>();
  for (const w of rows) {
    if (w.is_main) continue;
    counts.set(w.safety.kind, (counts.get(w.safety.kind) ?? 0) + 1);
  }

  const active =
    chosen.length > 0 || Boolean(filters.worktreeQuery) || Boolean(filters.occupiedOnly);

  return (
    <div className="flex flex-wrap items-center gap-2 px-4 py-2">
      <input
        type="search"
        value={filters.worktreeQuery ?? ""}
        onChange={(e) => setFilter("worktreeQuery", e.target.value || undefined)}
        placeholder="Filter by path or branch"
        aria-label="Filter worktrees"
        className="w-56 rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-xs text-[#e6edf3]"
      />

      {/* Only facets that MATCH something, so the bar does not offer ten
          buttons on a repository with two states -- but a facet already
          chosen stays visible even at zero, or unticking it would be
          impossible once it emptied the list. */}
      {FACETS.filter((f) => (counts.get(f.kind) ?? 0) > 0 || chosen.includes(f.kind)).map((f) => {
        const on = chosen.includes(f.kind);
        return (
          <button
            key={f.kind}
            type="button"
            aria-pressed={on}
            onClick={() => toggleFacet(f.kind)}
            className={`rounded border px-2 py-0.5 text-xs ${
              on
                ? "border-[#1f6feb] bg-[#1f6feb]/10 text-[#58a6ff]"
                : "border-[#30363d] text-[#8b949e] hover:bg-[#21262d]"
            }`}
          >
            {f.label} ({counts.get(f.kind) ?? 0})
          </button>
        );
      })}

      {/* The one facet that is not a verdict. "Who is using this" is a
          different question from "can I remove it", and it is the one
          that stops a user removing a tree out from under a running
          agent. Hidden when nothing is occupied: a permanent zero is
          noise, and there is nothing to narrow to. */}
      {occupiedCount > 0 || filters.occupiedOnly ? (
        <button
          type="button"
          aria-pressed={Boolean(filters.occupiedOnly)}
          onClick={() => setFilter("occupiedOnly", filters.occupiedOnly ? undefined : true)}
          className={`rounded border px-2 py-0.5 text-xs ${
            filters.occupiedOnly
              ? "border-[#8957e5] bg-[#8957e5]/10 text-[#a371f7]"
              : "border-[#30363d] text-[#8b949e] hover:bg-[#21262d]"
          }`}
        >
          In use ({occupiedCount})
        </button>
      ) : null}

      {active ? (
        <button
          type="button"
          onClick={() => {
            setFilter("safety", undefined);
            setFilter("worktreeQuery", undefined);
            setFilter("occupiedOnly", undefined);
          }}
          className="text-xs text-[#8b949e] underline hover:no-underline"
        >
          Clear filters
        </button>
      ) : null}
    </div>
  );
}

/// Exported for the facet list's own test, which asserts every kind
/// here is one `Safety` actually carries -- a facet for a verdict that
/// does not exist is a button that can only ever empty the list.
export const WORKTREE_FACETS = FACETS;
