import { useState } from "react";
import { useWorktreeDirs, useWorktrees } from "@/api/hooks";
import { current } from "@/lib/ariaCurrent";
import { useActiveFilters, useFilters } from "@/store/filters";
import { NarrowQueryError } from "./QueryError";
import { PartialScanNotice } from "./PartialScanNotice";
import { SettingsDialog } from "./SettingsDialog";
import { ViewSwitcher } from "./ViewSwitcher";

/// A plain repository list, for views whose only axis is "which repo".
///
/// Separate from `WorktreeSidebar`, which decorates its rows with
/// worktree counts and an Orphaned section, and from
/// `ArtifactSidebar`, which groups by artifact kind. Those carry
/// information this does not have, and bending either into a shared
/// component would mean passing empty decorations through it.
///
/// # `allLabel`, and why it is opt-in (#1043)
///
/// Three views share this list -- Packages, CLAUDE.md and Repositories --
/// and only one of them has a landing state worth a row. The Repositories
/// view renders the All Repositories overview when no repository is
/// chosen, so that state is a DESTINATION and needs an entry to click;
/// Packages and CLAUDE.md render a one-line prompt there ("Choose a
/// repository to see what is out of date"), which is a prompt rather than
/// a page, and giving it a permanent selected row would announce an
/// empty view as somewhere you are.
///
/// So the row is passed in by the view that has something behind it,
/// rather than being unconditional here or being a fourth copy of this
/// component. `WorktreeSidebar` and `RepoSidebar` both carry their own
/// "All repositories" row already; this is the same affordance in the
/// same position, and the label is a prop only because the overview it
/// leads to is named "All Repositories" on screen and the two must match.
export function RepoPickerSidebar({
  reviewingCount,
  allLabel,
}: {
  reviewingCount: number;
  /// The label for a pinned row above the repository list that clears
  /// the selection, or `undefined` for no such row. See above.
  allLabel?: string;
}) {
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
  // The scan's INPUT, not its output (#952). Everything above says what
  // the walk found; only this says whether the walk had anywhere to go.
  //
  // `default_worktree_dirs` returns an EMPTY vector when `~/code` is
  // absent, and its comment is the reason -- "Returning a path that does
  // not exist would make the worktrees view report 'no repos found' for
  // a directory the user never chose". That is correct and stays; what
  // was missing is a component that can tell the two apart. Costs one
  // cache hit: `useWorktreeDirs` is `staleTime: Infinity` and Settings
  // already holds the same query.
  const { dirs } = useWorktreeDirs();
  const [settingsOpen, setSettingsOpen] = useState(false);

  const rowClass = (active: boolean) =>
    `flex w-full items-center justify-between rounded px-3 py-2 text-sm ${
      active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
    }`;

  // `current` from `@/lib/ariaCurrent` (#852, shared in #977). The blue was
  // carrying the selection alone, against the rule `StatsSidebar` states:
  // "the selection is navigation state, and a screen reader reading a list
  // of repository names has no other way to know which one is open."

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
          <NarrowQueryError
            message="Could not scan for repositories."
            onRetry={() => void refetch()}
          />
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
        ) : repos.length === 0 && dirs.length === 0 ? (
          /* The FIFTH answer, and the first one a new user actually gets
             (#952): there is nowhere to look. Distinct from the empty arm
             below in exactly the way the arms above are distinct from each
             other -- "we looked and there is nothing" and "we had nowhere
             to look" are opposite answers, and only one of them is a task.

             AFTER the unreadable arm, not before: a scan that could not
             read its folders had folders, so `dirs` is non-empty there and
             the order is belt and braces rather than load-bearing. But it
             must come after `isError` and `isLoading` for the reason the
             block above gives at length -- `dirs` also defaults to `[]`
             while its own query is in flight, so placed first this would
             claim "no directories configured" during the very first
             render of a machine that has three.

             The button is the remedy the other two arms lack, and it is
             reachable now: #945 fixed the field that rejected `~/code`,
             so someone sent to Settings can actually type the path they
             have. `initialSection` is `ConnectionBanner`'s pattern --
             deep-linking to the section that is the only reason the
             control was pressed. */
          <div className="px-3 py-2">
            <p className="text-xs text-[#8b949e]">
              Headstate does not know where your repositories are yet. It has
              no folders to scan.
            </p>
            <button
              type="button"
              onClick={() => setSettingsOpen(true)}
              className="mt-2 rounded border border-[#30363d] px-2 py-1 text-xs text-[#58a6ff] hover:bg-[#161b22]"
            >
              Choose folders to scan…
            </button>
          </div>
        ) : repos.length === 0 ? (
          /* Directories ARE configured and the walk came back empty, so
             this stays a diagnosis -- and now NAMES them (#952). The
             paths are the difference between something the user can
             check and something they have to guess at: "no git
             repositories under /Users/x/src" is either obviously right
             or obviously the wrong folder, and the old sentence was
             neither. */
          <div className="px-3 py-2">
            <p className="text-xs text-[#8b949e]">
              No git repositories found in the {dirs.length === 1 ? "folder" : "folders"}{" "}
              being scanned.
            </p>
            <ul className="mt-1 space-y-0.5">
              {dirs.map((d) => (
                <li key={d} className="break-all font-mono text-[11px] text-[#8b949e]">
                  {d}
                </li>
              ))}
            </ul>
            <button
              type="button"
              onClick={() => setSettingsOpen(true)}
              className="mt-2 rounded border border-[#30363d] px-2 py-1 text-xs text-[#58a6ff] hover:bg-[#161b22]"
            >
              Change the folders scanned…
            </button>
          </div>
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
        {/* The landing state, as a DESTINATION (#1043).

            Pinned above the list and outside it, the position
            `WorktreeSidebar` and `RepoSidebar` both put theirs in. Before
            this, the Repositories view rendered its overview in the
            `!repo` branch and the sidebar listed only repositories, so the
            overview was a state with no entry to click -- and once a
            repository was picked there was no way back to it except
            deselecting, which nothing on screen offered.

            It is rendered even while the scan is loading, failing or
            empty, which is the opposite of the arms above: those describe
            what the scan FOUND, and this does not depend on the scan at
            all. A view whose landing page is unreachable during a failed
            scan is the failure mode this row exists to remove.

            `current()` from `@/lib/ariaCurrent`, never a bare boolean. A
            boolean serialises `false` to the literal string "false",
            which some screen readers announce as current -- so every
            inactive row would claim to be the one you are on. That exact
            defect shipped in #1037 and was fixed in #1039; this is the
            helper that exists so it cannot come back.

            `setFilter("repo", undefined)` clears `repoPath` and
            `repoFile` as well, and does so INSIDE the store rather than
            here: `filters.ts` resets them for the `repo` key precisely so
            that no caller has an ordering to get wrong. Clicking this
            while three directories into a file tree returns to the
            overview, not to a stale position in a repository that is no
            longer selected. */}
        {allLabel !== undefined ? (
          <button
            type="button"
            onClick={() => setFilter("repo", undefined)}
            aria-current={current(!filters.repo)}
            className={rowClass(!filters.repo)}
          >
            <span className="truncate">{allLabel}</span>
          </button>
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
      {/* Mounted only while open, `ConnectionBanner`'s shape: the dialog
          subscribes to every settings query, and a permanently mounted
          copy behind every repository sidebar would run them on views
          that never open it. */}
      {settingsOpen ? (
        <SettingsDialog
          open
          onOpenChange={setSettingsOpen}
          initialSection="repositories"
        />
      ) : null}
    </nav>
  );
}
