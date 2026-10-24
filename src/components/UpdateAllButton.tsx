import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
import {
  useCancelUpdateAll,
  useUpdateAllProgress,
  useUpdateAllRepositories,
  useUpdateAllState,
} from "@/api/hooks";
import type { UpdateAllReport, RepoUpdateOutcome } from "@/api/tauri";
import { summarise, updateAllLabel } from "@/lib/updateAll";

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
/// have certainly stopped being true since. The button sends the displayed
/// scope, never cached verdicts. The desktop rescans and updates only the
/// repositories that are both displayed and still found, listing every
/// other one with its reason rather than refusing the rest (#1954).
///
/// # It never says "All" (#1025, #1957)
///
/// The label always names the number of repositories the table shows
/// (`updateAllLabel`), complete scan or not, because "Update All
/// Repositories" over a census that could not read three directories is a
/// lie about scope, and that number is exactly the scope the run is
/// refused outside of. It stays ENABLED over a partial scan -- disabling
/// it would let one unreadable directory block updating 42 perfectly good
/// repositories. The shortfall itself is stated once, by the sidebar's
/// `PartialScanNotice`, and not repeated here.
///
/// There is no `unreadable` prop: the label does not depend on the
/// shortfall, and a prop documented as qualifying the label while being
/// ignored was the defect #1957 reported.
export function UpdateAllButton({
  count,
  expectedPaths,
}: {
  /// How many repositories the table can see. The button names this
  /// number.
  count: number;
  expectedPaths?: readonly string[];
}) {
  const update = useUpdateAllRepositories();
  const cancel = useCancelUpdateAll();
  const registry = useUpdateAllState();
  const [runId, setRunId] = useState(0);
  const progress = useUpdateAllProgress(runId);
  // This mount's own call is in flight.
  const [inFlight, setInFlight] = useState(false);
  // What this mount's own call returned. Shown only until the registry
  // has the same run on record.
  const [report, setReport] = useState<UpdateAllReport | null>(null);
  // The registry's run that was already on record when this mount last
  // clicked, so it is never shown as that click's answer.
  const [onRecord, setOnRecord] = useState<number | null>(null);

  // The desktop's registry is the record of the run (#1946). The run
  // outlives this component -- the user leaves the page and comes back,
  // or the phone's call gives up after 120 s while the desktop carries on
  // -- so whether a run is going, how far it has got and how the last one
  // ended are read from it, not only from this mount's own call.
  const recorded = registry.data;
  const recordedRunning = recorded?.state === "running" ? recorded : null;
  const running = inFlight || recordedRunning !== null;
  const recordedEnd =
    recorded && recorded.state !== "running" && recorded.run !== onRecord ? recorded : null;
  const shownReport = running ? null : (recordedEnd?.report ?? report);
  // A run that died keeps what it applied, and says why it ended (#1947).
  const failure = !running && recordedEnd?.state === "failed" ? recordedEnd.reason : null;
  const shownProgress =
    progress ?? (recordedRunning ? { done: recordedRunning.done, total: recordedRunning.total } : null);

  // Each finished repository re-reads the registry, which names what has
  // landed, so its row in the table refreshes now rather than when the
  // whole run ends (`useUpdateAllState` does the refreshing).
  const landedCount = progress?.done;
  const refetchRegistry = registry.refetch;
  useEffect(() => {
    if (landedCount) void refetchRegistry();
  }, [landedCount, refetchRegistry]);

  const run = () => {
    setRunId((previous) => previous + 1);
    setInFlight(true);
    // The previous run's report is cleared on START, not on completion:
    // leaving the last result on screen beside a live progress bar would
    // let a user read a stale summary as this run's answer.
    setReport(null);
    setOnRecord(recorded && recorded.state !== "running" ? recorded.run : null);
    update(expectedPaths)
      .then(setReport)
      .catch(async (e: unknown) => {
        // A rejection is not always the end of the run. The phone's call
        // gives up after 120 s while the desktop carries on, and a click
        // is refused while another run is going: ask the registry, and if
        // a run is going, show it rather than a failure that is not one.
        const now = await registry.refetch().catch(() => undefined);
        if (now?.state === "running") return;
        // The run itself failing -- a scan that could not start, or a run
        // that died part-way, whose report the registry kept. Distinct
        // from a run that COMPLETED with failures in it, which is the
        // report below and not a rejection.
        toast.error(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setInFlight(false));
  };

  const stop = () => {
    cancel().catch((e: unknown) => {
      toast.error(e instanceof Error ? e.message : String(e));
    });
  };

  return (
    <div className="px-4 py-2">
      <div className="flex flex-wrap items-center gap-2">
        {/* Fast-forward only, stated beside the button rather than
            discovered in the report: the label is a promise and this is
            its scope. The full sentence is on hover; the report lists
            every repository skipped, with its reason. */}
        <button
          type="button"
          onClick={run}
          disabled={running || count === 0}
          title="Fast-forwards only. Repositories with uncommitted changes, on another branch, or that cannot fast-forward are skipped and listed."
          className="rounded-md border border-[#30363d] px-3 py-1 text-sm text-[#e6edf3] disabled:opacity-50"
        >
          {updateAllLabel(count)}
        </button>
        {running ? null : <span className="text-xs text-[#8b949e]">Fast-forward only</span>}
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
            ? shownProgress
              ? `Updating ${shownProgress.done} of ${shownProgress.total}…`
              : "Checking repository scope…"
            : null}
        </span>
      </div>
      {failure ? (
        <p role="alert" className="mt-2 text-sm text-[#f85149]">
          Did not finish: {failure}
        </p>
      ) : null}
      {shownReport ? <UpdateAllReportPanel report={shownReport} /> : null}
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
        <OutcomeList title="Could not be reached" detailLabel="failed repositories" outcomes={needsAttention} />
      ) : null}
      {skipped.length > 0 ? <OutcomeList title="Skipped" outcomes={skipped} /> : null}
      {notAttempted.length > 0 ? (
        <OutcomeList title="Not attempted" outcomes={notAttempted} />
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
function OutcomeList({ title, detailLabel = title.toLowerCase() + " repositories", outcomes }: {
  title: string;
  detailLabel?: string;
  outcomes: RepoUpdateOutcome[];
}) {
  const [expanded, setExpanded] = useState(false);
  const [page, setPage] = useState(0);
  const id = useId();
  const large = outcomes.length > 3;
  const pageSize = 50;
  const current = Math.min(page, Math.ceil(outcomes.length / pageSize) - 1);
  const start = current * pageSize;
  return (
    <div className="mt-2">
      <h3 className="text-xs font-semibold text-[#8b949e]">{title}</h3>
      {large && <button type="button" aria-expanded={expanded} aria-controls={id}
        className="text-xs underline" onClick={() => { setExpanded(!expanded); setPage(0); }}>
        {expanded ? "Hide" : "Show"} {detailLabel} ({outcomes.length})
      </button>}
      {(!large || expanded) && <div id={id}>
        <ul tabIndex={0} aria-label={title} className="mt-1 max-h-48 space-y-1 overflow-y-auto">
          {outcomes.slice(start, start + pageSize).map((o) => (
            <li key={o.path} className="break-all text-xs text-[#8b949e]">
              <span className="font-mono text-[#e6edf3]">{o.path}</span>{" — "}{reasonOf(o)}
            </li>
          ))}
        </ul>
        {outcomes.length > pageSize && <div className="mt-2 flex gap-2 text-xs">
          <span aria-live="polite">{start + 1}–{Math.min(start + pageSize, outcomes.length)} of {outcomes.length}</span>
          <button type="button" disabled={current === 0} onClick={() => setPage(current - 1)}>Previous</button>
          <button type="button" disabled={start + pageSize >= outcomes.length} onClick={() => setPage(current + 1)}>Next</button>
        </div>}
      </div>}
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
