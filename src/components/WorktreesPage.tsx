import { ActingOnDesktop } from "./ActingOnDesktop";
import { summarisePull } from "@/lib/pullSummary";
import { ExternalLink } from "./ExternalLink";
import { Sparkles } from "lucide-react";
import { useMemo, useState } from "react";
import {
  useClaudeSessions,
  useUiPrefs,
  useClearAssessed,
  useMarkAssessed,
  useRemoveWorktree,
  useAssessed,
  usePullRequests,
  useRemoveWorktreeForced,
  useRemoveWorktrees,
  useWorktreeSafety,
  useWorktreeSizes,
  useWorktrees,
  usePullCheckout,
  useFetchRefs,
  useRemoveOrphan,
  useOrphanSize,
  useAllWorktreeSizes,
  useRemovalProgress,
  useAssessment,
  useUnlockWorktree,
  useUnlockWorktrees,
  usePruneWorktrees,
  useWorktreeDirs,
} from "../api/hooks";
import {
  worktreeSessions,
  occupancy,
  formatSize,
  totalSize,
  canClaudify,
  submoduleNote,
  isPending,
  isSafe,
  pathBasename,
  prForWorktree,
  worktreeSignal,
  isOrphaned,
  ORPHAN_FILTER,
  forceWarning,
  safetyReason,
  safetyTone,
  upstreamReasonAged,
  upstreamShort,
  upstreamTone,
  upstreamToneAged,
  refAge,
  lockAge,
  lockHolderNote,
  isDeadLock,
  sortWorktrees,
  WORKTREE_SORT_LABELS,
  type WorktreeSort,
} from "../lib/worktrees";
import { HelpButton } from "./HelpButton";
import { WorktreeKebab } from "./WorktreeKebab";
import { claudifyCommand } from "../api/tauri";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { isCancelled } from "@/lib/cancelled";
import { copyText } from "../lib/clipboard";
import { relativeTime } from "../lib/time";
import { useIsMobile } from "../lib/useIsMobile";
import { assessmentSummary } from "../lib/assessment";
import { rollupRepos } from "../lib/rollup";
import { useActiveFilters, useFilters } from "../store/filters";
import type {
  ClaudeSession, PullRequest, Worktree } from "../types/pr";
import { toast } from "sonner";
import { PartialScanNotice } from "./PartialScanNotice";
import { QueryError, errorMessage } from "./QueryError";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// Local git worktrees, so lingering ones can be found and removed.
///
/// Sorted largest-first by default -- with 152 worktrees on one
/// repository, the biggest offenders are what you came for -- and by
/// name, size or age on request (#771). But SAFETY is the primary axis:
/// every row says whether it can be removed and why not, because 52 of
/// 295 worktrees here hold commits that exist nowhere else.
/// A shimmering placeholder sized to the text it stands in for.
///
/// Deliberately not a spinner per row: 289 spinners is a disco, and a
/// spinner says "something is happening" where a skeleton says "a value
/// belongs here and is coming". Respects prefers-reduced-motion via the
/// motion-safe: prefix -- an animation on every row is exactly what that
/// setting exists to stop.
function Skeleton({ className = "" }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block h-3 rounded bg-[#30363d] align-middle motion-safe:animate-pulse ${className}`}
    />
  );
}

/// Why a size cell says "not measured" (#769).
///
/// One constant for both cells -- the per-repository row and the
/// all-repositories rollup -- so the explanation cannot drift between
/// two views of the same fact. It names the CAUSE rather than just the
/// outcome, because "could not measure" alone leaves the user with
/// nothing to do about it, and nesting is the cause they can act on.
const UNMEASURED_HINT =
  "This worktree could not be measured within the time limit. It is usually a very large tree, or one nested under another checkout so its files are walked twice.";

function Row({
  wt,
  repoPath,
  pr,
  onRemove,
  onClaudify,
  onForget,
  sizePending,
  sizeUnmeasurable = false,
  removing = false,
  assessed = false,
  onForce,
  onUnlock,
  onRemoveOrphan,
  onPull,
  pulling = false,
  onFetch,
  fetching = false,
  fetchedAt = null,
  scannedAt,
  agent = null,
}: {
  wt: Worktree;
  /// The live Claude session working in this worktree, if any (#1137).
  ///
  /// `null` covers both "nothing is here" and "the integration is off".
  /// The REFUSAL distinguishes those -- see `removable` on the page --
  /// because only one of them is a reason the user can act on. This prop
  /// is about the badge, and a badge for a session that is not there is
  /// the same absence either way.
  agent?: ClaudeSession | null;
  /// The open pull request for this worktree, when there is one.
  /// DISPLAY ONLY -- it never feeds a safety gate.
  pr?: PullRequest | null;
  onRemove: (wt: Worktree) => void;
  onClaudify: (wt: Worktree) => void;
  onForget: (wt: Worktree) => void;
  /// This row's removal is in flight. Per row, not per page: with 100+
  /// rows, freezing all of them because one is deleting would be worse
  /// than no feedback at all.
  removing?: boolean;
  /// This worktree has been handed to Claude Code and the branch has not
  /// moved since. Unlocks the override.
  assessed?: boolean;
  onForce: (wt: Worktree) => void;
  /// Clear this worktree's lock (#775). Removes nothing; it opens a
  /// confirmation naming the holder and the age.
  onUnlock: (wt: Worktree) => void;
  /// Delete an orphaned directory. A different call from `onRemove`:
  /// git cannot remove a worktree whose repository is gone.
  onRemoveOrphan: (wt: Worktree) => void;
  /// Sizes arrive in their own pass, after safety. Tracked separately so
  /// a row whose safety has resolved does not keep waiting on its size.
  sizePending?: boolean;
  /// The walk for THIS worktree gave up before it finished (#769).
  ///
  /// Distinct from `sizePending`, and it has to be: both would otherwise
  /// render as `size_bytes === null`, and the row would go on promising
  /// a number that is never coming. That promise outlasting 15 minutes
  /// is the bug. Distinct from a measured zero too -- "could not
  /// measure" must never read as "this tree is empty", which is an
  /// invitation to delete a checkout nobody has measured.
  sizeUnmeasurable?: boolean;
  /// The repo this worktree belongs to. Needed to assess it: git has to
  /// be run from the repo, not the worktree.
  repoPath: string;
  /// Fast-forward this checkout. Only ever called for the main one.
  onPull: (wt: Worktree) => void;
  /// This row's pull is in flight. Per row, like `removing`.
  pulling?: boolean;
  /// Refresh this repository's remote refs, moving nothing (#788). Only
  /// ever called for the main checkout, like `onPull`.
  onFetch: (wt: Worktree) => void;
  /// This row's fetch is in flight. Tracked SEPARATELY from `pulling`,
  /// not folded into one "busy" flag: the two actions are independently
  /// available -- Fetch works on a dirty tree where Update is refused --
  /// so one flag would disable a button that is not actually blocked.
  fetching?: boolean;
  /// When this worktree's REPOSITORY last fetched, RFC 3339, or null
  /// (#788).
  ///
  /// A repository-level fact on a per-worktree prop, because that is
  /// where it is needed: `fetched_at` lives on `WorktreeRepo` and the
  /// claim it qualifies -- "up to date with upstream" -- is rendered on
  /// this row. Until now it was rendered only on the page header, while
  /// the badge it qualifies is here, so a reader looking at the row saw
  /// green and never looked up. That separation IS the bug #788 reports.
  ///
  /// Defaults to null rather than being required, and null means "we do
  /// not know", never "fresh". The Orphaned view below passes no value at
  /// all and correctly cannot: an orphan's repository is the thing that is
  /// gone, so there is nothing to have fetched.
  fetchedAt?: string | null;
  /// The instant `fetchedAt` is measured against, in the caller's hands.
  ///
  /// `Date.now()` must not be called during render -- eslint enforces it,
  /// `Sparkline` in `SystemHealthPage.tsx` documents why, and with
  /// hundreds of these rows on screen a per-row clock read would also
  /// make no two rows agree about what time it is. `WorktreesPage` takes
  /// one anchor from the scan's own `dataUpdatedAt` and hands the same
  /// `Date` to every row; see its doc for why that instant is the
  /// truthful one rather than merely the pure one.
  scannedAt: Date;
}) {
  const safe = isSafe(wt.safety);
  const orphaned = isOrphaned(wt.safety);
  // `Safety` already answers "is this dirty, and by how much" -- reusing
  // it means the button's reason cannot disagree with the row's own
  // explanation of the same checkout.
  const dirtyCount = wt.safety.kind === "dirty" ? wt.safety.detail : null;
  const pending = isPending(wt.safety);
  const claudifiable = canClaudify(wt.safety);
  const signal = worktreeSignal(pr);
  // Fetched only once a row is opened: several git calls each, and
  // there can be hundreds of rows on screen.
  const [open, setOpen] = useState(false);
  const { data: assessment, isLoading: assessing } = useAssessment(
    open ? repoPath : null,
    open ? wt.path : null,
    open ? wt.branch : null,
  );
  // The cells, built once. The desktop lays them out on one line; the
  // phone stacks them -- name and size, then the verdict, then the
  // action -- so the verdict can wrap instead of truncating and the
  // action is never squeezed against the path.
  const isMobile = useIsMobile();
  /// A FLOOR under the name, so it cannot be squeezed to nothing
  /// (#818).
  ///
  /// Constraining the verdict cell is most of the fix, but `flex-1` on
  /// this cell is `flex-basis: 0%` -- it is still the first thing a
  /// flex row takes width from, and two long cells competing would
  /// leave the name a few characters wide. The identifier is the one
  /// cell that is useless partially: "agent-a1..." does not distinguish
  /// the row from the nine others beside it, which is the whole failure
  /// the issue reported. 12rem is about 20 monospace characters -- long
  /// enough for the agent-worktree names this page is full of to stay
  /// told apart, and it only binds on a genuinely narrow desktop window
  /// (the phone stacks instead).
  ///
  /// A `min-width` rather than a basis, because shrink applies AFTER
  /// the basis and would walk straight back through it -- `min-width`
  /// is the only one of the two that is a floor. It does not defeat the
  /// `truncate` beside it: truncation needs the box to be allowed
  /// narrower than its TEXT, which a floor well under the text's width
  /// still allows. The phone keeps `min-w-0`, since a stacked name has
  /// the whole row and no competitor to be floored against.
  const nameCell = (
      <span
        className={`${isMobile ? "min-w-0" : "min-w-48"} flex-1 truncate font-mono text-[#e6edf3]`}
      >
        {pathBasename(wt.path)}
        {wt.branch ? (
          <span className="ml-2 text-xs text-[#8b949e]">{wt.branch}</span>
        ) : (
          <span className="ml-2 text-xs text-[#8b949e]">detached</span>
        )}
        {/* Which agent is working here (#1137).
            The question at the top of this page on a machine running
            ~100 worktrees, and it could not be answered -- while the
            join already existed in the other direction at a measured
            83.1% match rate, imported by exactly one component.

            Named rather than a bare dot: "an agent is here" invites
            "which one", and the session's own title is what the Claude
            page shows for the same row. */}
        {agent ? (
          <span
            className="ml-2 shrink-0 rounded bg-[#1f6feb]/15 px-1.5 py-0.5 text-[10px] text-[#58a6ff]"
            title={`${agent.name ?? agent.session_id} is working here`}
          >
            agent here
          </span>
        ) : null}
      </span>
  );
  /// The verdict as ONE plain string, for the desktop cell's tooltip.
  ///
  /// The cell below renders the same facts as composite JSX -- a PR
  /// link, a coloured signal, a relative time -- and `title` takes only
  /// text, so the pieces are re-joined here rather than read back off
  /// the DOM. Built from the same expressions the cell uses, in the
  /// same order, because a tooltip that disagrees with the row it
  /// explains is worse than no tooltip: this is the ONLY place the
  /// truncated tail survives (#818), so it has to be complete.
  const safetyTitle = [
    pending ? null : safetyReason(wt.safety),
    wt.merged_at ? `merged ${wt.merged_at}` : null,
    wt.upstream && wt.is_main ? upstreamReasonAged(wt.upstream, fetchedAt, scannedAt) : null,
    wt.upstream && !wt.is_main ? upstreamShort(wt.upstream) : null,
    pr ? `#${pr.number}` : null,
    signal ? signal.label : null,
    wt.last_commit && !wt.is_main ? relativeTime(wt.last_commit) : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const safetyCell = (
      <span
        // TRUNCATES, and yields before the name does (#818).
        //
        // This cell used to be `shrink-0` with no width bound, which on
        // a flex row means "claim my full intrinsic width and give none
        // of it back". The name was the only `flex-1` cell, so it
        // absorbed all the pressure: a lock reason carrying a pid and a
        // start time squeezed the worktree's own name to nothing and
        // pushed the Remove button and kebab clean out of the bordered
        // box. You could neither tell which checkout the row was nor
        // reach its actions -- on the one page where the action deletes
        // a directory.
        //
        // So the reason now shrinks (`min-w-0`) and clips (`truncate`),
        // and the name keeps its `flex-1`. The priority is deliberate,
        // not incidental: of the three desktop cells the verdict is the
        // most expendable at narrow widths, because it is a SENTENCE --
        // its first clause carries the decision ("merged, pushed", "3
        // uncommitted files") and the tail is elaboration -- whereas the
        // name is an identifier that means nothing partially and the
        // action cell is a button that has to be clickable. A clipped
        // sentence still reads; half a name does not.
        //
        // `flex-auto` rather than `flex-1` for the basis: `flex-1` is
        // `flex: 1 1 0%`, which would give the verdict an equal share of
        // the row with the name no matter how short either one is, so a
        // two-word verdict beside a long name would sit in a half-row of
        // whitespace. `flex-auto` is `flex: 1 1 auto` -- sized from its
        // content, as it effectively was while `shrink-0`, and the
        // change is only that it can now give width back under
        // pressure. The wide row therefore looks exactly as it did
        // before this fix; only the tight one differs.
        //
        // Nothing is LOST: `title` carries the whole string. That is
        // what makes truncating honest here rather than the app hiding
        // a fact it computed -- and the mouse-free route to it is the
        // row's own disclosure, which shows the assessment in full.
        //
        // The phone is untouched. It already stacks the cells so the
        // verdict wraps instead of truncating, which is why this bug
        // was desktop-only; see the comment on the cells above.
        className={`${isMobile ? "" : "min-w-0 flex-auto truncate "}text-xs ${safetyTone(wt.safety)}`}
        // Only on the desktop, and only because that is the layout that
        // clips. Adding it on the phone would put a tooltip on text
        // already shown in full, and on a touch screen a `title` is
        // reachable by nothing.
        title={isMobile ? undefined : safetyTitle}
        // The whole row is one live region while it fills in, so a
        // screen reader hears the resolved value once rather than
        // announcing each cell as it lands.
        //
        // `aria-live` is what makes that true. `aria-busy` alone is
        // INERT on a non-live element -- it suppresses announcements
        // from a live region, so without the pairing the comment above
        // described an intent the markup never carried out.
        aria-live="polite"
        aria-busy={pending || sizePending ? true : undefined}
      >
        {pending ? <Skeleton className="w-40" /> : safetyReason(wt.safety)}
        {/* WHICH KIND of dirt, beside the count (#1138). The count is
            already right -- `git status --porcelain` reports a dirty
            submodule as a ` M <path>` line, so `Dirty` already wins and
            Remove already declines -- but "1 uncommitted file" and "a
            submodule has work in it" send the user to different
            commands, and only one of them is `git add`. */}
        {!pending && submoduleNote(wt.submodules) ? (
          <span className="text-[#d29922]"> · {submoduleNote(wt.submodules)}</span>
        ) : null}
        {wt.merged_at ? (
          <span className="text-[#8b949e]"> · merged {wt.merged_at}</span>
        ) : null}
        {/* The main checkout's row said only what it was, while every
            other row earned its space. "Behind by 40" is also what
            explains why the worktrees below it are stale. */}
        {/* The main checkout gets the long prose -- it is the only thing
            that line says. Every other row gets the compact arrow form,
            since it already carries name, branch, safety, and size. */}
        {/* The age of the refs rides WITH the verdict, in one span, and
            the verdict's colour is chosen knowing that age (#788).

            Both halves of #788's complaint are here. The age was
            previously only on the page header -- `refAge` at the top of
            this component's render, 1400 lines from this line in the
            markup -- so the reader looked at the ROW, read green, and
            never looked up. And the green meant two opposite things:
            "verified current" and "agreed with a ref nobody has checked
            for two days". `upstreamToneAged` greys the second.

            One span, not the verdict in one colour and the age appended
            in another. They are one claim: "these agree, as far as we
            last looked". Splitting the colour would say the agreement is
            solid and only its timestamp is doubtful, which is backwards
            -- it is the agreement that is in question. */}
        {wt.upstream && wt.is_main ? (
          <span className={upstreamToneAged(wt.upstream, fetchedAt, scannedAt)}>
            {" · "}
            {upstreamReasonAged(wt.upstream, fetchedAt, scannedAt)}
          </span>
        ) : null}
        {/* The compact arrow form keeps `upstreamTone`, NOT the
            age-aware one, and that is deliberate rather than an
            oversight (#788).

            `upstreamShort` returns null for `current` -- an up-to-date
            branch adds noise, not information, on a row that already
            carries name, branch, safety and size -- so there is no green
            "up to date" badge here for a stale ref to be resting on.
            The bug #788 reports cannot occur on these rows.

            What it does render is exactly the set whose colour must not
            move with the fetch age: `↑`/`↓`/`↑↓`, where staleness can
            only understate a real divergence, and `local only`, which
            does not consult `origin/*` at all. Greying any of them would
            hide a claim staleness cannot falsify. `upstreamToneAged`
            would in fact return the same string for every one of these
            kinds -- it only diverges on `current` -- so this is the same
            value said plainly rather than a second behaviour. */}
        {wt.upstream && !wt.is_main && upstreamShort(wt.upstream) ? (
          <span className={upstreamTone(wt.upstream)}>
            {" · "}
            {upstreamShort(wt.upstream)}
          </span>
        ) : null}
        {/* How stale the work is -- distinct from merged_at, which says
            whether it is already accounted for. */}
        {/* The app already holds GitHub's answer for this branch and
            never showed it here. Display only: a wrong pairing must not
            be able to authorise a deletion. */}
        {pr ? (
          <ExternalLink
            href={pr.url}
            className="text-[#58a6ff] hover:underline"
          >
            {" · "}#{pr.number}
          </ExternalLink>
        ) : null}
        {/* Why you would come back to this checkout. Display only, like
            the number itself: a wrong pairing must never authorise a
            deletion. Costs no new call -- every field read here is
            already on the pull request resolved above. */}
        {signal ? (
          <span className={signal.className}> · {signal.label}</span>
        ) : null}
        {wt.last_commit && !wt.is_main ? (
          <span className="text-[#8b949e]"> · {relativeTime(wt.last_commit)}</span>
        ) : null}
      </span>
  );
  const sizeCell = (
      <span className="w-20 shrink-0 text-right tabular-nums text-xs text-[#8b949e]">
        {/* Three states, not two. An em dash read as "measured, and the
            answer is nothing"; a skeleton says a number is still coming.
            #769 needed a third: the walk ran out of budget, so no number
            is coming and the row must stop implying one. Checked FIRST,
            because the query can still be fetching other worktrees while
            this one has already given up -- ordering it after
            `sizePending` would leave the skeleton up for the rest of the
            pass. */}
        {sizeUnmeasurable ? (
          <span className="cursor-help text-[#6e7681]" title={UNMEASURED_HINT}>
            not measured
          </span>
        ) : sizePending && wt.size_bytes === null ? (
          <Skeleton className="w-12" />
        ) : (
          formatSize(wt.size_bytes)
        )}
      </span>
  );
  const actionCells = (
    <>
      {/* One action per row, never two: the row is already dense. Safe
          rows get Remove; the 124 that cannot be removed get Claudify,
          which answers the question that actually applies to them --
          "is there anything in here worth keeping?" -- rather than
          showing a dead button that says the app will not help. */}
      {/* A FIXED-WIDTH action cell. The button in it changes label --
          "Claudify" becomes the wider "Remove anyway…" once assessed --
          and without a reserved width that swap re-flowed every column
          in the table. A row's layout should not depend on which action
          it currently offers. */}
      {/* Fixed width still, but wider: every row now holds a button AND
          a kebab, and the point of the fixed cell is that a row's
          layout never depends on which action it offers. */}
      <span
        className={
          isMobile
            ? "flex shrink-0 items-center gap-1"
            : "flex w-40 shrink-0 items-center justify-end gap-1"
        }
      >
      {claudifiable && assessed ? (
        // Only after an assessment of THIS worktree. Otherwise this is a
        // "delete anything" button with extra steps.
        //
        // The kebab beside it is the way BACK. Marking an assessment
        // used to be a one-way door: it replaced Claudify, persisted
        // across restarts, and cleared only when the branch moved -- so
        // one exploratory click permanently removed the only route to
        // that worktree's prompt. Needing the prompt again is normal.
        <button
          type="button"
          onClick={() => onForce(wt)}
          title="You assessed this worktree — remove it despite the safety gate"
          className="shrink-0 rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10"
        >
          Remove anyway…
        </button>
      ) : claudifiable ? (
        <button
          type="button"
          onClick={() => onClaudify(wt)}
          title={`Copy a prompt asking Claude Code to assess this worktree (${safetyReason(
            wt.safety,
          )})`}
          className="flex shrink-0 items-center gap-1 rounded border border-[#8957e5]/40 px-2 py-0.5 text-xs text-[#a371f7] hover:bg-[#8957e5]/10"
        >
          <Sparkles className="h-3 w-3" aria-hidden="true" />
          Claudify
        </button>
      ) : (
        <button
          type="button"
          // An ORPHAN is removable too, by a different route: git
          // cannot remove it (its repository is gone), so the Rust side
          // deletes the directory after re-checking that it is still
          // orphaned. Gating it on `safe` left the user looking at 2.5
          // GB they were told about and could not act on.
          disabled={(!safe && !orphaned) || removing}
          onClick={() => (orphaned ? onRemoveOrphan(wt) : onRemove(wt))}
          title={
            orphaned
              ? // The title no longer carries the WARNING (#845). It
                // used to be the only place "nothing about the contents
                // can be checked" was said before the click, which made
                // a hover-only surface load-bearing on a page with a
                // mobile layout. `ConfirmRemoveOrphan` says it now, in a
                // dialog that exists on touch, so this is back to what a
                // title is for: naming where the button goes.
                "Ask before deleting this directory — its repository is gone, so nothing about the contents can be checked"
              : safe
                ? "Remove this worktree"
                : // A prunable row says where its action IS, which is
                  // the dead end #793 reported: the button is correctly
                  // disabled -- there is no directory to remove -- and
                  // the tooltip restating "directory is gone — prunable"
                  // told the user a diagnosis they had already read on
                  // the row, with no remedy. So it names the header
                  // affordance, and says why the action is not here: the
                  // verb is repository-wide.
                  //
                  // It no longer PREFIXES `safetyReason` (#814). It used
                  // to, because the row's own text led with the problem
                  // and the reassurance had to be smuggled in here --
                  // "nothing can be lost" was a tooltip fact. The row now
                  // opens with "nothing to lose", so repeating it would
                  // make the tooltip a second copy of the line it hangs
                  // off. What is left is the only thing the row cannot say
                  // for itself: which affordance to use, and why it is not
                  // on this row.
                  wt.safety.kind === "prunable"
                  ? "Nothing to remove — the directory is already gone. Use “Prune stale registrations” above the list; git prunes a whole repository at once."
                  : safetyReason(wt.safety)
          }
          className={`shrink-0 rounded border px-2 py-0.5 text-xs ${
            (safe || orphaned) && !removing
              ? "border-[#f85149]/40 text-[#f85149] hover:bg-[#f85149]/10"
              : "border-[#30363d] text-[#8b949e] opacity-50"
          }`}
        >
          {/* The ellipsis says a question comes first (#845), matching
              "Remove anyway…" beside it and "Delete {n}…" on the
              branches page. A bare "Delete" on a button that now opens a
              dialog would understate it in the one direction that
              matters: a user who expects the deletion to have happened
              and does not see the modal has lost nothing, but one who
              expects a prompt and does not get one has lost a
              directory. */}
          {removing ? "Removing…" : orphaned ? "Delete…" : "Remove"}
        </button>
      )}
      {/* On EVERY row now, not only the assessed ones (#770).

          It used to appear solely beside "Remove anyway…", which meant
          the kebab existed exactly where removal was already on screen
          and was absent from the 124 rows where it was the missing
          affordance. Removal past the gate was reachable only from the
          Claudify toast's "I read the assessment" button — a transient
          surface for an unrecoverable action, so a stray click cost
          another agent invocation to get back.

          The ORPHANED rows keep their own Delete button as the only
          route: git cannot act on them at all, so the menu's removal
          items would be the wrong call. The kebab itself is still
          rendered there, because the Claudify pair may apply. */}
      <WorktreeKebab
        worktree={wt}
        assessed={assessed}
        onClaudify={onClaudify}
        onForget={onForget}
        onRemove={onRemove}
        onForce={onForce}
        onUnlock={onUnlock}
      />
      </span>
      {/* The disclosure, not a second action: the row keeps its
          one-action rule and this only reveals what the app already
          knows. */}
      {/* The main checkout reports how far behind it is and, until now,
          offered no way to act on it -- so fixing it meant leaving the
          app for a terminal, which is the thing this view exists to
          avoid. Its staleness is also what makes the worktrees below it
          stale.

          Disabled rather than hidden when the tree is dirty, with the
          reason in the title: an absent button just looks broken, while
          a greyed one that says "3 uncommitted files" teaches. Same
          rule `PrActions` applies to an unavailable merge.

          In practice this branch is unreachable for the button, which
          only renders when `is_main`: `classify` returns
          `Safety::MainCheckout` for the main checkout BEFORE it looks at
          `git status`, so `safety.kind` is never "dirty" there. The
          refusal the user actually meets comes from `pull_checkout`,
          which re-checks on the spot. Kept because the cost is a title
          attribute and the alternative -- deleting it and later giving
          the main checkout a real dirty verdict -- silently loses the
          explanation. */}
      {wt.is_main ? (
        <button
          type="button"
          disabled={dirtyCount !== null || pulling}
          onClick={() => onPull(wt)}
          title={
            dirtyCount !== null
              ? `${dirtyCount} uncommitted file${dirtyCount === 1 ? "" : "s"} — commit or stash first`
              : "Fast-forward this checkout to its upstream"
          }
          className={`shrink-0 rounded border px-2 py-0.5 text-xs ${
            dirtyCount !== null || pulling
              ? "border-[#30363d] text-[#8b949e] opacity-50"
              : "border-[#30363d] text-[#e6edf3] hover:bg-[#161b22]"
          }`}
        >
          {pulling ? "Updating…" : "Update to latest"}
        </button>
      ) : null}
      {/* Fetch: makes the COMPARISON true without moving the branch
          (#788).

          The closing half of #788. Saying "as of 2 days ago" beside the
          verdict tells the user their answer is old and leaves them only
          one way to get a new one -- Update, which performs the merge.
          So finding out whether you were behind required ceasing to be
          behind, and a user who only wanted to know had to accept a
          change to their working tree to find out. This separates the
          question from the answer.

          NOT disabled on a dirty tree, unlike Update beside it, and that
          difference is the whole point: `git fetch` writes only
          remote-tracking refs, so uncommitted work is not at risk and
          there is nothing for it to conflict with. A dirty main checkout
          is in fact the case where this matters most -- it is the one
          where Update is refused outright, which until now left the row
          with no way to refresh its own verdict at all.

          Placed BEFORE Update rather than after, because it is the
          cheaper and more reversible of the two and reads as the step you
          take first. A row's one-action rule is not broken here: Update
          is still the primary action and this renders only on the main
          checkout, the same single row Update does. */}
      {wt.is_main ? (
        <button
          type="button"
          disabled={fetching}
          onClick={() => onFetch(wt)}
          title="Refresh this repository's view of its remote. Moves no branch and touches no file — it only makes the comparisons on this page current."
          className={`shrink-0 rounded border px-2 py-0.5 text-xs ${
            fetching
              ? "border-[#30363d] text-[#8b949e] opacity-50"
              : "border-[#30363d] text-[#e6edf3] hover:bg-[#161b22]"
          }`}
        >
          {fetching ? "Fetching…" : "Fetch"}
        </button>
      ) : null}
      {/* Beside the action, since its two limits -- refuses on a dirty
          tree, never merges -- are not guessable from the label. Now also
          the place that explains how Fetch differs from Update, which is
          the distinction a user meeting two buttons needs (#788). */}
      {wt.is_main ? (
        <HelpButton topic="update-checkout" />
      ) : null}
      {claudifiable ? (
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
          aria-label={`What is in ${pathBasename(wt.path)}`}
          className="shrink-0 rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#8b949e] hover:bg-[#161b22]"
        >
          {open ? "Hide" : "What's in it?"}
        </button>
      ) : null}
    </>
  );
  return (
    <div className="border-b border-[#30363d] last:border-b-0">
    {isMobile ? (
      <div className="flex flex-col gap-1 px-4 py-2.5 text-sm">
        <div className="flex items-baseline gap-3">
          {nameCell}
          {sizeCell}
        </div>
        {safetyCell}
        <div className="flex flex-wrap items-center gap-2">{actionCells}</div>
      </div>
    ) : (
    <div className="flex items-baseline gap-3 px-4 py-2.5 text-sm">
      {nameCell}
      {safetyCell}
      {sizeCell}
      {actionCells}
    </div>
    )}
    {open ? (
      <div className="px-4 pb-2.5 text-xs text-[#8b949e]">
        {assessing ? (
          "Reading…"
        ) : assessment ? (
          <>
            {/* An empty summary means git answered none of it, which is
                worth saying rather than rendering a blank line. */}
            <p>{assessmentSummary(assessment) || "Nothing could be measured here."}</p>
            {/* Three arms, not two (#976). `null` is a failed `git log`,
                and rendering nothing for it looks identical to a branch
                with no commits -- so it says which. An intentional
                elision (`subjects_elided`) stays a separate line, for the
                reason `claude/transcript.rs` keeps its skipped count
                apart from its unreadable one. */}
            {assessment.subjects === null ? (
              <p className="mt-1">Commit list could not be read.</p>
            ) : assessment.subjects.length > 0 ? (
              <ul className="mt-1 list-inside list-disc">
                {assessment.subjects.map((subject, i) => (
                  <li key={`${i}-${subject}`} className="truncate">
                    {subject}
                  </li>
                ))}
                {assessment.subjects_elided > 0 ? (
                  <li className="list-none">and {assessment.subjects_elided} more</li>
                ) : null}
              </ul>
            ) : null}
          </>
        ) : (
          "Could not read this worktree."
        )}
      </div>
    ) : null}
    </div>
  );
}

/// Confirmation, naming the path and the branch.
///
/// A MODAL, not an inline banner: with 149 worktrees on one repository
/// the user clicks a row far down the page, and a prompt rendered at the
/// top is off-screen -- indistinguishable from nothing happening.
///
/// A count is not enough to act on safely either: the user needs to see
/// WHICH directory is about to disappear.
function ConfirmRemove({
  wt,
  onConfirm,
  onCancel,
}: {
  wt: Worktree;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog open onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="max-w-lg">
        <DialogTitle>Remove this worktree?</DialogTitle>
        <ActingOnDesktop />
        <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">{wt.path}</p>
        <p className="mt-2 text-sm text-[#8b949e]">
          Branch <span className="font-mono">{wt.branch || "detached"}</span> is merged and
          pushed
          {wt.merged_at ? <> (merged {wt.merged_at})</> : null}, so nothing is lost.
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
          >
            Remove
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/// Confirmation for deleting an ORPHANED directory (#845).
///
/// This is the one deletion in the app where NOTHING about the contents
/// was verified, and until #845 it was also the only directory-deleting
/// action with no dialog at all. The ceremony was exactly inverted: a
/// PROVABLY merged-and-pushed worktree got `ConfirmRemove`, and an
/// unverifiable one got a single click from the row.
///
/// The argument for having no dialog (the old comment on
/// `runRemoveOrphan`) was that "nothing about the contents can be
/// checked, so a dialog could only repeat what the button's title
/// already says". It fails on its own terms, and the app's own help text
/// is the proof -- `help/topics.ts`'s "orphaned-worktrees" says "you are
/// the only check" and "If you are unsure, copy the directory somewhere
/// first. Nothing about it can be recovered afterwards." That advice is
/// only actionable BEFORE the click, so a surface that appears only
/// after it cannot carry it. It is reproduced here verbatim rather than
/// paraphrased: it is the one piece of advice that survives the
/// deletion, and two wordings of it would be two pieces of advice.
///
/// The `title` the old argument leaned on is hover-only. This page has a
/// mobile layout (`isMobile`), and on touch there is no hover -- so on
/// the phone the justification described a surface that does not exist.
///
/// At least as heavy as `ConfirmRemove`, as #845 asks, and it carries
/// one thing that dialog does not: the SIZE, measured on open. An
/// orphan's size is absent everywhere else on this page (see
/// `useOrphanSize`), so the dialog is the only place it can be stated --
/// and "2.5 GB" is what makes "copy it somewhere first" a decision
/// rather than a slogan.
///
/// The size has three states and says which one it is in, never
/// defaulting to a number. `formatSize(0)` would read as "this tree is
/// empty, delete it" -- the most damaging thing this dialog could say
/// about a directory it could not measure, which is the same rule
/// `size_worktrees` states about flattening its own nulls.
function ConfirmRemoveOrphan({
  wt,
  onConfirm,
  onCancel,
}: {
  wt: Worktree;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  // Measured because the dialog is open, not because the row exists.
  const { bytes, measuring, failed } = useOrphanSize(wt.path, true);

  return (
    <Dialog open onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="max-w-lg">
        <DialogTitle>Delete {pathBasename(wt.path)}?</DialogTitle>
        <ActingOnDesktop />
        {/* The PATH, for the reason `ConfirmRemove` states: a count is
            not enough to act on safely, the user needs to see WHICH
            directory is about to disappear. More so here -- there is no
            branch name and no merge date to recognise it by, because
            the repository that held them is gone. */}
        <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">{wt.path}</p>
        <p className="mt-3 text-sm text-[#e6edf3]">
          {measuring
            ? "Measuring how much is in here…"
            : failed
              ? // NAMED, not silently omitted. An unmeasurable
                // directory is the second thing about this orphan that
                // could not be checked, and a dialog that simply left
                // the size out would look like one that had measured
                // nothing worth mentioning.
                "How much is in here could not be measured either."
              : bytes === null
                ? "How much is in here is not known."
                : `This frees ${formatSize(bytes)}.`}
        </p>
        {/* The red sentence, and the whole reason this dialog is as
            heavy as the forced-removal one. `forceWarning` can be
            specific about what is at risk because git could still be
            asked; here it could not, so the honest statement is the
            absence itself. */}
        <p className="mt-2 text-sm text-[#f85149]">
          Nothing inside could be checked — not whether it holds uncommitted work,
          and not whether its branch ever merged. The repository that could have
          answered is gone, so you are the only check.
        </p>
        {/* The help text's own sentence, verbatim (`help/topics.ts`,
            "orphaned-worktrees"). The only advice that survives this
            click, and the only surface it can be read on in time. */}
        <p className="mt-2 text-sm text-[#8b949e]">
          If you are unsure, copy the directory somewhere first. Nothing about it
          can be recovered afterwards.
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
          >
            Cancel
          </button>
          {/* Names the reading, not just the act -- the same shape as
              the forced removal's "I have reviewed this — remove it".
              A bare "Delete" here would be the one-click deletion this
              dialog exists to stop, with a modal in front of it. */}
          <button
            type="button"
            onClick={onConfirm}
            className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
          >
            Delete it anyway
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/// A clock that ticks ONCE per repository, when its first sizing pass
/// settles (#817).
///
/// The page freezes its row order against the size stream on purpose --
/// see the long comment on `snapshot` -- but the freeze is there to
/// protect an order the USER chose, and on arrival the user has chosen
/// nothing. This is what lets the snapshot be re-taken at the one moment
/// when re-ordering is free: the initial burst has landed, the rows are
/// no longer skeletons, and no considered click is in flight.
///
/// State adjusted DURING render -- React's documented pattern for
/// "derive from a prop change" -- rather than `useEffect` + state. The
/// effect form would paint the pre-settle order first and the settled
/// order on the following frame, so the list everybody sees on arrival
/// would visibly re-shuffle once: reintroducing, at exactly the moment
/// the user is first looking at it, the motion this page goes to such
/// lengths to avoid. Setting state during render instead makes React
/// re-run this component before it commits anything, so the settled
/// order is the FIRST one painted and no intermediate frame exists.
///
/// Not a `useRef` either, though that reads more naturally. Mutating a
/// ref during render is what `react-hooks/refs` forbids, and it is
/// right to: a ref write does not schedule the re-render that the new
/// value needs to be reflected, so the value would land one render late
/// -- which for a memo key means the list paints in the stale order
/// exactly once, the very flash this is avoiding.
///
/// Keyed by repository path and reset when it changes: each repository
/// runs its own sizing pass, so switching to one whose sizes have never
/// been measured must get the same one free re-sort rather than
/// inheriting the previous repo's spent clock.
///
/// Returns a counter rather than a boolean only because it feeds a memo
/// key built by string concatenation; one tick is all it ever takes.
function useFirstSizingSettled(repoPath: string | undefined, sizing: boolean): number {
  const [seen, setSeen] = useState<{
    repo: string | undefined;
    started: boolean;
    ticks: number;
  }>({ repo: repoPath, started: false, ticks: 0 });

  // The pass has to be observed STARTING before its finish counts.
  // Without this, the very first render -- where the query has not begun
  // fetching yet and `sizing` is still false -- would read as "already
  // settled" and spend the tick before a single measurement existed,
  // which is the bug it is meant to fix with extra steps.
  //
  // EVERY branch must return `seen` ITSELF when it has nothing to
  // change. Returning a fresh object with equal fields instead -- a
  // `{ ...seen, started: true }` on a `seen` already started -- fails
  // the identity check below, sets state, re-renders, and builds another
  // equal-but-new object: React's "Too many re-renders", reached on the
  // ordinary path rather than an edge case. Hence the field comparisons
  // in each guard rather than just the state transitions.
  const next =
    seen.repo !== repoPath
      ? { repo: repoPath, started: sizing, ticks: 0 }
      : sizing && !seen.started
        ? { ...seen, started: true }
        : !sizing && seen.started && seen.ticks === 0
          ? { ...seen, ticks: 1 }
          : seen;
  if (next !== seen) setSeen(next);
  return next.ticks;
}

export function WorktreesPage() {
  const {
    data: repos,
    isLoading,
    isError,
    error,
    refetch,
    dataUpdatedAt,
    // What the walk could not read (#951). Every count on this page is
    // taken over what it DID read, so a non-empty list makes each of them
    // a floor -- and the orphan header below is the one where that had to
    // be decided rather than merely noted.
    // `= []` so a test double of `useWorktrees` that predates #951 --
    // and there are several, each mocking only the fields it cares about
    // -- renders as "nothing unreadable" rather than crashing. Absent and
    // empty mean the same thing here: no shortfall was reported.
    unreadable = [],
  } = useWorktrees();
  const partial = unreadable.length > 0;
  // The scan's INPUT (#952): whether there were any folders to walk at
  // all, which nothing in the scan's OUTPUT can distinguish from a walk
  // that found nothing. One cache hit -- `staleTime: Infinity`.
  const { dirs } = useWorktreeDirs();
  const filters = useActiveFilters();
  const { setFilter } = useFilters();
  const isMobile = useIsMobile();

  const selected = repos?.find((r) => r.path === filters.repo) ?? repos?.[0];
  /// The instant every ref age on this page is measured against (#788).
  ///
  /// `Date.now()` is NOT called here, and that is not a stylistic
  /// preference -- eslint's impure-render rule would reject it, and the
  /// rule is right. `Sparkline` and `HealthConditions` in
  /// `SystemHealthPage.tsx` both document this same constraint and both
  /// take their `now` as a prop for the same two reasons.
  ///
  /// `dataUpdatedAt` is also the more HONEST anchor, not merely the pure
  /// one. `fetched_at` is `FETCH_HEAD`'s mtime, stat'd by the scan; this
  /// is the moment that scan's answer arrived. Measuring between them
  /// gives the age of the evidence as of when the evidence was read,
  /// which is what "as of 4h ago" claims. Anchoring on paint time instead
  /// would let a tab left open overnight count its own idleness as
  /// staleness in the repository, and a row would drift from "as of 2h
  /// ago" to "as of 14h ago" without anything about the repository having
  /// changed -- a number that moves on its own being exactly the kind
  /// nobody trusts.
  ///
  /// NO `|| Date.now()` fallback, and that is load-bearing rather than
  /// lint appeasement -- eslint rejects the call even inside this
  /// `useMemo`, and it is right to: a fallback clock would make the value
  /// differ between two renders with identical inputs.
  ///
  /// Before the first scan settles `dataUpdatedAt` is 0, so this is the
  /// 1970 epoch. Every real `fetched_at` is then in the FUTURE relative to
  /// it, which `refAge` answers by going silent -- "we cannot tell how
  /// stale this is" -- rather than by printing a negative age or a
  /// 20,000-day one. That is the correct answer for "no scan has landed
  /// yet", and it beats a present-time fallback, which would confidently
  /// report an age computed against a clock the data never came from.
  ///
  /// Unreachable in practice regardless: `isLoading` is true until the
  /// first scan settles, and that branch returns the scanning skeleton
  /// below without rendering a row. The epoch is the safe floor under a
  /// path nothing takes, not a case being relied on.
  const scannedAt = useMemo(() => new Date(dataUpdatedAt), [dataUpdatedAt]);
  // `data` is deliberately NOT read here. `partial` already contains the
  // settled set -- the hook merges `data` over the stream, so a row's
  // authoritative verdict wins on any path it has -- and reading both
  // would reintroduce the question "which of these two is the row's
  // verdict?" that #830's merge exists to answer once.
  const {
    isLoading: classifying,
    isError: classifyFailed,
    error: classifyError,
    refetch: retryClassify,
    partial: verdicts,
    pending: classifyPending,
    total: classifyTotal,
    failed: classifyUnknown,
  } = useWorktreeSafety(selected?.path, selected?.worktrees);
  const sizesQuery = useWorktreeSizes(selected?.path);
  // The settled answer when there is one, and the sizes streamed so far
  // when there is not. Before #754 this was `data` alone, so every row
  // held a skeleton until the LARGEST tree in the repository had been
  // walked -- MEASURED at 21.40s for one 200 GB checkout, and minutes
  // across a repository with 100 worktrees. A row's own size is known
  // long before that and there was no reason to withhold it.
  const sizes = sizesQuery.data ?? sizesQuery.partial;
  // `isFetching`, NOT `isLoading`. A DISABLED query reports `isLoading:
  // true` forever in TanStack v5 -- it has no data and never will --
  // so on "All repositories", where no repo is selected and the sizing
  // query never runs, every row waited on a request that was never
  // made. `isFetching` is true only while something is actually in
  // flight.
  const sizing = sizesQuery.isFetching;
  // The sizing pass failed outright, so NO size is coming for any row in
  // this repository. Until #769 this was never read: `useWorktreeSizes`
  // exposed `isError` and the page ignored it, so a rejected walk showed
  // skeletons while it retried and then collapsed to an em dash -- the
  // one reading the size cell's own comment says is wrong, because it
  // claims a measurement that never happened.
  const sizingFailed = sizesQuery.isError;
  const remove = useRemoveWorktree();

  /// Copy rather than spawn. The command lands in the user's own shell,
  /// where their config applies and `claude` resolves -- and there is no
  /// portable way to open "the user's terminal" anyway.
  const markRead = useMarkAssessed();
  const clearRead = useClearAssessed();

  /// Restore a worktree's Claudify action.
  ///
  /// Re-LOCKS force removal rather than unlocking it, so it is the safe
  /// direction and needs no confirmation of its own.
  const forget = (wt: Worktree) => {
    void clearRead(wt.path).then(
      () => toast.success(`${pathBasename(wt.path)} is no longer marked as assessed`),
      (e: unknown) =>
        toast.error("Could not clear the assessment", {
          description: typeof e === "string" ? e : undefined,
        }),
    );
  };

  /// "I read the assessment", the ONLY route to "Remove anyway…".
  ///
  /// Hoisted out of the clipboard's success branch, where it used to
  /// live. A failed copy took an early return, so the action vanished
  /// with it -- and the copy fails on a phone, where `copyText` reports
  /// "This window has no clipboard access." in a non-secure webview
  /// context, and on any desktop whose window is not focused. Losing
  /// the ONLY path to force-removal because the clipboard was busy is
  /// not a reasonable failure mode.
  const assessmentAction = (wt: Worktree) => ({
    label: "I read the assessment",
    onClick: () => {
      void markRead(wt.path).then(
        () => toast.success(`${pathBasename(wt.path)} can now be removed`),
        (e: unknown) =>
          toast.error("Could not record the assessment", {
            description: typeof e === "string" ? e : undefined,
          }),
      );
    },
  });

  const claudify = (wt: Worktree) => {
    claudifyCommand(selected?.path ?? "", wt.path, wt.branch).then(
      async ({ command, claude_installed }) => {
        // The phone has no terminal to paste into, and no usable
        // clipboard either. Show the command instead, framed for the
        // machine it actually runs on -- the desktop this phone drives.
        if (IS_MOBILE_BUILD) {
          setClaudifying({ worktree: wt, command, claudeInstalled: claude_installed });
          return;
        }
        // `copyText` rather than `navigator.clipboard` directly: an
        // ABSENT clipboard throws synchronously on property access, so
        // the old `.then(ok, err)` attached neither handler and the
        // click produced no toast of either kind.
        const failure = await copyText(command);
        if (failure !== null) {
          // The action rides along even here: the command could not be
          // copied, but the user can still read the assessment by
          // other means, and this is their only way to say so.
          toast.error("Could not copy the command", {
            description: failure,
            action: assessmentAction(wt),
          });
          return;
        }
        toast.success("Command copied to the clipboard", {
          // The user has to switch apps; this is the only place to say
          // so. And if Claude Code is missing, better to learn it here
          // than as a `command not found` after pasting.
          description: claude_installed
            ? "Paste it in your terminal to start the assessment."
            : "Paste it in your terminal. Claude Code was not found on this machine.",
          // The ONLY way to reach "Remove anyway…", and it is here
          // rather than automatic because that button removes a worktree
          // past its safety gate. Copying a prompt is not evidence
          // anyone read the answer; clicking this is.
          action: assessmentAction(wt),
        });
      },
      (e: unknown) =>
        toast.error("Could not build the command", {
          description: typeof e === "string" ? e : undefined,
        }),
    );
  };
  /// The command to show on the phone, which has no terminal to paste
  /// into. Null on the desktop, always: that path copies instead.
  const [claudifying, setClaudifying] = useState<{
    worktree: Worktree;
    command: string;
    claudeInstalled: boolean;
  } | null>(null);
  /// The chosen ordering, and the gesture clock that fixes it (#771).
  ///
  /// Local state rather than the `filters` store: sort here is a
  /// working choice made while clearing one repository, not a saved
  /// preference to restore on the next launch, and the store's `sort`
  /// key already means something else on the PR views.
  ///
  /// "Largest first" is the default because it is the question the page
  /// exists to answer -- on a machine with 100+ worktrees per repo,
  /// "which of these is biggest" is why you opened it.
  const [sort, setSort] = useState<WorktreeSort>("size-desc");
  /// Bumped by an explicit gesture -- choosing a sort, or clicking
  /// "re-sort" -- and by nothing else. It is what freezes the order
  /// against the size stream; see the comment on `shown`.
  const [sortedAt, setSortedAt] = useState(0);
  const chooseSort = (next: WorktreeSort) => {
    setSort(next);
    setSortedAt((n) => n + 1);
  };
  const [pending, setPending] = useState<Worktree | null>(null);
  /// The path currently being removed, or null. A path rather than a
  /// boolean so only the clicked row goes busy.
  const [removing, setRemoving] = useState<string | null>(null);
  const [bulkOpen, setBulkOpen] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const removalProgress = useRemovalProgress();
  const removeMany = useRemoveWorktrees();
  const forceRemove = useRemoveWorktreeForced();
  const { data: assessedPaths } = useAssessed();
  const { data: prs = [] } = usePullRequests();
  // Only while the confirmation is open. Docker is a subprocess call,
  // and the Worktrees page has no business paying for it just to sit
  // there -- the manifest is needed at the moment of confirming, not
  // on every render of a list nobody is acting on.
  const assessed = new Set(assessedPaths ?? []);

  // Verdicts and sizes are merged onto the listing AS THEY ARRIVE, so
  // the page is useful immediately and gets more informative rather than
  // blocking on git. Merged here rather than by refetching the list, so
  // the page never flickers back to unclassified.
  //
  // Computed HERE, above every early return, because the sort below is a
  // hook: React requires the same hooks in the same order on every
  // render, and the orphan and all-repositories branches return before
  // this point.
  // `has` rather than `??`: an explicit null VALUE is "the walk was
  // abandoned, no number is coming" (#769), where an absent KEY is "not
  // measured yet". `??` collapses the two, so a row that gave up would
  // fall back to its stale size and keep its skeleton forever.
  //
  // VERDICTS are merged the same way and for the same reason (#830).
  // `classified` is the settled set and arrives only when the whole
  // repository is done; `verdicts` carries each row's own answer the
  // moment it exists. Before this the page read `classified` alone, so on
  // a 111-worktree repository every row held its skeleton until the
  // slowest branch finished -- and since classification makes an
  // unbounded number of git calls per worktree, "until" could be never.
  //
  // The listing is the base, so the set of ROWS never depends on how far
  // classification has got: a row appears with its name, branch and size,
  // and its verdict fills in underneath it. A verdict for a path the
  // listing does not have is APPENDED rather than dropped -- the two
  // passes run against the same repository a moment apart, and a worktree
  // created in that gap should appear rather than wait for the next
  // listing refetch.
  // A `??` on an optional field yields a NEW `[]` on every render when
  // the left side is undefined, which would change the identity of both
  // memos below on every render and make them useless. The linter names
  // this case specifically.
  const listedRows = useMemo(() => selected?.worktrees ?? [], [selected]);
  // Memoised as well (#1150). `rows` feeds `withSizes` below, so a
  // fresh array every render would make that memo useless -- the
  // dependency would change on every render by construction. The
  // linter's exhaustive-deps rule catches exactly this, which is how
  // the omission was found rather than shipped.
  const rows = useMemo(
    () => [
      ...listedRows,
      ...[...verdicts.values()].filter((v) => !listedRows.some((l) => l.path === v.path)),
    ],
    [listedRows, verdicts],
  );
  // Memoised too (#1150): memoising `shown` against a `withSizes` that
  // is rebuilt every render would save nothing, because the dependency
  // would change every time. Both, or neither.
  //
  // `rows` is itself derived per render, so the deps are the three
  // things that actually move: the listing, the verdict map and the
  // size map -- each of which now changes once per frame rather than
  // once per event.
  const withSizes = useMemo(() => rows.map((w) => ({
    ...w,
    // The verdict's fields win over the listing's -- that is the point --
    // but ONLY for the fields classification actually answers. It also
    // carries a `size_bytes`, which it does not measure and which is
    // therefore stale or null on arrival, so the explicit assignment below
    // has to come AFTER this spread. The fallback there reads `w`, the
    // pre-spread row, so an unmeasured worktree keeps the LISTING's size
    // rather than the verdict's empty one. Order is load-bearing here;
    // swapping these two lines silently blanks every size.
    ...(verdicts.get(w.path) ?? {}),
    size_bytes: sizes?.has(w.path) ? (sizes.get(w.path) ?? null) : w.size_bytes,
    /// This row's own walk was abandoned, OR the whole repository's
    /// sizing pass failed. Either way no number is coming for it, and
    /// the row must say so instead of holding a skeleton.
    sizeUnmeasurable: (sizes?.has(w.path) && sizes.get(w.path) === null) || sizingFailed,
  })), [rows, verdicts, sizes, sizingFailed]);

  // SORTING VS STREAMING (#771).
  //
  // Sizes land progressively (#754, #758), so a live size sort re-orders
  // rows under the cursor: the row you were reaching for slides away as
  // some other row's `du` returns. On this page that is not a cosmetic
  // annoyance -- the button it moves out from under you removes a
  // directory -- so a silent live re-sort is the one option that is
  // genuinely unsafe.
  //
  // The previous code answered this by refusing to sort at all until
  // every size was in, which is why the page could sit in path order for
  // two minutes with nothing to explain it.
  //
  // So: sort on the values known when the sort was CHOSEN, and re-sort
  // only on an explicit gesture. `sortedAt` is that gesture's clock --
  // picking a sort takes a fresh snapshot, and clicking "re-sort" takes
  // another. Between gestures the ORDER is frozen while each row's
  // displayed size keeps updating live, so a landing measurement is
  // never hidden; it simply does not move anything.
  //
  // What is memoised is the ORDER ALONE -- a list of paths -- and not
  // the rows themselves. Freezing the rows would freeze their contents
  // too, so a size landing after the snapshot would stay invisible
  // behind its skeleton until the next gesture. That is the opposite of
  // the intent: the number is never withheld, only its power to move
  // the row is.
  //
  // `orderKey` is the only dependency that may bump the memo, since the
  // freshly-built `withSizes` array is a new reference on every render
  // and listing it would restore exactly the live re-ordering this
  // exists to prevent. `length` rides along so a worktree appearing or
  // disappearing is not silently dropped from the list.
  //
  // THE FIRST BURST IS NOT A GESTURE TO PROTECT (#817).
  //
  // `settledAt` joins the clock, and the distinction it draws is the
  // whole of that issue's second complaint. Everything above is about
  // not moving a row out from under a user who CHOSE an order. On
  // arrival nobody has chosen anything: the default is "largest first",
  // and the first measurements necessarily land after the mount, so the
  // page used to greet a user who had touched nothing with "re-sort --
  // 94 newly measured". That is not honesty about a frozen order, it is
  // the page confessing that data arrived.
  //
  // There is also nothing to protect at that moment. The hazard is a
  // button sliding out from under a cursor that is already reaching for
  // it; during the initial burst the rows are still filling in their
  // skeletons and there is no considered click in flight to spoil.
  //
  // So the snapshot is re-taken ONCE, when the first sizing pass for
  // this repository settles, and the button thereafter means "things
  // changed since YOU chose" rather than "data arrived". Crucially this
  // is one bump, not a subscription to `sizing`: a clock that moved on
  // every pass would re-sort the list on each background refetch, which
  // is exactly the live re-ordering the paragraphs above call the one
  // genuinely unsafe option.
  const settledAt = useFirstSizingSettled(selected?.path, sizing);
  const orderKey = `${sort}:${sortedAt}:${settledAt}`;
  const snapshot = useMemo(
    () => ({
      order: sortWorktrees(withSizes, sort, assessed).map((w) => w.path),
      // Which paths already had a size when this order was fixed. Kept
      // alongside the order because it is the only way to tell later
      // that a measurement has landed SINCE -- comparing against the
      // live rows would compare them with themselves.
      measured: new Set(
        withSizes.filter((w) => w.size_bytes !== null && w.size_bytes !== undefined).map((w) => w.path),
      ),
      // Which paths were assessed when this order was fixed, for the
      // same reason and read the same way (#817).
      //
      // `sortWorktrees` ranks assessed rows above unassessed ones on
      // EVERY sort, size or not, so an assessment landing changes what
      // the correct order would be just as a measurement does. The
      // staleness count used to look only at sizes, which left that
      // half of the data silently stale: the order was wrong and the
      // page did not say so -- precisely the dishonesty the re-sort
      // button exists to prevent.
      assessedThen: new Set(
        withSizes.filter((w) => assessed.has(w.path)).map((w) => w.path),
      ),
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [orderKey, withSizes.length],
  );
  const order = snapshot.order;
  // The live rows, in the frozen order. A path the snapshot has not
  // seen yet sorts to the end rather than being dropped -- a worktree
  // must never vanish from the list because it appeared between
  // gestures.
  // Memoised against the batched map identity (#1150).
  //
  // `withSizes` is a new array on every render -- the file says so at
  // its own definition -- and this rebuilds a Map of every path and
  // re-sorts the whole list. With the streams batched per frame that is
  // now ~1 rebuild per frame rather than ~295 in a burst, and memoising
  // makes the saving real rather than merely available: without it the
  // sort runs again on every unrelated render too.
  //
  // The frozen sort order (#771/#817) is untouched -- `order` is still
  // the snapshot's, and a path it has not seen still sorts to the end
  // rather than being dropped.
  const rank = useMemo(() => new Map(order.map((p, i) => [p, i])), [order]);
  const shown = useMemo(
    () =>
      [...withSizes].sort(
        (a, b) => (rank.get(a.path) ?? Infinity) - (rank.get(b.path) ?? Infinity),
      ),
    [withSizes, rank],
  );

  const [forcing, setForcing] = useState<Worktree | null>(null);
  /// The worktree whose lock the user is being asked to confirm
  /// clearing, or null (#775).
  ///
  /// Its own state rather than a flag on `forcing`: the two dialogs say
  /// opposite things -- one is about losing work irreversibly, the
  /// other about clearing a reversible guard -- and sharing a slot
  /// would put them one boolean away from showing the wrong one.
  const [unlocking, setUnlocking] = useState<Worktree | null>(null);
  const unlock = useUnlockWorktree();
  /// The bulk unlock's dialog and in-flight flag (#792).
  ///
  /// Two pieces of state rather than one nullable list, because the
  /// batch outlives its dialog: the dialog closes on confirm so the user
  /// gets their app back -- the same decision the bulk removal made for
  /// the same reason -- and the button then needs to know work is still
  /// running. The rows themselves come from `deadLocks` below, recomputed
  /// each render, so a freezing snapshot cannot go stale against them.
  const [bulkUnlockOpen, setBulkUnlockOpen] = useState(false);
  const [bulkUnlockBusy, setBulkUnlockBusy] = useState(false);
  const unlockMany = useUnlockWorktrees();
  /// Pruning's in-flight flag, and no dialog (#793).
  ///
  /// Deliberately the only cleanup on this page with no confirmation.
  /// Every registration it clears describes a directory git has already
  /// reported gone, so there is nothing to lose and nothing to review --
  /// and a dialog over an action with no recoverable loss is how users
  /// learn to click through the dialogs that do matter.
  const [pruning, setPruning] = useState(false);
  const prune = usePruneWorktrees();
  const [pullingPath, setPullingPath] = useState<string | null>(null);
  /// The row whose Fetch is in flight (#788). A SECOND path rather than
  /// a shared "busy" one: Fetch is available on a dirty main checkout
  /// where Update is refused, so collapsing them would grey out a button
  /// that is not blocked.
  const [fetchingPath, setFetchingPath] = useState<string | null>(null);
  const removeOrphanFn = useRemoveOrphan();
  // Every repository's sizes, only on the all-repositories view. One
  // query per repository so results land progressively -- the full set
  // takes ~2 minutes, and blocking on it is what made this page look
  // stuck.
  const {
    sizes: allSizes,
    pending: sizesPending,
    total: sizesTotal,
  } = useAllWorktreeSizes(
    (repos ?? []).map((r) => r.path),
    !filters.repo,
  );

  /// The orphan whose deletion is awaiting confirmation, or null (#845).
  ///
  /// Its own slot rather than reusing `pending`, for the same reason
  /// `unlocking` is separate from `forcing`: the two dialogs make
  /// opposite claims. `ConfirmRemove` says "nothing is lost"; this one
  /// says nothing could be checked. Sharing a slot would put them one
  /// boolean away from showing the wrong one over the wrong directory.
  const [pendingOrphan, setPendingOrphan] = useState<Worktree | null>(null);

  /// Delete an orphaned directory, reporting the Rust side's own words.
  ///
  /// Reached only from `ConfirmRemoveOrphan`'s confirm button now
  /// (#845). It used to fire straight from the row, which made this the
  /// ONLY directory-deleting action in the app with no dialog -- and the
  /// only one where nothing about the contents had been verified. The
  /// argument for that (a dialog "could only repeat what the button's
  /// title already says") is dismantled in `ConfirmRemoveOrphan`'s own
  /// comment; the short version is that the help text's "copy the
  /// directory somewhere first" is only actionable before the click, and
  /// `title` does not exist on touch.
  ///
  /// The Rust re-check still happens at the moment of deletion and is
  /// still not a substitute: it confirms only that the directory is
  /// STILL an orphan, which says nothing about its contents.
  const runRemoveOrphan = (wt: Worktree) => {
    setRemoving(wt.path);
    removeOrphanFn(wt.path).then(
      () => {
        setRemoving(null);
        toast.success(`Deleted ${pathBasename(wt.path)}`);
      },
      (e: unknown) => {
        setRemoving(null);
        toast.error(`Could not delete ${pathBasename(wt.path)}`, {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };
  /// Clear this repository's stale worktree registrations (#793).
  ///
  /// No confirmation: see `pruning`. What it does need is an HONEST
  /// result, and the count is it -- `git worktree prune` is silent on
  /// success, so a bare "Pruned" would be indistinguishable from a
  /// no-op on a repository somebody had already pruned in a terminal.
  /// The Rust side counts by listing before and after, so the number is
  /// what actually went rather than what we hoped would.
  ///
  /// A zero is reported as a neutral message, not a success and not an
  /// error. Nothing was wrong and nothing happened, and dressing that up
  /// either way would misdescribe it.
  const runPrune = () => {
    setPruning(true);
    prune(selected?.path ?? "").then(
      (cleared) => {
        setPruning(false);
        if (cleared === 0) {
          toast.info("Nothing to prune", {
            description:
              "Git found no stale registrations — they may already have been cleared.",
          });
        } else {
          toast.success(
            `Pruned ${cleared} stale registration${cleared === 1 ? "" : "s"}`,
            {
              // Says what did NOT happen as plainly as what did. The
              // word "prune" beside a list of worktrees invites reading
              // this as a deletion, and on a page whose other buttons
              // delete directories that confusion is worth one line.
              description: "No files were deleted — only git's records of directories already gone.",
            },
          );
        }
      },
      (e: unknown) => {
        setPruning(false);
        toast.error("Could not prune stale registrations", {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  const pull = usePullCheckout();

  /// Fast-forward the main checkout, reporting git's own words either
  /// way. A generic "could not update" would throw away the one part of
  /// the failure that tells the user what to do.
  const runPull = (wt: Worktree) => {
    setPullingPath(wt.path);
    pull(wt.path).then(
      (out) => {
        setPullingPath(null);
        // Summarised, not verbatim: a real fast-forward's stdout is the
        // whole diffstat, and on a busy repository that filled the
        // screen to say "it worked" (#652). `summarisePull` keeps the
        // distinction the old comment here was protecting -- git's
        // "Already up to date." is passed through, because replacing it
        // with "Updated" would claim a change that did not happen.
        toast.success(summarisePull(out));
      },
      (e: unknown) => {
        setPullingPath(null);
        toast.error(`Could not update ${pathBasename(wt.path)}`, {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  const fetchRefsFn = useFetchRefs();

  /// Refresh the repository's remote refs, moving no branch (#788).
  ///
  /// The success toast is WRITTEN HERE rather than echoing git, which is
  /// the one place this differs structurally from `runPull` above. `git
  /// pull` narrates itself -- a diffstat, or "Already up to date." --
  /// which is why `summarisePull` exists to trim it. `git fetch` writes
  /// its progress to stderr and NOTHING to stdout, so the resolved value
  /// is routinely the empty string; passing it to a toast would show a
  /// blank one, and passing it through `summarisePull` would be worse,
  /// inventing a verdict about commits from a string that mentions none.
  ///
  /// So the toast says what the app knows: the refs were refreshed, and
  /// the rows are about to re-answer. It does NOT say whether anything
  /// new arrived -- the rows themselves say that a moment later, from the
  /// re-classification the hook invalidates, and a toast that guessed
  /// "up to date" here could contradict the row it appeared over.
  ///
  /// The hook's invalidation is what repaints; nothing is patched
  /// locally. A ref age patched optimistically would be the page
  /// claiming freshness it had not verified, on the one feature whose
  /// entire purpose is not doing that.
  const runFetch = (wt: Worktree) => {
    setFetchingPath(wt.path);
    fetchRefsFn(wt.path).then(
      () => {
        setFetchingPath(null);
        toast.success(`Refreshed ${pathBasename(wt.path)} from its remote`, {
          description: "The rows below are re-checking against the new refs.",
        });
      },
      (e: unknown) => {
        setFetchingPath(null);
        // Git's own words, same rule `runPull` follows: a fetch failure
        // usually names the host, the permission or the ref, and
        // "could not fetch" names none of them.
        toast.error(`Could not reach ${pathBasename(wt.path)}'s remote`, {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  /// The orphan confirmation, as ONE element rendered on two paths
  /// (#845).
  ///
  /// An orphan row appears in two places -- the Orphaned section, which
  /// is its own early `return`, and a repository page that happens to
  /// contain one -- so a dialog written into either branch alone leaves
  /// the other click unconfirmed. That is exactly the split that let this
  /// action ship with no dialog at all, so the fix must not reproduce its
  /// shape.
  ///
  /// A local `const` rather than a second copy of the JSX, and rather
  /// than hoisting the whole page into one return: a copy is two things
  /// to keep in step, and the early returns are load-bearing -- the
  /// Orphaned section is deliberately not a repository page (an orphan
  /// belongs to no repository, so `selected` cannot express it).
  ///
  /// Built unconditionally and cheap when closed: `pendingOrphan` is null
  /// on every render but the one that matters, so this is `null` and
  /// `useOrphanSize` is never mounted -- no walk is started for a
  /// directory nobody asked about.
  const orphanDialog = pendingOrphan ? (
    <ConfirmRemoveOrphan
      wt={pendingOrphan}
      onCancel={() => setPendingOrphan(null)}
      onConfirm={() => {
        const target = pendingOrphan;
        setPendingOrphan(null);
        runRemoveOrphan(target);
      }}
    />
  ) : null;

  // Which worktree each live agent is working in (#1137).
  //
  // Here rather than beside the code that uses it, because six early
  // returns sit below this point: a hook called after any of them is a
  // conditional hook, and React fails the next render outright. The
  // test that caught it renders a state that takes one of those
  // branches.
  //
  // Gated on the Claude integration like every other reader of this
  // data: a user who has not enabled it is not asked to pay a poll for
  // a badge they did not ask for.
  const { prefs } = useUiPrefs();
  const claudeOn = prefs?.claude_integrations_enabled ?? false;
  const sessions = useClaudeSessions(claudeOn);
  const sessionList = sessions.list.data?.sessions;
  const occupiedBy = useMemo(() => worktreeSessions(sessionList), [sessionList]);

  if (isLoading) {
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center text-sm text-[#8b949e]">
        Scanning for worktrees…
      </div>
    );
  }

  if (isError) {
    return (
      <QueryError
        title="Could not scan for worktrees"
        message={errorMessage(error)}
        onRetry={() => void refetch()}
      />
    );
  }

  if (!repos || repos.length === 0) {
    // The same diagnosis-versus-shortfall split as `RepoPickerSidebar`
    // (#951): "Set the directories to scan in Settings" names the user's
    // configuration, and sending someone there when the directories were
    // fine and merely unreadable is the failure that issue is about.
    if (partial) {
      return (
        <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
          <p className="text-sm font-semibold text-[#e6edf3]">
            No repositories could be read
          </p>
          <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
            The scan ran but could not read where it looked, so it cannot say
            whether there are repositories there. The scanned directories may
            well be correct.
          </p>
          <div className="mx-auto mt-4 max-w-lg text-left">
            <PartialScanNotice
              unreadable={unreadable}
              consequence="nothing could be listed from them."
            />
          </div>
        </div>
      );
    }
    // And the same split again (#952): with no directories configured,
    // "No repositories found" is a claim about the user's disk that this
    // scan never tested. It looked nowhere. That is a task to state, not
    // a finding to report -- and the two want opposite words.
    if (dirs.length === 0) {
      return (
        <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
          <p className="text-sm font-semibold text-[#e6edf3]">
            Headstate does not know where your repositories are
          </p>
          <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
            No folders are being scanned yet. Set the directories to scan in
            Settings, at the bottom right.
          </p>
        </div>
      );
    }
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
        <p className="text-sm font-semibold text-[#e6edf3]">No repositories found</p>
        {/* NAMES them (#952). "Set the directories to scan in Settings"
            without saying which ones are set asks the user to go and
            look up what the app already knows. */}
        <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
          No git repositories under{" "}
          <span className="font-mono">{dirs.join(", ")}</span>. Change the
          directories to scan in Settings, at the bottom right.
        </p>
      </div>
    );
  }

  // "All repositories": a genuine rollup rather than the first repo.
  //
  // Deliberately READ-ONLY. Safety classification is per repo and costs
  // ~16s across all of them, so this view has no verdicts -- and every
  // removal path in this app is gated on a verdict. Offering Remove here
  // would mean either deleting without a safety check or blocking the
  // view on a 16-second scan; showing where the disk went, and sending
  // the user into the repo to act, is neither.
  // The Orphaned section. Its own branch rather than a repo page: an
  // orphan belongs to no repository, so `selected` cannot express it,
  // and the per-repo page's safety affordances do not apply.
  if (filters.repo === ORPHAN_FILTER) {
    const orphans = (repos ?? [])
      .flatMap((r) => r.worktrees)
      .filter((w) => isOrphaned(w.safety));
    return (
      <>
      {orphanDialog}
      <div className="rounded-md border border-[#30363d]">
        <div className="border-b border-[#30363d] px-4 py-3">
          {/* # The orphan verdict over an incomplete walk (#951)

              `caches/mod.rs` exists because of exactly this shape, and it
              takes the hardest line available: on a `truncated` walk it
              downgrades `Orphaned` to `Unknown` (`:202-206`) and REFUSES
              the deletion outright (`:495`, citing #747). The question
              #951 asks is whether this page must do the same, and the
              answer is NO -- for a reason specific to how the two
              verdicts are reached, not because this one matters less.

              A venv is called orphaned RESIDUALLY: "no live project in
              the index hashes to it". That verdict is a claim about the
              whole index, so an index that is too small turns a live
              venv into a deletion candidate. The completeness of the
              walk is load-bearing for correctness, and a short walk
              produces FALSE POSITIVES -- which is why it must refuse.

              An orphaned worktree is called orphaned POSITIVELY.
              `orphan_gitdir` in `scan.rs` reads the worktree's own `.git`
              file, takes the `gitdir:` path out of it, and checks that
              path does not exist. Nothing about that consults the rest of
              the walk. A directory the scan never reached cannot make a
              worktree we DID read look orphaned, so a short walk here
              produces no false positives at all -- only false NEGATIVES:
              orphans we failed to find.

              So the error has a known direction, and the honest response
              to a one-directional error is to name the direction. The
              count becomes "at least N", which is what the size total on
              the all-repositories header already does for an unmeasured
              worktree and for the same stated reason: a number that
              silently counts unknowns as zero is a confident wrong
              answer.

              QUALIFIED rather than SUPPRESSED, and that is a deliberate
              refusal of the tempting symmetry with `caches/mod.rs`.
              Hiding the section on a partial scan would hide orphans that
              are real, individually verified, and sitting on disk -- and
              this page exists to reclaim exactly them. It would also be
              the failure `ArtifactsPage:196-203` names: refusing what was
              measured because something else was not.

              And the DELETION is not gated, which is the sharpest part of
              this decision. It is safe not because the walk is complete
              but because it never depended on the walk: the removal path
              re-derives the orphan verdict on the one path it is about,
              and `Safety::Orphaned` is already un-removable through every
              other gate. Gating it on `partial` would block a delete on
              evidence that has no bearing on it -- a guard that looks
              careful and is merely in the way, which is how a real guard
              gets disabled later. */}
          <span className="text-sm font-semibold text-[#e6edf3]">
            {partial ? "at least " : ""}
            {orphans.length} orphaned worktree{orphans.length === 1 ? "" : "s"}
            {/* The most important help in the app: this is the only
                place it offers a delete having verified nothing. */}
            <HelpButton topic="orphaned-worktrees" />
          </span>
          {/* Says what these ARE before offering to delete them. The
              row's own reason says nothing can be checked; this says
              why that is, which is what makes the Delete button a
              considered choice rather than a gamble. */}
          <p className="mt-1 text-xs text-[#8b949e]">
            The repository each of these belonged to has been deleted, so git can
            no longer read them — not whether they hold uncommitted work, and not
            whether their branch ever merged. Deleting one removes the directory
            outright.
          </p>
        </div>
        {/* Why the header says "at least". The rows below are each real;
            what the shortfall costs is orphans that were never found, so
            the paths go here rather than the count being hidden. */}
        <PartialScanNotice
          unreadable={unreadable}
          consequence="there may be more orphans than the rows below."
        />
        {orphans.map((wt) => (
          <Row
            key={wt.path}
            wt={wt}
            repoPath=""
            assessed={false}
            onForce={setForcing}
            onUnlock={setUnlocking}
            onRemoveOrphan={setPendingOrphan}
            onPull={runPull}
            onFetch={runFetch}
            // No `fetchedAt`, and it cannot have one: an orphan's
            // repository is precisely what is gone, so there is nothing
            // left to have fetched. The prop's default of null is the
            // honest value. Neither button reaches these rows anyway --
            // both are gated on `is_main`, and an orphan is not a main
            // checkout -- and `upstreamReasonAged` is likewise only
            // rendered for `is_main`, so nothing here prints an age.
            scannedAt={scannedAt}
            onRemove={setPending}
            onClaudify={claudify}
              onForget={forget}
            removing={removing === wt.path}
          />
        ))}
      </div>
      </>
    );
  }

  if (!filters.repo) {
    // Sizes merged in as each repository answers. MEASURED: the full
    // set takes ~2 minutes, so awaiting it would show dashes for that
    // long with nothing to explain them -- which is what was reported.
    // `has`, not `??`. A worktree the walk gave up on (#769) arrives as
    // an explicit null VALUE, and `??` would fall straight through it to
    // the stale `w.size_bytes` -- erasing the one fact that stops the
    // row promising a number. An ABSENT key still means "not measured
    // yet" and must keep the existing value.
    const withSizes = repos.map((r) => ({
      ...r,
      worktrees: r.worktrees.map((w) => ({
        ...w,
        size_bytes: allSizes.has(w.path) ? (allSizes.get(w.path) ?? null) : w.size_bytes,
      })),
    }));
    const { worktrees, totalBytes, sizesComplete } = rollupRepos(withSizes);
    return (
      <div className="rounded-md border border-[#30363d]">
        <div className="flex items-baseline justify-between border-b border-[#30363d] px-4 py-3">
          {/* "at least" on a partial scan (#951), for the same reason the
              size total beside it already says it: both numbers were
              silently low, and a total that counts what it could not read
              as zero is a confident wrong answer. One "at least" covers
              both halves of the phrase, because both are short together --
              an unreadable repository takes its worktrees with it. */}
          <span className="text-sm font-semibold text-[#e6edf3]">
            {partial ? "at least " : ""}
            {worktrees.length} worktree{worktrees.length === 1 ? "" : "s"} across{" "}
            {repos.length} repositor{repos.length === 1 ? "y" : "ies"}
          </span>
          <span className="text-xs text-[#8b949e]">
            {/* "at least" while any size is still unmeasured: a total
                that silently counts unknowns as zero is a confident
                wrong answer. */}
            {sizesComplete ? "" : "at least "}
            {formatSize(totalBytes)}
          </span>
        </div>
        <PartialScanNotice
          unreadable={unreadable}
          consequence="the counts and the total above are floors."
        />
        {/* Sizes arrive one repository at a time, and this says how
            many are still outstanding.
            
            MEASURED: the full set takes ~2 minutes on a real machine
            (158 worktrees, `du` per worktree). Awaiting all of it
            showed dashes for that long with nothing to explain them.
            A count that visibly falls is the difference between "still
            working" and "broken". */}
        {sizesPending > 0 ? (
          <p className="border-b border-[#30363d] px-4 py-2 text-xs text-[#8b949e]">
            Measuring sizes — {sizesPending} of {sizesTotal} repositor
            {sizesTotal === 1 ? "y" : "ies"} still to go.
          </p>
        ) : null}
        {worktrees.map((wt) => (
          <button
            type="button"
            key={wt.path}
            onClick={() => setFilter("repo", wt.repoPath)}
            title="Open this repository to act on it"
            // On the phone the row wraps: repository and size on the
            // first line, the worktree's path on its own line beneath,
            // where it has the full width rather than what a fixed
            // 10rem repository column leaves over.
            className={
              isMobile
                ? "flex w-full flex-wrap items-baseline gap-x-3 gap-y-0.5 border-b border-[#30363d] px-4 py-2.5 text-left text-sm last:border-b-0 hover:bg-[#161b22]"
                : "flex w-full items-baseline gap-3 border-b border-[#30363d] px-4 py-2.5 text-left text-sm last:border-b-0 hover:bg-[#161b22]"
            }
          >
            <span
              className={
                isMobile
                  ? "min-w-0 flex-1 truncate text-[#8b949e]"
                  : "w-40 shrink-0 truncate text-[#8b949e]"
              }
            >
              {wt.repoName}
            </span>
            <span
              className={
                isMobile
                  ? "order-last min-w-0 basis-full truncate font-mono text-[#e6edf3]"
                  : "min-w-0 flex-1 truncate font-mono text-[#e6edf3]"
              }
            >
              {pathBasename(wt.path)}
            </span>
            <span className="w-20 shrink-0 text-right tabular-nums text-xs text-[#8b949e]">
              {/* The same three states as the per-repository cell. Once
                  the pass has finished, a null here is no longer "still
                  coming" -- it is a walk that was abandoned (#769), and
                  an em dash would read as "measured, and the answer is
                  nothing". The banner above covers the pending case, so
                  this only has to separate the other two. */}
              {wt.size_bytes === null && sizesPending === 0 ? (
                <span className="cursor-help text-[#6e7681]" title={UNMEASURED_HINT}>
                  not measured
                </span>
              ) : wt.size_bytes === null ? (
                "—"
              ) : (
                formatSize(wt.size_bytes)
              )}
            </span>
          </button>
        ))}
      </div>
    );
  }

  // How many rows would sort differently than they do, because
  // something they sort ON has landed since the order was fixed.
  //
  // This is what makes the frozen order HONEST rather than merely
  // stable: without it the user reads a "Largest first" list that is
  // quietly out of date and has no way to know. With it, the page says
  // so and offers the one click that fixes it.
  //
  // TWO sources, not one (#817). It used to count sizes alone, on the
  // reasonable-sounding grounds that sizes are the thing that streams.
  // But `sortWorktrees` ranks assessed rows above unassessed ones
  // before it looks at any axis, so marking a worktree assessed ALSO
  // invalidates the displayed order -- and on a name sort, where sizes
  // are irrelevant, it was the only thing that could. The page
  // therefore went quiet in exactly the case where it had nothing else
  // to notice.
  //
  // Size staleness stays gated on a size sort: a name sort does not
  // read `size_bytes` at all, so a measurement landing under it changes
  // nothing and a button offering to re-apply it would be noise.
  // Assessment staleness is NOT gated, because the assessed-first rule
  // applies to every sort.
  //
  // `last_commit` needs no clause here: it arrives with the row rather
  // than streaming in afterwards, so it cannot land after a snapshot.
  const sizeSorted = sort === "size-asc" || sort === "size-desc";
  const restaleCount = withSizes.filter(
    (w) =>
      (sizeSorted &&
        w.size_bytes !== null &&
        w.size_bytes !== undefined &&
        !snapshot.measured.has(w.path)) ||
      (assessed.has(w.path) && !snapshot.assessedThen.has(w.path)),
  ).length;

  // Withheld unless classification actually SUCCEEDED. A failed pass
  // used to resolve as an empty success, so rows sat on "checking..."
  // forever while this read a confident "0 safe to remove".
  const safeKnown = !classifying && !classifyFailed;

  // Which worktree each live agent is working in (#1137).
  //
  // Gated on the Claude integration like every other reader of this
  // data: a user who has not enabled it is not asked to pay a poll for
  // a badge they did not ask for.
  // A worktree an agent is working in is NOT one-click removable, and
  // neither is one whose occupancy could not be established. "We could
  // not tell" is not "nothing is there" -- this page refuses anything it
  // cannot establish is safe, which is the rule the force path exists
  // to let a user override deliberately.
  /// The session working in a worktree, for the row's badge (#1137).
  ///
  /// Through `occupancy`, never the map directly: the path-matching rule
  /// lives in one place, so the badge and the refusal below cannot
  /// disagree about one worktree.
  const agentIn = (path: string) => {
    const o = occupancy(path, sessionList, occupiedBy);
    return o.kind === "occupied" ? o.session : null;
  };

  const removable = (w: Worktree) =>
    isSafe(w.safety) &&
    (!claudeOn || occupancy(w.path, sessionList, occupiedBy).kind === "free");

  const shownSafe = shown.filter(removable);
  const safeCount = shownSafe.length;
  /// Stale registrations, counted and labelled SEPARATELY from "safe to
  /// remove" (#793).
  ///
  /// Their own count on purpose, and the issue is explicit that this is
  /// the right shape: a prunable worktree is not unsafe, it is a
  /// different verb. `is_safe()` stays a two-variant allowlist, the
  /// Remove button stays disabled on these rows because there is no
  /// directory to remove, and the thing that was missing was never a
  /// wider gate -- it was an action and a number of its own. Folding
  /// them into the green count would claim 12 directories are
  /// recoverable disk when they are 12 dangling pointers.
  const shownPrunable = shown.filter((w) => w.safety.kind === "prunable");
  const prunableCount = shownPrunable.length;
  /// Locks whose named holder is provably gone (#792).
  ///
  /// `isDeadLock` narrows to `holder_running === false` -- see its own
  /// doc for why `null` must not count. These rows are the ones a bulk
  /// unlock may touch; every other locked row keeps the single-row
  /// confirmation, because a bulk action over claims that might be live
  /// is exactly what #753 refused to offer and was right to.
  const shownDeadLocks = shown.filter(isDeadLock);
  const deadLockCount = shownDeadLocks.length;
  // Same honesty rule as the all-repositories rollup: an unmeasured
  // size is null, and counting it as zero would report a confident
  // wrong total. Sizes arrive in their own pass after safety, so this
  // grows as results land rather than being computed once.
  const measured = shown.filter((w) => w.size_bytes !== null && w.size_bytes !== undefined);
  const totalBytes = measured.reduce((n, w) => n + (w.size_bytes ?? 0), 0);
  const sizesComplete = measured.length === shown.length;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-baseline gap-3 text-sm">
        <span className="font-semibold">{selected?.name}</span>
        <span className="text-[#8b949e]">
          {shown.length} worktree{shown.length === 1 ? "" : "s"}
        </span>
        {/* The stash stack, once per repository, because it IS once per
            repository (#1138).

            Stated with the shared-stack fact, which is the part that
            makes it actionable: an entry pushed in one worktree is
            listed from every other and SURVIVES `git worktree remove`
            on the tree that made it, and git records no attribution. So
            a user clearing worktrees here is looking at work that will
            outlive the directory it belongs to and reappear, unlabelled,
            in whatever tree they open next.

            Absent at zero and absent when unread -- `null` means the
            scan did not ask, not that the stack is empty (#846), and
            zero is the ordinary case that needs no sentence. */}
        {selected?.stash_entries ? (
          <span
            className="text-[#d29922]"
            title="The stash stack is shared by every worktree of this repository, and entries outlive the worktree that created them."
          >
            {selected.stash_entries} stash entr{selected.stash_entries === 1 ? "y" : "ies"} (shared
            by all worktrees)
          </span>
        ) : null}
        {/* How old these answers are, for the whole repository.

            Every verdict on the rows below is computed against refs on
            disk; the scan never fetches, on purpose. Saying nothing made
            a fortnight-old answer read like the present tense (#702).
            Silent under an HOUR since #815 -- not under a day, which was
            silent through the whole window the bug lives in -- because a
            caveat shown always is a caveat nobody reads.

            KEPT, now that the main checkout's row carries the same age
            (#788). It is not a duplicate: this qualifies every row
            including the MERGE verdicts, which have no per-row age note
            and are the reason #702 and #815 were filed, while the row's
            note qualifies the one claim -- "up to date with upstream" --
            that a reader takes at face value. Removing this would
            un-fix #702 to fix #788.

            `scannedAt`, not an implicit `Date.now()`. That call was here
            and was an impure read during render; it also let the number
            drift upward on a tab left open, so a repository looked
            staler the longer you looked at it. See `scannedAt`'s own
            doc. */}
        {refAge(selected?.fetched_at ?? null, scannedAt) === null ? null : (
          <span
            className="text-xs text-[#d29922]"
            title="Merge and upstream verdicts are computed from the refs on disk, which are only as current as the last fetch. Fetch on the main checkout's row refreshes them without moving a branch."
          >
            {refAge(selected?.fetched_at ?? null, scannedAt)}
          </span>
        )}
        {/* The count is withheld, not shown as a growing number: a
            "3 safe to remove" that climbs to 122 as rows resolve invites
            acting on a figure that was never the answer.

            The PROGRESS is shown, though, which is the half #830 was
            missing. "checking what is safe to remove…" said only that
            work was happening, so it read identically at second 2 and at
            minute 15 -- the user could not tell a pass that was moving
            from one that had stopped, which is the whole complaint. A
            remaining count falls as verdicts land and is the one signal
            that distinguishes the two.

            Phrased as a remainder ("12 to go") rather than "99 of 111",
            matching the size pass's own progress line on this page. */}
        {classifying ? (
          <span className="text-xs text-[#58a6ff]">
            checking what is safe to remove
            {classifyPending > 0 && classifyTotal > 0 ? ` — ${classifyPending} to go` : "…"}
          </span>
        ) : classifyFailed ? (
          <button
            type="button"
            onClick={() => void retryClassify()}
            className="text-xs text-[#f85149] hover:underline"
            title={errorMessage(classifyError)}
          >
            could not check what is safe — retry
          </button>
        ) : (
          <span className="inline-flex items-center text-xs text-[#3fb950]">
            {safeCount} safe to remove
            {/* This view deletes directories on the strength of a
                one-line verdict. The rules behind it run to several
                paragraphs and lived only in source comments. */}
            <HelpButton topic="worktree-safety" />
          </span>
        )}
        {/* Worktrees that ARRIVED without a usable verdict, counted
            separately from the ones still pending (#830).

            The distinction is the lesson `useAllWorktreeSizes` records
            about its own `failed`: "a caller that only watches `pending`
            sees the number fall to zero and concludes everything was
            measured." Here the same mistake is worse, because the figure
            beside it is a count of directories the user is invited to
            delete. A pass where 9 of 111 worktrees could not be
            classified finishes with `pending` at zero and a confident
            green "102 safe to remove", and nothing on screen would say
            that 9 rows were never answered.

            So it sits NEXT TO the green count rather than inside it, and
            it is amber rather than red: these are not failures of the
            app, they are questions git could not answer in the time
            allowed, and each row already says so in its own words. The
            number exists so the green count cannot be read as covering
            the whole repository.

            Silent at zero, like every other conditional count in this
            row -- a permanent "0 could not be checked" is furniture. */}
        {!classifying && !classifyFailed && classifyUnknown > 0 ? (
          <span
            className="text-xs text-[#d29922]"
            title="These worktrees were listed but could not be classified — git refused, or the check ran past its time limit. Each row says which. They are never counted as safe to remove."
          >
            {classifyUnknown} could not be checked
          </span>
        ) : null}
        {/* A SECOND count, still in its own words and its own number
            (#793) -- but no longer in its own colour (#814).

            "12 stale registrations" is not a subset of "safe to remove"
            and must not read as one: there is no directory behind these,
            so they are not disk to reclaim and the Remove button is
            rightly disabled on them. That part of #793 stands, and so does
            the separate count.

            What changed is the COLOUR. Grey beside a green "N safe to
            remove" made the second number read as a caveat on the first --
            a warning, or a leftover the green count had declined to
            vouch for. Both are clearable and only the verb differs, so the
            shade must not be what carries the difference; the word
            "clear" does instead. The issue was filed twice, which is the
            signal that a correct distinction had become an invisible one.

            Green does NOT claim Remove works here. It means "nothing is
            stopping you getting rid of this", which is as true of a
            dangling registration as of a merged worktree -- more so, since
            nothing can be lost. The button beside it names the verb, and
            `isSafe` still excludes the kind, so nothing about the action
            has widened.

            Silent at zero. Most repositories have none, and a permanent
            "0 stale registrations" would be furniture. */}
        {safeKnown && prunableCount > 0 ? (
          <span className="text-xs text-[#3fb950]">
            {prunableCount} stale registration{prunableCount === 1 ? "" : "s"} to clear
          </span>
        ) : null}
        {/* The action #793 found missing entirely. `git worktree prune`
            appeared in three prose comments in this repository and in no
            argument list anywhere, so the app diagnosed the condition,
            named the remedy, disabled the button that would be wrong,
            and sent the user to a terminal.

            ONE affordance over the repository rather than a button per
            row, because that is the shape of git's verb: `prune` takes
            no path and clears every stale registration. A per-row button
            would have cleared all 12 and said it cleared one.

            No confirmation behind it, unlike every other cleanup on this
            page -- see `pruning`'s own note. Styled as an ordinary
            action, not a destructive one: red here would put a dangling
            pointer in the same visual class as deleting a directory
            full of unpushed commits. */}
        {safeKnown && prunableCount > 0 ? (
          <button
            type="button"
            disabled={pruning}
            onClick={() => runPrune()}
            title="Run `git worktree prune` — clears registrations whose directory is already gone. Nothing on disk is deleted."
            className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#161b22] disabled:opacity-50"
          >
            {pruning
              ? "Pruning…"
              : `Prune ${prunableCount} stale registration${prunableCount === 1 ? "" : "s"}`}
          </button>
        ) : null}
        {/* Dead-holder locks, counted separately again and for the same
            reason (#792): this is not disk to reclaim, it is an obstacle
            that has stopped being one. Grey, matching these rows' new
            tone -- a lock nothing holds is bookkeeping, and amber would
            keep asking for the judgement that is no longer required. */}
        {safeKnown && deadLockCount > 0 ? (
          <span className="text-xs text-[#8b949e]">
            {deadLockCount} lock{deadLockCount === 1 ? "" : "s"} with no live holder
          </span>
        ) : null}
        {/* The low-friction route for the provably-dead case (#792).

            A BULK affordance because the condition is bulk: five on the
            reporting machine after one reboot, 20+ historically, and
            clicking through five separate confirmations that each say
            "the process it names is no longer running" adds no judgement
            — it only adds clicks to a decision already made.

            Still behind a dialog, unlike Prune, and the line between
            them is real: pruning clears a pointer to a directory that is
            gone, while this clears a claim on a directory that is still
            there. The dialog lists the paths, so the bulk action is
            reviewed once rather than not at all.

            Amber, like the single-row unlock item: reversible by `git
            worktree lock`, so not red, but not a plain action either. */}
        {safeKnown && deadLockCount > 0 ? (
          <button
            type="button"
            disabled={bulkUnlockBusy}
            onClick={() => setBulkUnlockOpen(true)}
            className="rounded border border-[#d29922]/40 px-2 py-0.5 text-xs text-[#d29922] hover:bg-[#d29922]/10 disabled:opacity-50"
          >
            {bulkUnlockBusy
              ? "Unlocking…"
              : `Unlock ${deadLockCount} abandoned lock${deadLockCount === 1 ? "" : "s"}`}
          </button>
        ) : null}
        {/* A COUNT, not a bare "measuring sizes…".

            The old label said only that work was happening, which is
            indistinguishable from a hang once it has said it for ten
            minutes -- and that is precisely what #754 reported. The walk
            is unbounded in wall-clock terms (MEASURED: 21.40s for a
            single 200 GB checkout), so the honest thing is not to
            promise a finish time but to show it advancing. A number that
            visibly falls is the difference between "still working" and
            "broken". */}
        {!classifying && sizing ? (
          <span className="text-xs text-[#8b949e]">
            measuring sizes — {shown.length - measured.length} of {shown.length} to go
          </span>
        ) : null}
        {/* "at least" until every worktree has been measured. Reported
            here as well as on the rollup, because the per-repo page is
            where you land after choosing a repo and could not answer
            "how much is this one holding?". */}
        {measured.length > 0 ? (
          <span className="text-xs text-[#8b949e]">
            {sizesComplete ? "" : "at least "}
            {formatSize(totalBytes)} total
          </span>
        ) : null}

        {/* The three columns the row already renders, both ways (#771).
            A `<select>` rather than clickable column headers: this list
            is not a table -- the safety verdict is prose that wraps, and
            there are no headers to click. `ArtifactsPage` reached the
            same shape for the same reason, so this matches it rather
            than inventing a third pattern. */}
        {/* The count is in the label, so the scope is legible before
            clicking rather than only in the dialog. 106 of 268 worktrees
            are safe on a real machine, mostly in a few repos -- clicking
            those one at a time adds no safety, only clicks.

            BEFORE the Sort group, and that order is the rest of #817.
            See the group's own comment below for why. */}
        {safeCount > 1 && safeKnown ? (
          <button
            type="button"
            disabled={bulkBusy}
            onClick={() => setBulkOpen(true)}
            className="rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10 disabled:opacity-50"
          >
            {/* A count, not a spinner: ~100 worktrees is around 30
                seconds of sequential deletion, and a bare "Removing…"
                for that long is indistinguishable from a hang. */}
            {bulkBusy
              ? removalProgress
                ? `Removed ${removalProgress.done} of ${removalProgress.total}…`
                : "Removing…"
              : `Remove ${safeCount} safe worktree${safeCount === 1 ? "" : "s"}`}
          </button>
        ) : null}
        {/* Beside the button rather than in the dialog: the question
            ("can I leave this page?") occurs while it is running, which
            is when the dialog is already gone. */}
        {safeCount > 1 && safeKnown ? <HelpButton topic="bulk-removal" /> : null}

        {/* ONE GROUP, placed AFTER the bulk Remove button. Both halves
            are the fix for #817.

            The re-sort button used to sit directly before the bulk
            "Remove N safe worktrees" button in this `flex flex-wrap`
            row. So an ADVISORY control appearing -- which it does on its
            own schedule, as measurements and assessments land -- pushed a
            DESTRUCTIVE control sideways or onto a second line. That is
            the same hazard the frozen row order exists to prevent,
            reproduced one level up in the toolbar: a thing that deletes
            directories must not move because something else arrived.

            Grouping it with the Sort select fixed the ADJACENCY, and it
            is where the button belongs -- it does nothing but re-run what
            the select chose. But grouping alone did not fix the
            DISPLACEMENT, which is what the reporter actually asked for
            ("it shouldn't displace everything"): the group is still a
            flex item in the same wrapping row, and it still sits where it
            sat, so a button appearing inside it widens the group and
            pushes everything after it along. The structural test that
            shipped with it asserted only that the two buttons are not
            siblings, which is true and insufficient.

            Ordering is what actually settles it. With the group after
            Remove, nothing upstream of Remove changes width when the
            button appears, disappears, or changes its count -- so Remove
            cannot move, by construction rather than by tuning. That is
            stronger than reserving a fixed slot, the other option the
            issue offered: a reserved slot holds a permanent gap on a
            toolbar that is usually complete without it, and it only
            stops the two from moving rather than stopping them from being
            neighbours.

            The cost is that Sort is no longer the last control before
            "All repositories". Worth it: Sort is advisory and idempotent,
            Remove deletes directories, and when only one of them can hold
            a stable position it is not the advisory one.

            `shrink-0` so the group is not what the toolbar compresses,
            and no `flex-wrap` inside it: the button belongs to the
            select, and a wrap between them would read as two unrelated
            controls. */}
        <span className="flex shrink-0 items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-[#8b949e]">
            Sort
            <select
              value={sort}
              onChange={(e) => chooseSort(e.target.value as WorktreeSort)}
              aria-label="Sort worktrees"
              className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 text-xs text-[#e6edf3]"
            >
              {(Object.keys(WORKTREE_SORT_LABELS) as WorktreeSort[]).map((k) => (
                <option key={k} value={k}>
                  {WORKTREE_SORT_LABELS[k]}
                </option>
              ))}
            </select>
          </label>

          {/* The explicit gesture that makes the frozen order honest, and
              it STAYS -- the reporter settled that: "the button is useful
              for showing that re-calc is being performed, but it
              shouldn't displace everything". It is a progress indicator,
              not unwanted UX, so auto-sorting is off the table and the
              `:734-765` safety rationale is not under pressure.

              Rows are ordered on what was known when the sort was
              chosen, so a measurement or an assessment landing afterwards
              does not move anything under the cursor -- but it would
              leave a "Largest first" list quietly out of date with no way
              to tell. This says how many rows have changed since, and one
              click applies them. Silent when there is nothing to apply,
              so it is not a permanent piece of furniture.

              "out of date" rather than the old "newly measured": the
              count now includes assessments, which are not measurements,
              and a label naming only one of its two causes would
              misreport the other. */}
          {restaleCount > 0 ? (
            <button
              type="button"
              onClick={() => setSortedAt((n) => n + 1)}
              aria-live="polite"
              title="Re-apply the chosen sort, using the sizes and assessments that have landed since"
              className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#58a6ff] hover:bg-[#161b22]"
            >
              re-sort — {restaleCount} out of date
            </button>
          ) : null}
        </span>

        <button
          type="button"
          onClick={() => setFilter("repo", undefined)}
          className="ml-auto text-xs text-[#8b949e] hover:text-[#e6edf3]"
        >
          All repositories
        </button>
      </div>

      {/* Per worktree, not bulk. Bulk-deleting directories is where a
          wrong predicate becomes unrecoverable at scale, and with 149
          removable worktrees on one repo the temptation is real. */}
      {claudifying !== null ? (
        <Dialog open onOpenChange={() => setClaudifying(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Assess {pathBasename(claudifying.worktree.path)}</DialogTitle>
            <ActingOnDesktop />
            <p className="text-sm text-[#8b949e]">
              {claudifying.claudeInstalled
                ? "Run this on that desktop to have Claude Code assess what removing this worktree would lose."
                : "Run this on that desktop. Claude Code was not found there, so it may need installing first."}
            </p>
            {/* Selectable, and wrapped rather than truncated: the point
                is that a person can read and retype it. No copy button
                -- `copyText` is exactly what does not work here, and
                offering one that fails is worse than offering none. */}
            <pre className="max-h-48 overflow-auto rounded border border-[#30363d] bg-[#0d1117] p-3 text-xs break-all whitespace-pre-wrap select-text">
              {claudifying.command}
            </pre>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                className="min-h-11"
                onClick={() => {
                  const wt = claudifying.worktree;
                  setClaudifying(null);
                  assessmentAction(wt).onClick();
                }}
              >
                I read the assessment
              </Button>
              <Button variant="ghost" className="min-h-11" onClick={() => setClaudifying(null)}>
                Close
              </Button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
      {pending ? (
        <ConfirmRemove
          wt={pending}
          onCancel={() => setPending(null)}
          onConfirm={() => {
            const target = pending;
            const name = pathBasename(target.path);
            setPending(null);
            setRemoving(target.path);
            remove(selected?.path ?? "", target.path).then(
              () => {
                setRemoving(null);
                toast.success(`Removed ${name}`);
              },
              // The backend re-checks safety at delete time, so a
              // worktree that went dirty since the scan is refused. That
              // message is the useful part -- show it, do not summarise.
              (e: unknown) => {
                // Back to normal, not stuck on "Removing...": the backend
                // re-checks safety at delete time and legitimately
                // refuses a worktree that went dirty since the scan.
                setRemoving(null);
                toast.error(`Could not remove ${name}`, {
                  description: typeof e === "string" ? e : undefined,
                });
              },
            );
          }}
        />
      ) : null}

      {/* The orphan confirmation (#845). The SAME element the Orphaned
          section renders -- see `orphanDialog` for why it is one value
          rather than two copies. */}
      {orphanDialog}

      {forcing ? (
        <Dialog open onOpenChange={(o) => !o && setForcing(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Remove {pathBasename(forcing.path)}?</DialogTitle>
            <ActingOnDesktop />
            <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">{forcing.path}</p>
            {/* The specific loss, computed now -- "are you sure?" is not
                something anyone can act on, and this is the only
                genuinely unrecoverable case in the app. */}
            <p className="mt-3 text-sm text-[#e6edf3]">
              {safetyReason(forcing.safety)}
              {forcing.upstream && upstreamShort(forcing.upstream)
                ? ` · ${upstreamShort(forcing.upstream)}`
                : ""}
              {forcing.last_commit ? ` · last commit ${relativeTime(forcing.last_commit)}` : ""}
            </p>
            {/* An empty branch is the one case here where nothing is at
                risk, so it gets neither the red nor the warning. Saying
                "these commits are not pushed anywhere" over a branch
                with no commits is the exact misreport of #701, and
                repeating it in the confirmation is worse than in the
                row: this is the moment the user decides. */}
            <p
              className={`mt-2 text-sm ${
                forcing.safety.kind === "empty" ? "text-[#8b949e]" : "text-[#f85149]"
              }`}
            >
              {forceWarning(forcing.safety)}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setForcing(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const target = forcing;
                  const name = pathBasename(target.path);
                  setForcing(null);
                  setRemoving(target.path);
                  forceRemove(selected?.path ?? "", target.path).then(
                    () => {
                      setRemoving(null);
                      toast.success(`Removed ${name}`);
                    },
                    (e: unknown) => {
                      setRemoving(null);
                      toast.error(`Could not remove ${name}`, {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                I have reviewed this — remove it
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {/* The unlock confirmation (#775).

          #753 declined an unlock action on the grounds that a button
          beside a row invites clearing a claim without reading it. This
          dialog is the answer to that objection rather than a way
          around it: it exists to make the user LOOK at what they are
          clearing, so it leads with the holder and the age and says
          what is underneath before offering the button.

          Deliberately NOT styled as a destructive confirmation. Nothing
          is deleted, `git worktree lock` puts it back, and dressing a
          reversible action in the red of the one unrecoverable action
          on this page would teach the user to discount both. The
          caution here is a different one -- another process may be
          using that directory -- and it is stated in those words. */}
      {unlocking && unlocking.safety.kind === "locked" ? (
        <Dialog open onOpenChange={(o) => !o && setUnlocking(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Unlock {pathBasename(unlocking.path)}?</DialogTitle>
            <ActingOnDesktop />
            <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">
              {unlocking.path}
            </p>

            {/* WHO and WHEN, as two separate lines, because they are
                two separate pieces of evidence and one of them is much
                better than the other.

                The age leads. It is the only part that differs per row
                and that a long-lived parent process cannot fake --
                measured, the locks on the reporting machine span four
                days while every one of them names the same pid. */}
            <p className="mt-3 text-sm text-[#e6edf3]">
              Locked {lockAge(unlocking.safety.detail) ?? "at an unknown time"}
              {unlocking.safety.detail.reason === null ? (
                <>, with no reason given.</>
              ) : (
                <>
                  {" "}
                  by{" "}
                  <span className="font-mono text-xs">
                    {unlocking.safety.detail.reason}
                  </span>
                </>
              )}
            </p>

            {/* What checking the pid actually established -- phrased so
                a running process cannot be mistaken for proof that the
                lock is live. On the reporting machine this reads
                "still running" for all 20 locks and every one of them
                is abandoned, which is exactly why the sentence carries
                its own caveat. */}
            {lockHolderNote(unlocking.safety.detail) ? (
              <p className="mt-2 text-sm text-[#8b949e]">
                {lockHolderNote(unlocking.safety.detail)}
              </p>
            ) : null}

            {/* What is UNDERNEATH -- the thing #753 could not tell the
                user, and the reason unlocking was a blind action. */}
            <p className="mt-2 text-sm text-[#8b949e]">
              Underneath the lock, this worktree is{" "}
              <span className="text-[#e6edf3]">
                {safetyReason(unlocking.safety.detail.underlying)}
              </span>
              .
            </p>

            {/* The honest caution. Not "this cannot be undone" -- it
                plainly can -- but the risk that is real: the lock is
                how another process says it is using this directory. */}
            <p className="mt-3 text-sm text-[#d29922]">
              Unlocking deletes nothing and can be undone by locking it
              again. It does clear the only signal another process has for
              saying it is working here — so check the holder above before
              clearing it.
            </p>

            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setUnlocking(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const target = unlocking;
                  const name = pathBasename(target.path);
                  setUnlocking(null);
                  unlock(selected?.path ?? "", target.path).then(
                    () =>
                      toast.success(`Unlocked ${name}`, {
                        // Says what changed and what did NOT: the row
                        // is about to re-classify, and the user should
                        // not read a successful unlock as a removal.
                        description:
                          "The worktree is still there — its safety verdict will refresh.",
                      }),
                    (e: unknown) =>
                      toast.error(`Could not unlock ${name}`, {
                        description: typeof e === "string" ? e : undefined,
                      }),
                  );
                }}
                className="rounded border border-[#d29922]/40 px-3 py-1.5 text-sm font-medium text-[#d29922] hover:bg-[#d29922]/10"
              >
                Unlock it
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {/* The bulk unlock confirmation (#792).

          Narrower than it looks: only rows where `holder_running` is
          explicitly false get here, so every line in the list below has
          a named process that has been checked and is not running. A
          lock whose holder is alive, or whose reason named nobody to
          check, is excluded and keeps the single-row dialog -- a bulk
          action over claims that MIGHT be live is exactly what #753
          refused to offer, and nothing here reopens that.

          So the dialog's job is narrower than the single-row one's. That
          one exists to make the user read evidence and weigh it; this
          one states the evidence is already decisive and shows the
          scope, because the remaining question is "which directories"
          rather than "should I". Hence the path list and no per-row
          holder prose.

          Deliberately NOT styled as destructive, for the reason the
          single-row dialog gives: nothing is deleted and `git worktree
          lock` puts it back. */}
      {bulkUnlockOpen ? (
        <Dialog open onOpenChange={(o) => !o && setBulkUnlockOpen(false)}>
          <DialogContent className="max-w-2xl">
            <DialogTitle>
              Unlock {deadLockCount} abandoned lock{deadLockCount === 1 ? "" : "s"}?
            </DialogTitle>
            <ActingOnDesktop />
            <p className="mt-2 text-sm text-[#8b949e]">
              Each of these locks names a process, and each of those processes is
              no longer running — so nothing is working in these directories.
              Unlocking deletes nothing and can be undone by locking them again.
            </p>
            {/* What the unlock does NOT do, said before the button
                rather than only in the toast afterwards. On a page where
                the adjacent bulk button deletes directories, a user
                clicking this one deserves to know the rows will still be
                there. */}
            <p className="mt-2 text-sm text-[#8b949e]">
              The worktrees stay where they are. Their safety verdicts are
              re-checked afterwards, and each is removable only if it earns that
              on its own.
            </p>
            {/* Every path. These are claims on real directories, and a
                count alone would make the scope unreviewable. */}
            <ul className="mt-3 max-h-64 overflow-y-auto font-mono text-xs text-[#8b949e]">
              {shownDeadLocks.map((w) => (
                <li key={w.path} className="py-0.5">
                  {w.path}
                </li>
              ))}
            </ul>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setBulkUnlockOpen(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const targets = shownDeadLocks.map((w) => w.path);
                  // Closed NOW, like the bulk removal and for the same
                  // reason: the work does not need the dialog, progress
                  // is on the toolbar button, and twenty sequential
                  // unlocks should not hold the app.
                  setBulkUnlockBusy(true);
                  setBulkUnlockOpen(false);
                  unlockMany(selected?.path ?? "", targets).then(
                    (outcomes) => {
                      setBulkUnlockBusy(false);
                      const failed = outcomes.filter((o) => o.error !== null);
                      const ok = outcomes.length - failed.length;
                      // Never a bare "done". A row somebody else
                      // unlocked between the scan and the click is an
                      // ordinary race, and hiding the refusals would
                      // misreport which locks are still in place.
                      if (failed.length === 0) {
                        toast.success(`Unlocked ${ok} worktree${ok === 1 ? "" : "s"}`, {
                          description:
                            "The worktrees are still there — their safety verdicts will refresh.",
                        });
                      } else {
                        toast.error(
                          `${failed.length} of ${outcomes.length} could not be unlocked`,
                          {
                            description: failed
                              .map((f) => `${pathBasename(f.path)}: ${f.error}`)
                              .join("\n"),
                          },
                        );
                      }
                    },
                    (e: unknown) => {
                      setBulkUnlockBusy(false);
                      if (isCancelled(e)) return;
                      toast.error("The bulk unlock could not run", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                disabled={bulkUnlockBusy}
                className="rounded border border-[#d29922]/40 px-3 py-1.5 text-sm font-medium text-[#d29922] hover:bg-[#d29922]/10 disabled:opacity-50"
              >
                {bulkUnlockBusy
                  ? "Unlocking…"
                  : `Unlock ${deadLockCount} lock${deadLockCount === 1 ? "" : "s"}`}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {bulkOpen ? (
        <Dialog open onOpenChange={(o) => !o && setBulkOpen(false)}>
          <DialogContent className="max-w-2xl">
            <DialogTitle>
              Remove {safeCount} safe worktree{safeCount === 1 ? "" : "s"}?
            </DialogTitle>
            <ActingOnDesktop />
            <p className="mt-2 text-sm text-[#8b949e]">
              {/* Sizes are already computed, and reclaimed space is the
                  number that makes this decision -- it is why the view
                  exists. */}
              {/* "Still measuring" and "nothing to reclaim" are
                  different answers and used to share one dash. */}
              {(() => {
                const total = totalSize(shown.filter(removable));
                return total === null
                  ? "Sizes are still being measured."
                  : `Reclaims ${formatSize(total)}.`;
              })()}{" "}
              Each is re-checked before deletion, so anything that changed
              since the scan is skipped.
            </p>
            {/* The third system this page could never reach. Docker
                images built from these worktrees outlive them, and
                until now removing them meant going to another view and
                working out by hand which ones belonged to what.

                Named, not just counted, and listed separately from the
                paths: this is a SECOND irreversible action in a dialog
                that used to perform one, and folding it in silently
                would be exactly the unreviewed bulk delete the manifest
                exists to prevent. */}
            {/* Every path, not a count: these are directories on disk. */}
            <ul className="mt-3 max-h-64 overflow-y-auto font-mono text-xs text-[#8b949e]">
              {shownSafe.map((w) => (
                <li key={w.path} className="py-0.5">
                  {w.path}
                </li>
              ))}
            </ul>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setBulkOpen(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const targets = shownSafe.map((w) => w.path);
                  // Closed NOW, not when the batch finishes.
                  //
                  // It used to close in `.then()`, so the modal sat over
                  // the app for the whole removal -- around 30 seconds
                  // for ~100 worktrees -- and nothing else could be
                  // used. The work does not need the dialog: progress
                  // is already on the toolbar button ("Removed 12 of
                  // 40…"), the outcomes arrive as a toast, and the
                  // promise runs to completion regardless of what is on
                  // screen. So the user asked for a deletion and gets
                  // their app back.
                  setBulkBusy(true);
                  setBulkOpen(false);
                  removeMany(selected?.path ?? "", targets).then(
                    (outcomes) => {
                      setBulkBusy(false);
                      const failed = outcomes.filter((o) => o.error !== null);
                      const ok = outcomes.length - failed.length;
                      // Never a bare "done": a worktree that went dirty
                      // since the scan is refused, and hiding that would
                      // misreport what is still on disk.
                      if (failed.length === 0) {
                        toast.success(`Removed ${ok} worktree${ok === 1 ? "" : "s"}`);
                      } else {
                        toast.error(`${failed.length} of ${outcomes.length} could not be removed`, {
                          description: failed
                            .map((f) => `${pathBasename(f.path)}: ${f.error}`)
                            .join("\n"),
                        });
                      }
                    },
                    (e: unknown) => {
                      setBulkBusy(false);
                      // Silent when the user dismissed the biometric
                      // prompt: they declined, nothing was removed, and
                      // telling them the action "could not run" reports
                      // their own decision back as a failure.
                      if (isCancelled(e)) return;
                      toast.error("The bulk removal could not run", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                disabled={bulkBusy}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37] disabled:opacity-50"
              >
                {/* `bulkBusy` existed and nothing read it, so a slow
                    removal showed an unchanged button and a live count
                    ticking down behind the modal. */}
                {bulkBusy
                  ? "Removing…"
                  : `Remove ${safeCount} worktree${safeCount === 1 ? "" : "s"}`}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {/* `overflow-hidden` is a BACKSTOP, not the fix (#818).

          The fix is that every cell in a row can now yield; this is what
          happens if some future cell cannot. Without it the border is
          decoration that content walks straight through: the row that
          reported this issue rendered its Remove button and kebab
          outside the box entirely, to the right of the border, where
          they looked like they belonged to nothing. With it, an
          unbounded cell degrades to being clipped at the edge -- still
          wrong, but visibly wrong inside the frame instead of silently
          scattering controls across the page.

          Clipping rather than `overflow-x-auto`: a horizontal
          scrollbar on the list would mean the action buttons can be
          scrolled OUT of view, and the whole complaint was that they
          were unreachable. `rounded-md` also only actually rounds the
          first and last rows' corners once the box clips them. */}
      <div className="overflow-hidden rounded-md border border-[#30363d]">
        {shown.length === 0 ? (
          <div className="px-4 py-12 text-center text-sm text-[#8b949e]">
            No worktrees in this repository.
          </div>
        ) : (
          shown.map((wt) => (
            <Row
              key={wt.path}
              wt={wt}
              agent={agentIn(wt.path)}
              repoPath={selected?.path ?? ""}
              pr={prForWorktree(prs, selected?.identity ?? null, wt.branch)}
              assessed={assessed.has(wt.path)}
              onForce={setForcing}
              onUnlock={setUnlocking}
              onRemoveOrphan={setPendingOrphan}
              onPull={runPull}
              pulling={pullingPath === wt.path}
              onFetch={runFetch}
              fetching={fetchingPath === wt.path}
              // `?? null`, not `??  ""` or a fallback date. The Rust side
              // always sends the key, but a repository that has never
              // fetched sends null -- and null must stay null all the way
              // to `refStaleness`, which reports it as `unknown` rather
              // than as an age of zero.
              fetchedAt={selected?.fetched_at ?? null}
              scannedAt={scannedAt}
              onRemove={setPending}
              onClaudify={claudify}
              onForget={forget}
              sizePending={sizing}
              sizeUnmeasurable={wt.sizeUnmeasurable}
              removing={removing === wt.path}
            />
          ))
        )}
      </div>
    </div>
  );
}
