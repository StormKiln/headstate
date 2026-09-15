import { useState } from "react";
import { toast } from "sonner";
import {
  useCancelUpdateAll,
  useUpdateAllProgress,
  useUpdateAllRepositories,
} from "@/api/hooks";
import type { UpdateAllReport, RepoUpdateOutcome } from "@/api/tauri";
import { summarise, tally, updateAllLabel } from "@/lib/updateAll";

/// Bring every repository level with its remote default branch (#1012,
/// #1014, #1016, #1025).
///
/// # Fast-forward only, and the label says so
///
/// Not "attempt to update". Every way of honouring that open-ended
/// promise for a repository that cannot fast-forward -- stash, rebase,
/// merge, reset -- writes to a working tree the app did not create, on
/// behalf of a user who clicked one button for 45 repositories and is
/// watching a progress bar rather than that repository. With `--ff-only`
/// there is no merge and therefore no conflict: git either moves the ref
/// or refuses, and a refusal leaves the working tree byte-identical.
///
/// # The verdict in the table beside this is a reason to OFFER, never to DO
///
/// Every precondition is re-derived inside the command at the moment of
/// acting, per repository. The table's currency column was computed when
/// the table was drawn -- possibly minutes ago, and across 45 rows some
/// have certainly stopped being true since. This button therefore sends
/// no list and no verdict: it asks the desktop to work out the set and
/// the state for itself.
///
/// # Over a partial scan it does not say "All" (#1025)
///
/// The label names the number it can actually see, because "Update All
/// Repositories" over a census that could not read three directories is a
/// lie about scope. It stays ENABLED -- disabling it would let one
/// unreadable directory block updating 42 perfectly good repositories,
/// trading a lie for a broken feature -- and the completion summary
/// repeats the shortfall, so a user who dismissed `PartialScanNotice`
/// before clicking still learns the set was short.
export function UpdateAllButton({
  count,
  unreadable,
}: {
  /// How many repositories the table can see. The button names this
  /// number when the scan fell short.
  count: number;
  /// What the scan could not read, carried from `RepoScan.unreadable`.
  unreadable: readonly string[];
}) {
  const update = useUpdateAllRepositories();
  const cancel = useCancelUpdateAll();
  const progress = useUpdateAllProgress();
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<UpdateAllReport | null>(null);

  const run = () => {
    setRunning(true);
    // The previous run's report is cleared on START, not on completion:
    // leaving the last result on screen beside a live progress bar would
    // let a user read a stale summary as this run's answer.
    setReport(null);
    update()
      .then(setReport)
      .catch((e: unknown) => {
        // The run itself failing -- a refused second run, a scan that
        // could not start. Distinct from a run that COMPLETED with
        // failures in it, which is the report above and not a rejection.
        toast.error(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setRunning(false));
  };

  const stop = () => {
    cancel().catch((e: unknown) => {
      toast.error(e instanceof Error ? e.message : String(e));
    });
  };

  return (
    <div className="px-4 py-2">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={run}
          disabled={running || count === 0}
          className="rounded-md border border-[#30363d] px-3 py-1 text-sm text-[#e6edf3] disabled:opacity-50"
        >
          {updateAllLabel(count, unreadable)}
        </button>
        {/* Cancel appears only while a run is going, and it is the
            difference between a feature and a 22-minute hang: MEASURED,
            45 repositories against unreachable remotes is 45 x the 30s
            git timeout. The user who realises they are on the wrong
            network realises it within seconds. */}
        {running ? (
          <button
            type="button"
            onClick={stop}
            className="rounded-md border border-[#30363d] px-3 py-1 text-sm text-[#e6edf3]"
          >
            Stop
          </button>
        ) : null}
        {/* `aria-live` so a screen reader hears the run progress and
            finish without the focus moving. `polite`, not `assertive`:
            this updates once per repository and interrupting 45 times
            would be unusable. */}
        <span aria-live="polite" className="text-xs text-[#8b949e]">
          {running
            ? progress
              ? `Updating ${progress.done} of ${progress.total}…`
              : "Updating…"
            : null}
        </span>
      </div>
      {/* Fast-forward only, stated on the button rather than discovered
          in the report. The label is a promise and this is its scope. */}
      <p className="mt-1 text-xs text-[#8b949e]">
        Fast-forwards only. Repositories with uncommitted changes, on another branch, or that cannot
        fast-forward are skipped and listed.
      </p>
      {report ? <UpdateAllReportPanel report={report} /> : null}
    </div>
  );
}

/// What the run did, per repository (#1014).
///
/// Never one aggregate. The summary line distinguishes could-not from
/// did-not, and the rows beneath it name the repositories so the user
/// knows where to go -- which is the reason paths are in the RESULT and
/// not in the progress events.
function UpdateAllReportPanel({ report }: { report: UpdateAllReport }) {
  const t = tally(report);
  // Failures first, then skips. Already-level and updated rows carry no
  // action, and listing 21 of them above the 3 that need attention would
  // bury the only part worth reading. The counts for those two live in
  // the summary line, which is where a total belongs.
  const needsAttention = report.outcomes.filter((o) => o.result.state === "failed");
  const skipped = report.outcomes.filter((o) => o.result.state === "skipped");
  const notAttempted = report.outcomes.filter((o) => o.result.state === "notAttempted");

  return (
    <div className="mt-2 rounded-md border border-[#30363d] px-3 py-2">
      <p role="status" className="text-sm text-[#e6edf3]">
        {summarise(report)}
      </p>
      {needsAttention.length > 0 ? (
        <OutcomeList title="Could not be reached" outcomes={needsAttention} />
      ) : null}
      {skipped.length > 0 ? <OutcomeList title="Skipped" outcomes={skipped} /> : null}
      {notAttempted.length > 0 ? (
        <OutcomeList title="Not attempted" outcomes={notAttempted} />
      ) : null}
      {/* The scan's shortfall, repeated here (#1025). Rendered as a
          DIRECTORY count and visibly not a repository outcome: an
          unreadable directory is a hole in the list, not a row in it, and
          summing it into a per-repository count would invent rows for
          things that were never enumerated. */}
      {t.unreadable > 0 ? (
        <ul className="mt-2 list-none space-y-1">
          {report.unreadable.map((u) => (
            <li key={u} className="font-mono text-xs text-[#8b949e]">
              {u}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

/// One group of outcomes, each carrying git's own message.
///
/// The message survives to the screen rather than being replaced by a
/// category label: "could not update" says nothing, while git's refusal
/// usually names the host, the permission or the ref. The category is
/// ADDITIONAL -- it is this list's heading -- never a replacement.
function OutcomeList({ title, outcomes }: { title: string; outcomes: RepoUpdateOutcome[] }) {
  return (
    <div className="mt-2">
      <h3 className="text-xs font-semibold text-[#8b949e]">{title}</h3>
      <ul className="mt-1 space-y-1">
        {outcomes.map((o) => (
          <li key={o.path} className="text-xs text-[#8b949e]">
            <span className="font-mono text-[#e6edf3]">{o.path}</span>
            {" — "}
            {reasonOf(o)}
          </li>
        ))}
      </ul>
    </div>
  );
}

/// Git's own words, or this app's own refusal, whichever the outcome
/// carries. Never a generic category string.
function reasonOf(o: RepoUpdateOutcome): string {
  switch (o.result.state) {
    case "failed":
      return o.result.error;
    case "skipped":
      return o.result.reason;
    case "notAttempted":
      return "the run ended before reaching it";
    case "updated":
      return o.result.message;
    case "alreadyLevel":
      return "already level";
  }
}
