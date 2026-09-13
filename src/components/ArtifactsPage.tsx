import { isCancelled } from "@/lib/cancelled";
import { ActingOnDesktop } from "./ActingOnDesktop";
import { useMemo, useState } from "react";
import { HardDrive } from "lucide-react";
import type { Artifact, ArtifactKind } from "@/types/pr";
import { useArtifacts, useArtifactSizes, useRemoveArtifacts, useVenvs } from "@/api/hooks";
import { useActiveFilters } from "@/store/filters";
import { relativeSeconds } from "@/lib/time";
import { useIsMobile } from "@/lib/useIsMobile";
import { diagMark } from "@/api/diag";
import { GROUP_LABEL, VENV_GROUP } from "./ArtifactSidebar";
import { toast } from "sonner";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { formatSize } from "@/lib/worktrees";
import { HelpButton } from "./HelpButton";
import { QueryError, errorMessage } from "./QueryError";
import { VenvSection } from "./VenvSection";
import { CleanupLog } from "./CleanupLog";

/// A placeholder holding the same footprint as the number it stands in
/// for, so rows do not jump as each measurement lands. Matches the one
/// on the worktree page rather than importing it -- that one is local to
/// its module, and a shared component for six lines of markup would be
/// indirection without benefit.
function Skeleton({ className = "" }: { className?: string }) {
  return (
    <span
      // `motion-safe:` and `aria-hidden` for the same reasons the
      // worktree page's version carries them: an animation on every row
      // is what that setting exists to stop, and a placeholder has
      // nothing to announce.
      aria-hidden="true"
      className={`inline-block h-3 rounded bg-[#30363d] align-middle motion-safe:animate-pulse ${className}`}
    />
  );
}

/// What rebuilds each kind, shown beside the row.
///
/// "You can delete this" is only actionable next to what puts it back,
/// and the command is the whole safety argument in one phrase: removal
/// costs a rebuild, not work.
const REBUILD: Record<ArtifactKind, string> = {
  cargo_target: "cargo build",
  node_modules: "npm install",
  terraform: "terraform init",
  dotnet_build: "dotnet build",
  build_output: "the project's build",
};

const LABEL: Record<ArtifactKind, string> = {
  cargo_target: "target",
  node_modules: "node_modules",
  terraform: ".terraform",
  dotnet_build: "bin / obj",
  build_output: "build output",
};

/// Recently-written directories are probably being built into right now.
///
/// A running `cargo build` does NOT make git dirty -- build output is
/// gitignored -- so no git-based check can see it. Directory mtime is
/// the only available signal, which is why this is surfaced rather than
/// silently folded into a safety verdict.
///
/// FIFTEEN minutes, matching `ACTIVE_WINDOW_SECS` in
/// `src-tauri/src/artifacts/mod.rs` -- the value that actually decides
/// whether a delete succeeds -- and `cleanup.rs`' copy of the same rule.
/// This said `60 * 60` until #850. The two sides disagreeing was
/// user-visible in three places at once: a `target/` last written 20
/// minutes ago was one the backend WOULD remove (20 > 15), but the UI
/// excluded it from `removable`, so the Remove button under-counted,
/// the reclaimable-space figure under-reported, and `selectedActive`
/// warned "something is building here" about a directory the backend
/// did not consider active.
///
/// Fifteen rather than sixty because the Rust comment chose it
/// deliberately against an hour: long enough to cover a build's quiet
/// phases (linking a large binary writes nothing for minutes), short
/// enough that yesterday's work is not still blocked today. The UI
/// silently having the hour was drift, not a second opinion.
///
/// Exported so `src/lib/mirroredConstants.test.ts` can assert it against
/// the Rust literal it mirrors, which it reads via Vite's `?raw`. Two
/// constants in two languages cannot be one declaration, so the
/// agreement has to be a test that reads BOTH sides -- which is what the
/// comment below claiming the backend enforces the same rule was
/// asserting on nothing but good intentions.
export const ACTIVE_SECS = 15 * 60;

/// Regenerable build output across the scanned directories.
///
/// A separate view from Worktrees despite scanning the same roots,
/// because it answers a different question. Measured on the machine that
/// prompted this: 0.28 GB of Rust build output sat inside worktrees,
/// against 108 GB beside main checkouts -- so the worktree view
/// structurally could not reach 99.7% of the largest thing on the disk.
export function ArtifactsPage() {
  const filters = useActiveFilters();
  const isMobile = useIsMobile();
  // `repo` is the sidebar's selection key across every view; here it
  // holds an artifact KIND rather than a path. Reusing it keeps one
  // selection mechanism instead of a second parallel one.
  const group = filters.repo;
  // `isError`, `error` and `refetch` as well as the data (#846).
  //
  // The `= []` default made a REJECTED scan indistinguishable from an
  // empty machine: the page fell through to "No {label} found in the
  // scanned directories", which on a disk-cleanup tool reads as "your
  // machine is clean" when the truth is it could not look. Nobody
  // investigates good news, and there was nothing red and no retry, so
  // the failure was not merely unreported -- it was actively reassuring.
  // `QueryError`'s own doc comment diagnoses exactly this: "An error has
  // to look like an error."
  const {
    data: allArtifacts = [],
    isLoading,
    isError,
    error,
    refetch,
  } = useArtifacts(true);
  // Read here only to decide the empty state; VenvSection owns the rest.
  const { data: venvList = [] } = useVenvs(true);
  const venvCount = venvList.length;
  // Filtered BEFORE sizing, so a group page measures only what it shows
  // rather than paying for the whole machine to render one section.
  const artifacts =
    group === undefined || group === VENV_GROUP
      ? allArtifacts
      : allArtifacts.filter((a) => a.kind === group);
  const { sizes, ages, pending, total, failed } = useArtifactSizes(artifacts, artifacts.length > 0);
  /// Whether the byte figures on this page are the whole answer (#956).
  ///
  /// `pending > 0` alone was the test, and a REJECTED batch satisfies it:
  /// a failed query is neither fetching nor holding data, so `pending`
  /// falls to zero with measurements still missing. Every honesty
  /// qualifier on this page hung off that one number and therefore failed
  /// open -- the total lost its "at least", and a destructive button
  /// gained a byte count summed over a map that is missing the failed
  /// batch's paths.
  ///
  /// A failure is PERMANENT where pending is temporary (`retry: false`,
  /// #846), so this does not clear on its own. That is why the copy below
  /// says WHY the number is a floor rather than only that it is one.
  const sizesComplete = pending === 0 && failed === 0;
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [sort, setSort] = useState<"size" | "age">("size");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const remove = useRemoveArtifacts();

  const toggle = (path: string) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  // Largest or oldest first, and UNMEASURED rows sort last rather than
  // as zero. Sorting a null as 0 would bury the biggest directory on the
  // machine at the bottom until its size happened to arrive -- the
  // ordering bug #360 describes on the worktree page. The same rule
  // applies to age, where an unknown must not read as "brand new".
  const rows = useMemo(() => {
    const key = (p: string) => (sort === "size" ? sizes.get(p) : ages.get(p));
    return [...artifacts].sort((a, b) => {
      const va = key(a.path);
      const vb = key(b.path);
      if (va === undefined && vb === undefined) return a.path.localeCompare(b.path);
      if (va === undefined) return 1;
      if (vb === undefined) return -1;
      // Both descending: biggest first, and oldest first -- a larger
      // seconds-ago IS older.
      return vb - va;
    });
  }, [artifacts, sizes, ages, sort]);

  const selectedBytes = [...checked].reduce((n, p) => n + (sizes.get(p) ?? 0), 0);
  const selectedActive = [...checked].filter((p) => {
    const age = ages.get(p);
    return age !== undefined && age < ACTIVE_SECS;
  }).length;

  // Everything a build is NOT currently writing to. The same rule the
  // backend enforces at delete time, applied here so the button's count
  // matches what the click will actually remove -- and the agreement is
  // now ASSERTED (`src/lib/mirroredConstants.test.ts` reads the Rust
  // literal), because this comment was true of the intent and false of
  // the code for as long as ACTIVE_SECS was an hour (#850).
  //
  // An UNKNOWN age counts as removable here, which is deliberately the
  // opposite of the backend's delete gate: the backend refuses what it
  // could not measure (#841), so such a row is offered, attempted, and
  // refused with a reason the user can read. Excluding it instead would
  // hide the largest directories on an unmeasurable volume from the page
  // entirely, with nothing said.
  const removable = rows.filter((r) => {
    const age = ages.get(r.path);
    return age === undefined || age >= ACTIVE_SECS;
  });
  const removableBytes = removable.reduce((n, r) => n + (sizes.get(r.path) ?? 0), 0);

  const measured = rows.filter((r) => sizes.has(r.path));
  const totalBytes = measured.reduce((n, r) => n + (sizes.get(r.path) ?? 0), 0);

  // On the virtualenv page the build-artifact list is not the subject.
  const showArtifacts = group !== VENV_GROUP;
  const showVenvs = group === undefined || group === VENV_GROUP;

  if (isLoading && showArtifacts) {
    return <p className="p-4 text-sm text-[#8b949e]">Looking for build output…</p>;
  }

  /// The failed artifact scan, as a panel rather than a page (#846).
  ///
  /// A `const` mounted inside the layout below, NOT an early return. An
  /// early return would take the virtualenv section with it, and on
  /// "Everything" that section can be perfectly healthy -- hiding 78
  /// removable virtualenvs behind a failure about build output would
  /// replace one silent loss with another. The page's own empty state
  /// already reasons this way: "an empty artifact list beside 78
  /// virtualenvs is not an empty page".
  ///
  /// Gated on `showArtifacts` for the same reason the loading arm is: on
  /// the virtualenv group page the artifact scan is not the subject.
  const artifactsError =
    isError && showArtifacts ? (
      <QueryError
        title="Could not scan for build output"
        message={errorMessage(error)}
        onRetry={() => void refetch()}
      >
        {/* What the failure actually COSTS, which the message cannot say:
            this page's whole claim is a total, and a scan that refused
            has no total -- not a zero. Stated rather than left implicit,
            because "could not scan" invites reading the last number the
            user saw as still true. */}
        <p className="mx-auto mt-2 max-w-lg text-sm text-[#8b949e]">
          Nothing was measured, so there is no total — this is not a report that
          your directories are clean.
        </p>
      </QueryError>
    ) : null;

  // Named by the group the user chose, so an empty Terraform page does
  // not claim there is no build output at all.
  const label =
    group !== undefined && group in GROUP_LABEL
      ? GROUP_LABEL[group as keyof typeof GROUP_LABEL].toLowerCase()
      : "build output";

  // Only when there is genuinely nothing on the page. On "Everything"
  // that means no artifacts AND no virtualenvs -- an empty artifact list
  // beside 78 virtualenvs is not an empty page, and saying so would be
  // wrong in the one place the user is looking for the total.
  //
  // And only when the scan actually ANSWERED (#846). `artifacts.length
  // === 0` is true on a rejection too -- `data` is left at its `[]`
  // default -- so without the `!isError` guard this branch was reached
  // first and reported a clean machine. It is the branch the whole issue
  // is about, and `artifactsError` below is unreachable without this.
  if (!isError && artifacts.length === 0 && (!showVenvs || venvCount === 0)) {
    return (
      <p className="p-4 text-sm text-[#8b949e]">
        No {label} found in the scanned directories.
      </p>
    );
  }

  return (
    <div className="p-4">
      {/* IN PLACE OF the toolbar and the list, not above them (#846).

          A failed scan has no count, no total and no rows, so rendering
          "0 directories · 0 B" beside an error would state two things at
          once and let the eye take the reassuring one. The virtualenv
          section below is untouched and still renders, which is the whole
          reason this is a panel rather than an early return. */}
      {artifactsError}
      {showArtifacts && !isError ? (
      // Wraps on the phone: six items on one 390px line broke "3
      // directories" and "4.8 GB" across lines mid-phrase.
      <div className={isMobile ? "mb-3 flex flex-wrap items-center gap-2 text-sm" : "mb-3 flex items-center gap-2 text-sm"}>
        <HardDrive className="h-4 w-4 shrink-0 text-[#8b949e]" aria-hidden="true" />
        <span className="font-semibold text-[#e6edf3]">
          {artifacts.length} director{artifacts.length === 1 ? "y" : "ies"}
        </span>
        {/* "at least" until every batch has answered, because a total
            over a partial set is not the total. Claiming a finished
            number while measurement is still running is the kind of
            quiet wrongness this app tries not to ship.

            ANSWERED includes "rejected" (#956). A failed batch had
            answered by the old `pending > 0` test, so the qualifier came
            off a total that was still partial -- and unlike a pending
            batch, a failed one never comes back to correct it. */}
        <span className="text-[#8b949e]">
          {sizesComplete ? "" : "at least "}
          {formatSize(totalBytes)}
        </span>
        {/* Says WHY the total is a floor, which "at least" alone cannot.
            Not a `QueryError`: the scan succeeded and every row on this
            page is real -- it is the SIZES that are short, which is the
            partial-answer case `PartialScanNotice` exists for and which
            this app answers with a label rather than a blank page. No
            retry, for the reason that component states: `retry: false` is
            deliberate here (`useRemoveArtifacts` measured a 20.4s freeze
            from re-running these very calls), and a second identical walk
            does not read what the first could not. */}
        {failed > 0 ? (
          <span role="status" className="text-xs text-[#d29922]">
            {failed} of {total} size {failed === 1 ? "batch" : "batches"} could not be
            measured, so every byte figure here is a floor
          </span>
        ) : null}
        {/* The measuring advisory has MOVED, to after the Remove buttons
            (#852). See the comment beside it down there -- ordering is
            #817's actual remedy, and it was upstream of two destructive
            buttons here. */}
        {/* Age is the more useful ordering when every row is the same
            size -- which is the normal case for node_modules. */}
        <label className="flex items-center gap-1 text-xs text-[#8b949e]">
          Sort
          <select
            value={sort}
            onChange={(e) => setSort(e.target.value as "size" | "age")}
            aria-label="Sort artifacts"
            className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 text-xs text-[#e6edf3]"
          >
            <option value="size">Largest</option>
            {/* "Least recently written", not "Oldest": the ordering is
                by last write, and "oldest" invites reading it as
                creation date. */}
            <option value="age">Least recently written</option>
          </select>
        </label>

        <HelpButton topic="build-artifacts" />

        {/* A SPACER, replacing `ml-auto` on the buttons below (#852).

            `ml-auto` made each Remove button's position a function of
            everything upstream of it: a variable-width advisory appearing
            mid-scan consumed the free space the margin was absorbing and
            pushed the button left -- and on this container, which has no
            `flex-wrap` and no `overflow` on the desktop, off the right
            edge entirely. #817 rejected exactly this kind of remedy
            ("reserving a slot… only stops the two from moving rather than
            stopping them from being neighbours") in favour of structure.

            An explicit `flex-1` item is that structure: it is the ONE
            element that absorbs slack, so it is the only thing that
            changes width when anything else does. Upstream items size
            from their content and downstream items keep their position
            relative to the right edge, rather than every item inheriting
            a margin whose value depends on all the others.

            `min-w-0` so it can collapse to nothing on a narrow window
            rather than being the thing that forces a wrap. */}
        <span aria-hidden="true" className="min-w-0 flex-1" />

        {/* One click for the group, EXCLUDING anything a build may be
            writing to. Those are refused at delete time anyway, so
            selecting them would only produce a failure report the user
            did not ask for -- and the count in the label would promise
            more than the click delivers. */}
        {/* A FIXED-WIDTH action cell, so the two buttons cannot jump past
            each other (#852).

            The pair differ in width by a lot -- "Remove all 47 · 112 GB"
            against "Remove 3 · 8 GB" -- and they swap on the first
            checkbox tick, so ticking a row moved the button the user was
            about to aim at. `WorktreesPage`'s row reached the same shape
            for the same reason: "A row's layout should not depend on
            which action it currently offers."

            `justify-end` so the button hugs the right edge whichever one
            is showing, and the cell is absent entirely when neither is --
            a permanently reserved gap on a toolbar that is usually
            complete without it is the cost #817 declined to pay, and here
            there is no need: the spacer above already holds the position,
            so removing the cell changes nothing upstream of it. */}
        {removable.length > 1 || checked.size > 0 ? (
          <span
            className={
              isMobile
                ? "flex shrink-0 items-center justify-end"
                : "flex w-48 shrink-0 items-center justify-end"
            }
          >
            {checked.size > 0 ? (
              <button
                type="button"
                disabled={busy}
                onClick={() => setConfirming(true)}
                className="rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10 disabled:opacity-50"
              >
                {/* The COUNT and the size in the label, so the scope is
                    legible before the dialog rather than only inside it. */}
                {/* Same rule as the "Remove all" label (#956):
                    `selectedBytes` sums with `?? 0` over a map that a
                    failed batch left holes in, so it is a floor rather
                    than a figure and does not go on the button. */}
                {busy
                  ? "Removing…"
                  : sizesComplete
                    ? `Remove ${checked.size} · ${formatSize(selectedBytes)}`
                    : `Remove ${checked.size}`}
              </button>
            ) : (
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setChecked(new Set(removable.map((r) => r.path)));
                  setConfirming(true);
                }}
                className="rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10 disabled:opacity-50"
              >
                Remove all {removable.length}
                {/* The size appears only when it IS the size (#956). The
                    old test was `pending === 0`, which a rejection
                    satisfies, so the button carried a figure summed with
                    `?? 0` over a map missing the failed batch's paths --
                    an understated number on the label of the click that
                    deletes. Omitted rather than qualified here, because a
                    button label is not where a caveat is read; the
                    advisory above says what is missing, and the dialog
                    below states the floor in words. */}
                {sizesComplete ? ` · ${formatSize(removableBytes)}` : ""}
              </button>
            )}
          </span>
        ) : null}

        {/* The measuring advisory, AFTER the destructive buttons -- #817's
            actual remedy, applied here (#852).

            It used to sit upstream of both Remove buttons, and it appears
            on its OWN schedule as each repository's measurement lands. So
            a purely advisory element, arriving unbidden, moved a control
            that deletes directories -- and the desktop container has no
            `flex-wrap` and no `overflow`, so the button went off the right
            edge rather than onto a second line.

            `WorktreesPage`: "With the group after Remove, nothing upstream
            of Remove changes width when the button appears… by
            construction rather than by tuning." The same argument settles
            this: nothing downstream of Remove can move it, whatever it
            does, because the spacer above is the only item that yields.

            The cost is the same one #817 accepted -- an advisory is no
            longer where the eye first lands -- and the trade is the same:
            this is idempotent and merely informative, Remove deletes
            directories, and when only one of them can hold a stable
            position it is not the advisory one.

            The live region is also now ALWAYS MOUNTED, which is the other
            half of #852. `StatusBar` states the rule: "`aria-live` on the
            CONTAINER, which is always mounted, rather than on the message,
            which is not. A live region has to exist before the text
            appears or the first announcement is missed -- the one that
            matters most, since it is the one saying work started."

            This was the conditional form: the `<span>` carrying
            `aria-live` was itself created when `pending` first went
            positive, so the announcement it was created to make was the
            one a screen reader never heard. The container now persists and
            holds an empty string when idle, exactly as the status bar
            does. It costs nothing visually -- an empty span with no
            padding -- and it means the FIRST count is announced.

            `shrink-0` still, so the advisory is never what the toolbar
            compresses; it has nothing downstream of it to push. */}
        <span aria-live="polite" className="shrink-0 text-xs text-[#58a6ff]">
          {pending > 0 ? `measuring — ${total - pending} of ${total} repositories` : ""}
        </span>
      </div>
      ) : null}

      {confirming ? (
        <Dialog open onOpenChange={(o) => !o && setConfirming(false)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>
              Remove {checked.size} director{checked.size === 1 ? "y" : "ies"}?
            </DialogTitle>
            <ActingOnDesktop />
            {/* The specific loss, computed now. "Are you sure?" is not
                something anyone can act on -- and here the honest answer
                is that the loss is TIME, not work, which is exactly what
                makes this different from removing a worktree. */}
            {/* The specific loss, and how sure of it we are (#956).
                `selectedBytes` is a `?? 0` sum over a map that a failed
                size batch left holes in, so a user reading "This frees
                180 MB" on a partial measurement reads a small number and
                confirms. The dialog is the last place the figure is seen
                before the click, so it is where the floor is stated in
                words rather than left to a qualifier on a button. */}
            <p className="mt-3 text-sm text-[#e6edf3]">
              {sizesComplete
                ? `This frees ${formatSize(selectedBytes)}.`
                : `This frees at least ${formatSize(selectedBytes)} — some sizes could not be measured, so the real figure is larger.`}{" "}
              Everything here is rebuilt by the command shown beside it — the cost is
              the rebuild, not lost work.
            </p>
            {selectedActive > 0 ? (
              <p className="mt-2 text-sm text-[#d29922]">
                {selectedActive} of them {selectedActive === 1 ? "was" : "were"} written
                to recently and may have a build running. Those will be refused.
              </p>
            ) : null}
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirming(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const paths = [...checked];
                  // DIAGNOSTIC (Settings > diagnostic log). The freeze
                  // report is about the window between this click and
                  // the UI responding, which spans a command, a render,
                  // and a query invalidation -- no single call wraps it,
                  // so it is bracketed by hand.
                  const clickedAt = performance.now();
                  diagMark(`ui remove_artifacts click n=${paths.length} rows=${rows.length}`);
                  setConfirming(false);
                  setBusy(true);
                  remove(paths).then(
                    (outcomes) => {
                      diagMark(
                        `ui remove_artifacts resolved ${Math.round(
                          performance.now() - clickedAt,
                        )}ms`,
                      );
                      setBusy(false);
                      // Clear only what was actually REMOVED.
                      //
                      // A blanket reset discarded two things: anything
                      // ticked while the removal was in flight (a long
                      // window, with no sign it happened), and the
                      // selection for rows that FAILED -- which are
                      // exactly the ones still needing attention.
                      setChecked((prev) => {
                        const next = new Set(prev);
                        for (const o of outcomes) {
                          if (o.error === null) next.delete(o.path);
                        }
                        return next;
                      });
                      // After the state updates above, so the gap
                      // between "resolved" and this line is the render
                      // cost rather than the removal's.
                      queueMicrotask(() =>
                        diagMark(
                          `ui remove_artifacts settled ${Math.round(
                            performance.now() - clickedAt,
                          )}ms`,
                        ),
                      );
                      const failed = outcomes.filter((o) => o.error !== null);
                      const ok = outcomes.length - failed.length;
                      // Never a bare "done": a directory refused at
                      // delete time is the guard working, and hiding it
                      // would misreport what is still on disk.
                      if (failed.length === 0) {
                        toast.success(`Removed ${ok} director${ok === 1 ? "y" : "ies"}`);
                      } else {
                        toast.error(
                          `${failed.length} of ${outcomes.length} could not be removed`,
                          { description: failed.map((f) => f.error).join("\n") },
                        );
                      }
                    },
                    (e: unknown) => {
                      setBusy(false);
                      // Silent when the user dismissed the biometric prompt:
                      // they declined, nothing was removed, and reporting
                      // their own decision back as a failure is noise.
                      if (isCancelled(e)) return;
                      toast.error("The removal could not run", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Remove
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {/* `!isError` as well (#846). `rows` is empty on a rejection so
          this renders nothing either way, but an empty `<ul>` under an
          error panel is a list asserting there are no rows -- and the
          gate should say what it means rather than rely on the data
          happening to be empty. */}
      {showArtifacts && !isError ? (
      <ul className="flex flex-col gap-1">
        {rows.map((a) => (
          <ArtifactRow
            key={a.path}
            artifact={a}
            bytes={sizes.get(a.path)}
            ageSecs={ages.get(a.path)}
            checked={checked.has(a.path)}
            onToggle={() => toggle(a.path)}
          />
        ))}
      </ul>
      ) : null}

      {/* Shown on "Everything" and on the virtualenv page, hidden when a
          build-artifact group is selected -- the sidebar's whole point is
          that choosing a group narrows the page to it. */}
      {showVenvs ? <VenvSection /> : null}

      {/* The ledger, on "Everything" only: it spans both kinds, so it
          belongs where the whole picture is rather than repeated under
          each group. */}
      {group === undefined ? <CleanupLog /> : null}
    </div>
  );
}

function ArtifactRow({
  artifact,
  bytes,
  ageSecs,
  checked,
  onToggle,
}: {
  artifact: Artifact;
  bytes: number | undefined;
  ageSecs: number | undefined;
  checked: boolean;
  onToggle: () => void;
}) {
  const active = ageSecs !== undefined && ageSecs < ACTIVE_SECS;
  // The cells, built once. The desktop lays them out on one line; the
  // phone puts the checkbox, path and size on the first line and the
  // kind, age and rebuild hint beneath, so the path keeps most of the
  // width rather than being squeezed between five fixed columns.
  const isMobile = useIsMobile();
  const checkbox = (
      <input
        type="checkbox"
        checked={checked}
        onChange={onToggle}
        // The PATH, not "select": with 178 rows an unnamed checkbox is
        // 178 identical controls to a screen reader.
        aria-label={`Select ${artifact.path}`}
        className="shrink-0"
      />
  );
  const kind = (
      <span className="shrink-0 rounded-full border border-[#30363d] px-2 py-0.5 text-xs text-[#8b949e]">
        {LABEL[artifact.kind]}
      </span>
  );
  const path = (
      <span className="min-w-0 flex-1 truncate font-mono text-xs text-[#e6edf3]">
        {artifact.path}
      </span>
  );
  const facts = (
    <>
      {active ? (
        // Surfaced rather than hidden: this is the one hazard git cannot
        // see, and the user is the only one who knows whether a build is
        // theirs.
        <span className="shrink-0 text-xs text-[#d29922]">written recently</span>
      ) : null}
      <span className="shrink-0 text-xs text-[#8b949e]">{REBUILD[artifact.kind]}</span>
      {/* Age, not just the recently-written warning.
          Size cannot rank these: every node_modules is ~1.4 GB, so the
          list sorts identically and says nothing about which are safe to
          delete. How long ago it was written is the discriminator.
          Undefined renders as a skeleton, never as "just now" -- the
          same rule the size column follows for "not measured yet". */}
      {/* Titled, because the number alone cannot say WHICH date it is.
          "3 months ago" against a build directory reads equally well as
          "created then" or "last touched then", and those imply opposite
          actions: a directory created months ago but written to this
          morning is in active use, while one created this morning and
          untouched since is not. The value is the newest mtime of
          anything INSIDE the tree (`artifacts/mod.rs`), which is the
          more useful of the two -- a running `cargo build` writes deep
          inside without touching the root's own timestamp. #722. */}
      <span
        title="Last written: the most recent change to anything inside this directory, not when the directory itself was created."
        className={isMobile ? "shrink-0 text-xs text-[#8b949e]" : "w-24 shrink-0 text-right text-xs text-[#8b949e]"}
      >
        {ageSecs === undefined ? (
          <Skeleton className="ml-auto w-16" />
        ) : (
          relativeSeconds(ageSecs)
        )}
      </span>
    </>
  );
  const size = (
      <span className="w-20 shrink-0 text-right tabular-nums">
        {bytes === undefined ? <Skeleton className="ml-auto w-14" /> : formatSize(bytes)}
      </span>
  );
  if (isMobile) {
    return (
      <li className="flex flex-col gap-1 rounded border border-[#30363d] px-3 py-2 text-sm">
        <div className="flex items-center gap-3">
          {checkbox}
          {path}
          {size}
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pl-7">
          {kind}
          {facts}
        </div>
      </li>
    );
  }
  return (
    <li className="flex items-center gap-3 rounded border border-[#30363d] px-3 py-2 text-sm">
      {checkbox}
      {kind}
      {path}
      {facts}
      {size}
    </li>
  );
}
