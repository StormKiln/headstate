import { useState } from "react";
import { toast } from "sonner";
import { useCleanupLog } from "@/api/hooks";
import { formatSize } from "@/lib/worktrees";
import { relativeTime } from "@/lib/time";

/// How many ledger rows render at once (#852).
///
/// A named constant rather than a literal inside the `slice`, because the
/// number now appears TWICE -- in the slice and in the "showing 50 of N"
/// line beneath it -- and a literal in both places is how the stated cut
/// drifts from the actual one. A ledger whose footer misreports its own
/// truncation is worse than one that says nothing.
const LEDGER_ROWS = 50;

/// What each action means, in the terms the user cares about.
const ACTION_TONE: Record<string, string> = {
  proposed: "bg-[#1f6feb]/15 text-[#58a6ff]",
  skipped: "bg-[#d29922]/15 text-[#d29922]",
  refused: "bg-[#d29922]/15 text-[#d29922]",
  removed: "bg-[#f85149]/15 text-[#f85149]",
};

/// The cleanup ledger, and a button to run a pass now.
///
/// This IS Phase 1. The setting turns the rules on; this is where the
/// user finds out what those rules actually picked on their own machine
/// — which is the only thing that can turn "trust this predicate" into
/// "I have read this list".
export function CleanupLog() {
  // `isLoading` as well as the entries (#852). The hook has always
  // exposed it and this never read it, so during the INITIAL load the
  // empty-state copy below ran -- "Turn on automatic cleanup in
  // Settings, then check now" -- telling the user to switch on a setting
  // that is very often already on.
  //
  // That is the failure `RepoPickerSidebar` documents fixing: "'No
  // repositories found' is a DIAGNOSIS, not a holding message… Shown
  // before the scan finishes it says the scan directories are wrong when
  // they are fine, and sends someone to fix something that is not
  // broken."
  const { entries, isLoading, isError, refetch, run } = useCleanupLog(true);
  const [busy, setBusy] = useState(false);

  const proposed = entries.filter((e) => e.action === "proposed");
  const bytes = proposed.reduce((n, e) => n + (e.bytes ?? 0), 0);
  // The rows actually rendered, and the CUT is stated below (#852).
  //
  // `entries.slice(0, 50)` was the only silent truncation in the
  // codebase. Against `StatsSidebar`: "The count is always stated ('Show
  // all 49'), never silently cut" -- and `PrRow` prints a `+N` with a
  // `title` for the same reason.
  //
  // It matters more here than anywhere else, because this component's
  // whole purpose (see the doc comment above) is turning "trust this
  // predicate" into "I have read this list". A silently cut audit log
  // cannot do that: the reader believes they have read the ledger.
  const shown = entries.slice(0, LEDGER_ROWS);
  const elided = entries.length - shown.length;

  return (
    <section className="mt-6">
      <div className="mb-3 flex items-center gap-2 text-sm">
        <span className="font-semibold text-[#e6edf3]">What cleanup would reclaim</span>
        {proposed.length > 0 ? (
          <span className="text-[#8b949e]">
            {proposed.length} item{proposed.length === 1 ? "" : "s"} · {formatSize(bytes)}
          </span>
        ) : null}
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            run().then(
              (out) => {
                setBusy(false);
                // The COUNT, because a run that found nothing is a real
                // answer and a silent button is not.
                toast.success(
                  out.length === 0
                    ? "Nothing to reclaim right now"
                    : `Found ${out.length} item${out.length === 1 ? "" : "s"}`,
                );
              },
              (e: unknown) => {
                setBusy(false);
                toast.error("The cleanup pass could not run", {
                  description: typeof e === "string" ? e : undefined,
                });
              },
            );
          }}
          className="ml-auto rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#161b22] disabled:opacity-50"
        >
          {busy ? "Checking…" : "Check now"}
        </button>
      </div>

      {/* A HOLDING message first, then the diagnosis (#852).

          The two are opposite answers -- "we have not read the ledger
          yet" and "we read it and it is empty" -- and the diagnosis is
          the one that sends someone into Settings. Rendering it during
          the initial load pointed at a setting that is already on.

          And the THIRD answer, which #852 left out when it added the
          second: "we tried to read the ledger and could not" (#854). It
          could not be written at all until `useCleanupLog` returned
          `isError` -- the hook destructured it and kept it. First of the
          three arms, because `entries` keeps its `[]` default on a
          rejection so an arm after the empty one never renders, and
          before `isLoading`, because a retry leaves both true. */}
      {isError ? (
        <div>
          <p className="text-sm text-[#f85149]">Could not read the cleanup ledger.</p>
          {/* A bordered button rather than the bare underlined link this
              was, for the reason #974 gives: at `text-sm` with no padding
              it was a ~17px hit area against the 44px minimum
              `index.css` establishes, and it looked like a different kind
              of thing from the identical "Try again" four other surfaces
              render as a button. */}
          <button
            type="button"
            onClick={() => void refetch()}
            className="tap-target mt-1 rounded border border-[#30363d] px-2 py-0.5 text-sm text-[#e6edf3] hover:bg-[#161b22]"
          >
            Try again
          </button>
        </div>
      ) : isLoading ? (
        <p className="text-sm text-[#8b949e]">Reading the cleanup ledger…</p>
      ) : entries.length === 0 ? (
        <p className="text-sm text-[#8b949e]">
          No reports yet. Turn on automatic cleanup in Settings, then check now.
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {shown.map((e, i) => (
            <li
              key={`${e.at}-${e.target}-${i}`}
              // `overflow-hidden`, the third part of the #818 remedy
              // (#852). Without it a cell that overflows its share draws
              // straight through the bordered box, so even a correctly
              // shrinking neighbour cannot stop the row spilling -- and
              // the box stops describing what is in it.
              className="flex items-center gap-3 overflow-hidden rounded border border-[#30363d] px-3 py-2 text-sm"
            >
              <span
                className={`shrink-0 rounded-full px-2 py-0.5 text-xs ${
                  ACTION_TONE[e.action] ?? "bg-[#30363d] text-[#8b949e]"
                }`}
              >
                {e.action}
              </span>
              {/* The TARGET keeps its `flex-1`, and the `title` is new.
                  It is the row's IDENTIFIER -- which item the ledger is
                  discussing -- and a path means nothing partially, so it
                  is the cell that must not be the one squeezed. The
                  tooltip is the only place the truncated tail survives,
                  exactly as #818 concluded for the worktree row. */}
              <span
                title={e.target}
                className="min-w-0 flex-1 truncate font-mono text-xs text-[#e6edf3]"
              >
                {e.target}
              </span>
              {/* The REASON, on anything that was not proposed. A row
                  passed over without explanation reads as a malfunction
                  rather than a guard doing its job. */}
              {/* TRUNCATES, and yields before the target does -- the #818
                  fix this should have inherited (#852).

                  It was `shrink-0` with no width bound, which on a flex
                  row means "claim my full intrinsic width and give none
                  of it back". The target was the only `flex-1` cell, so
                  it absorbed all the pressure: a `refused` entry reading
                  "could not remove … Device or resource busy" collapsed
                  the path to nothing, and the user could not tell WHICH
                  item the ledger was discussing -- on the surface that
                  exists to audit what cleanup deleted.

                  `flex-auto` rather than `flex-1` for the basis, for
                  `WorktreesPage`'s reason: `flex-1` is `flex: 1 1 0%` and
                  would give the reason an equal share of the row however
                  short it is, leaving a two-word refusal sitting in a
                  half-row of whitespace. `flex-auto` is `flex: 1 1 auto`
                  -- sized from its content, as it effectively was while
                  `shrink-0`, and the only change is that it can now give
                  width back under pressure.

                  The priority is deliberate: of the two variable cells
                  the reason is the more expendable, because it is a
                  SENTENCE whose first clause carries the decision and
                  whose tail is elaboration, while the target is an
                  identifier. A clipped sentence still reads; half a path
                  does not.

                  And a `title`, which the advisory had none of -- so
                  clipped text was unrecoverable, worse than on the
                  worktree row where #818 at least left a tooltip. */}
              {e.error ? (
                <span
                  title={e.error}
                  className="min-w-0 flex-auto truncate text-xs text-[#8b949e]"
                >
                  {e.error}
                </span>
              ) : null}
              <span className="shrink-0 text-xs text-[#8b949e]">
                {relativeTime(e.at)}
              </span>
              <span className="w-20 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                {e.bytes !== null ? formatSize(e.bytes) : "—"}
              </span>
            </li>
          ))}
          {/* The CUT, stated (#852). `StatsSidebar`: "The count is always
              stated ('Show all 49'), never silently cut."

              Says both halves -- how many are shown and how many exist --
              because "and 12 more" alone leaves the reader doing
              arithmetic to find out whether they have read the ledger.
              A `<li>` inside the list rather than a sibling paragraph, so
              it travels with the rows it is describing and a screen
              reader reaches it as the list's last item. */}
          {elided > 0 ? (
            <li className="px-3 py-1 text-xs text-[#8b949e]">
              Showing {shown.length} of {entries.length} — the {elided} older{" "}
              {elided === 1 ? "entry is" : "entries are"} not listed.
            </li>
          ) : null}
        </ul>
      )}
    </section>
  );
}
