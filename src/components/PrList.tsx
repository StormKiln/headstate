import type { PullRequest } from "@/types/pr";
import { PrRow } from "@/components/PrRow";
import { HelpButton } from "@/components/HelpButton";
import { useFilters } from "@/store/filters";
import { prKey } from "@/lib/prIdentity";
import { deriveStacked } from "@/lib/derive";

/// Renders PRs in whatever order it is given -- sorting is the caller's
/// responsibility (see `sortPrs` in `@/lib/derive`), so this component has
/// no opinion about ordering and doesn't drift from what the caller chose.
/// `hasFilters` distinguishes the two empty cases the old single string
/// conflated. With filters active, "nothing matches" is true and useful.
/// With NO filters active it was false and alarming: it read as a bug to a
/// user whose account genuinely has no open PRs, and told a reviewer-heavy
/// new user nothing about why their list is empty -- the app only ever
/// queries PRs they authored, and no rendered string said so.
export function PrList({
  prs,
  hasFilters = false,
  total,
  fetched,
  onOpen,
  canWrite = true,
  selectable = false,
  unreachable = false,
}: {
  prs: PullRequest[];
  hasFilters?: boolean;
  /// GitHub's true open-PR count when the poll could not fetch them all,
  /// else undefined (or 0, once a later poll came back complete). Shown
  /// so a truncated list never passes for a complete one.
  total?: number;
  /// How many pull requests the poll actually FETCHED, before this
  /// component's filters narrowed them.
  ///
  /// `prs` is the visible list, so it is the filtered count -- and the
  /// marker read "showing 3 of 29" when 29 arrived and a filter hid 26
  /// of them, inventing a truncation that never happened. `total` comes
  /// from GitHub and knows nothing about filters, so the number it is
  /// compared against must not know about them either (#745).
  ///
  /// Defaults to `prs.length` for the unfiltered call sites and tests
  /// where the two are the same.
  fetched?: number;
  /// Called with the clicked PR. Omitted where rows are not clickable.
  onOpen?: (pr: PullRequest) => void;
  canWrite?: boolean;
  selectable?: boolean;
  /// The last poll failed and there is nothing cached to fall back on.
  ///
  /// On a first launch there is no snapshot, so a failed poll leaves
  /// this list genuinely empty -- and "No open pull requests" is then a
  /// confident answer to a question the app could not ask. Same rule
  /// the rejected-query and truncated-list cases already follow.
  unreachable?: boolean;
}) {
  const { checked, setChecked, cursor, selectedPr } = useFilters();

  // What the poll got, not what survived the filters.
  const shown = fetched ?? prs.length;

  // Select-all acts on what is ON SCREEN, not the unfiltered list.
  // Selecting rows the user cannot see and then bulk-closing them is the
  // failure this avoids -- BulkBar deliberately reads the unfiltered
  // list so narrowing a filter cannot shrink a batch, which makes it all
  // the more important that the batch only ever grows from visible rows.
  const visibleKeys = prs.map(prKey);
  const allSelected = visibleKeys.length > 0 && visibleKeys.every((k) => checked.includes(k));
  const someSelected = !allSelected && visibleKeys.some((k) => checked.includes(k));

  // Range selection lives HERE, not in the row: only the list knows the
  // order rows are rendered in, and a range is defined by that order.
  // ADDS to the selection rather than replacing it, so shift-clicking a
  // second range extends rather than discards the first.
  const selectRange = (from: string, to: string) => {
    const a = visibleKeys.indexOf(from);
    const b = visibleKeys.indexOf(to);
    if (a === -1 || b === -1) return;
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    setChecked([...new Set([...checked, ...visibleKeys.slice(lo, hi + 1)])]);
  };

  // Resolved HERE for the same reason `selectRange` is: a stack is a
  // relationship between two rows, and only the list can see both of
  // them (#743). Computed once per render rather than per row -- the
  // per-row form is a scan of the whole list inside a map over it.
  //
  // Deliberately over `prs`, the FILTERED list, not the unfiltered one.
  // A marker reading "on #12" has to mean the reader can scroll to #12,
  // and resolving against rows the filter is hiding would point at PRs
  // that are not on screen. The cost is that filtering a parent out
  // silently unmarks its child; the alternative is a marker that lies
  // about where to look, which is worse for a signal whose only job is
  // to be trusted.
  const stacked = deriveStacked(prs);

  const toggleAll = () => {
    if (allSelected) {
      // Clear only the visible ones, leaving any off-screen selection
      // the user made before filtering.
      setChecked(checked.filter((k) => !visibleKeys.includes(k)));
    } else {
      setChecked([...new Set([...checked, ...visibleKeys])]);
    }
  };

  return (
    <div className="rounded-md border border-[#30363d]">
      <div className="flex items-center justify-between border-b border-[#30363d] bg-[#161b22] px-4 py-3 text-sm">
        <span className="flex items-center gap-3 font-semibold text-[#e6edf3]">
          {selectable ? (
            <label className="flex items-center">
              <span className="sr-only">Select all</span>
              <input
                type="checkbox"
                checked={allSelected}
                // A partial selection is neither checked nor unchecked,
                // and only the DOM property can say so.
                ref={(el) => {
                  if (el) el.indeterminate = someSelected;
                }}
                onChange={toggleAll}
                className="h-4 w-4 cursor-pointer accent-[#1f6feb]"
              />
            </label>
          ) : null}
          {prs.length} Open
          {/* One icon for the row vocabulary, at the head of the list
              rather than on every row -- a `?` per row is the noise
              that teaches people to ignore all of them. */}
          <HelpButton topic="pending-reviewers" />
        </span>
        {shown < (total ?? 0) ? (
          // Amber, on the list header rather than above the list, for
          // the same reason `StaleRibbon` sits on the data it doubts: a
          // notice one scroll away from the rows it qualifies is a
          // notice that gets scrolled past. The rows here are real; what
          // is wrong is the list's implicit claim to be all of them.
          //
          // Says the CAUSE, not a page size. Truncation now means pages
          // of the search failed -- the poll fans out to fetch every
          // page (#744's slowness is the same fault) -- so "GitHub
          // returns at most 100" named a limit that is no longer the
          // reason and pointed the user at nothing they could act on. A
          // refresh genuinely is the fix.
          <span role="status" className="text-xs text-[#d29922]">
            Showing {shown} of {total} — the rest did not load. Refresh to try again.
          </span>
        ) : null}
      </div>
      {prs.length === 0 ? (
        <div className="px-4 py-12 text-center text-sm text-[#8b949e]">
          {hasFilters ? (
            "No pull requests match these filters."
          ) : unreachable ? (
            <>
              <p className="text-[#e6edf3]">Could not reach GitHub.</p>
              <p className="mt-1">
                This list is empty because the last refresh failed, so the app
                does not know what is open.
              </p>
            </>
          ) : (
            <>
              <p className="text-[#e6edf3]">No open pull requests.</p>
              <p className="mt-1">
                Headstate tracks pull requests you opened, across every repository you
                can access.
              </p>
            </>
          )}
        </div>
      ) : (
        prs.map((pr, i) => (
          <PrRow
            key={prKey(pr)}
            pr={pr}
            onOpen={onOpen ? () => onOpen(pr) : undefined}
            canWrite={canWrite}
            selectable={selectable}
            onRange={selectRange}
            cursored={cursor === i}
            // Which row the detail panel is showing (#977). Read here
            // rather than in the row for the same reason `cursored` is:
            // the store's selection is a property of the LIST's state,
            // not of any one pull request.
            opened={
              selectedPr !== null && prKey(selectedPr) === prKey(pr)
            }
            stackedOn={stacked.get(pr.id)}
          />
        ))
      )}
    </div>
  );
}
