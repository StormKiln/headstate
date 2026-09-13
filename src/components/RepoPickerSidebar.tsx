import { useWorktrees } from "@/api/hooks";
import { useActiveFilters, useFilters } from "@/store/filters";
import { PartialScanNotice } from "./PartialScanNotice";
import { ViewSwitcher } from "./ViewSwitcher";

/// A plain repository list, for views whose only axis is "which repo".
///
/// Separate from `WorktreeSidebar`, which decorates its rows with
/// worktree counts and an Orphaned section, and from
/// `ArtifactSidebar`, which groups by artifact kind. Those carry
/// information this does not have, and bending either into a shared
/// component would mean passing empty decorations through it.
export function RepoPickerSidebar({ reviewingCount }: { reviewingCount: number }) {
  const filters = useActiveFilters();
  const { setFilter } = useFilters();
  // The SAME repository list the worktree view uses -- one scan, one
  // source of truth for what exists in the monitored directories.
  //
  // `isError` and `refetch` since #854. #846 fixed this exact defect on
  // four surfaces, `WorktreeSidebar` among them -- which consumes THIS
  // hook -- and this second consumer of it was missed. The copy below
  // makes it the sharpest instance of the four: it is a diagnosis naming
  // the user's settings, so a failed scan sent someone to fix a
  // configuration that was never wrong.
  //
  // `unreadable` since #951, and it is what finally gives that diagnosis
  // something to lose to. #854's `isError` arm was added for a command
  // that CANNOT reject: `list_worktrees` returned a bare `Vec<Repo>` from
  // an infallible walk, so a repository whose worktree listing failed was
  // dropped from the list and the diagnosis rendered anyway. The failure
  // now travels in the payload instead of being unrepresentable.
  const {
    data: repos = [],
    isLoading,
    isError,
    refetch,
    // `= []` as `WorktreesPage` explains: a pre-#951 test double mocks
    // only what it needs, and absent reads the same as empty here.
    unreadable = [],
  } = useWorktrees();

  const rowClass = (active: boolean) =>
    `flex w-full items-center justify-between rounded px-3 py-2 text-sm ${
      active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
    }`;

  /// `aria-current` for the selected row (#852). The blue was carrying the
  /// selection alone, against the rule `StatsSidebar` states: "the
  /// selection is navigation state, and a screen reader reading a list of
  /// repository names has no other way to know which one is open."
  ///
  /// `"true"` rather than `"page"`: these rows SCOPE the current page
  /// rather than navigating to a different one. `undefined` on the
  /// inactive rows, because absence is how "not current" is spelled and
  /// `aria-current="false"` is announced by some readers.
  const current = (active: boolean) => (active ? ("true" as const) : undefined);

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={{ "to-review": reviewingCount }} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* "No repositories found in the scanned folders" is a
            DIAGNOSIS, not a holding message -- it points at the user's
            settings. Shown before the scan finishes it says the scan
            directories are wrong when they are fine, and sends someone
            to fix something that is not broken.
            
            "We have not looked yet" and "we looked and there is
            nothing" are opposite answers, which is the same rule this
            codebase applies to a failed check anywhere else.

            And so is the THIRD answer, "we looked and could not tell",
            which this had no arm for until #854 -- the sharpest omission
            of the six that issue found, because the copy above is a
            diagnosis naming the user's settings. The failure arm comes
            FIRST, for the reason `ClaudeMdPage` states: `data` keeps its
            `[]` default on a rejection, so an arm placed after the empty
            one is unreachable in exactly the case it exists for. And
            before `isLoading`, because a retry leaves both true and
            flipping back to "Looking…" reads as the error resolving
            itself. */}
        {isError ? (
          <div className="px-3 py-2">
            <p className="text-xs text-[#f85149]">Could not scan for repositories.</p>
            <button
              type="button"
              onClick={() => void refetch()}
              className="mt-1 text-xs text-[#58a6ff] hover:underline"
            >
              Try again
            </button>
          </div>
        ) : isLoading ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">Looking for repositories…</p>
        ) : repos.length === 0 && unreadable.length > 0 ? (
          /* The FOURTH answer, and the one this component existed to get
             wrong: the scan ran, found nothing, and could not read some
             of where it looked. Placed BEFORE the empty arm for the
             ordering reason `ClaudeMdPage` states -- an arm after it is
             unreachable in exactly the case it exists for -- and it is
             not merely an ordering nicety here: "no repositories" and
             "we could not read the folders" have OPPOSITE remedies, and
             the copy below names the user's settings.

             The scan is not reported as failed either, because it did
             not fail: `isError` above is a rejection of the whole
             command, and this is a walk that ran and came back short.
             Offering "Try again" would promise that a second identical
             walk might read what the first could not. */
          <p className="px-3 py-2 text-xs text-[#8b949e]">
            No repositories could be read. The paths below explain why — the
            scanned folders may well be correct.
          </p>
        ) : repos.length === 0 ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">
            No repositories found in the scanned folders.
          </p>
        ) : null}
        {/* Below whichever message above applies, and shown alongside a
            NON-empty list too: some repositories reading is not evidence
            that all of them did, and a list that is quietly short is the
            finding. */}
        {!isLoading && !isError ? (
          <PartialScanNotice
            unreadable={unreadable}
            consequence={
              repos.length === 0
                ? "no repository could be listed from them."
                : `the ${repos.length === 1 ? "repository" : `${repos.length} repositories`} below ` +
                  "may not be all of them."
            }
          />
        ) : null}
        {repos.map((r) => (
          <button
            type="button"
            key={r.path}
            onClick={() => setFilter("repo", r.path)}
            aria-current={current(filters.repo === r.path)}
            className={rowClass(filters.repo === r.path)}
          >
            <span className="truncate">{r.name}</span>
          </button>
        ))}
      </div>
    </nav>
  );
}
