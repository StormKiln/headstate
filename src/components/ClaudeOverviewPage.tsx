import { AlertTriangle, Bot, FolderX, Play, RotateCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useClaudeOverview } from "../api/hooks";
import { copyText } from "../lib/clipboard";
import { relativeTime } from "../lib/time";
import { pathBasename } from "../lib/worktrees";
import { QueryError, errorMessage } from "./QueryError";
import { SessionsChart } from "./stats/SessionsChart";
import { Card } from "@/components/ui/card";
import type { ClaudeResumable } from "@/types/pr";

/// How many days the activity chart covers.
///
/// MUST equal `ACTIVITY_DAYS` in `src-tauri/src/claude/overview.rs`, which
/// is what actually cuts the window -- this is the number the SUBTITLE
/// quotes. A mismatch renders "the last 30 days" over 14 bars, and
/// `mirroredConstants.test.ts` is the house mechanism that catches exactly
/// that (#850: every test in a directory using a constant SYMBOLICALLY is
/// self-consistent at any value).
export const ACTIVITY_DAYS = 30;

/// A headline figure with the copy that says what it means.
///
/// `value` is `number | null` and `null` renders as "could not tell"
/// rather than as a dash or a zero. That is the page's central rule in
/// component form: on a dashboard, zero is a MEASUREMENT and absence is
/// not, and the two look identical unless something forces them apart.
function Tile({
  label,
  value,
  hint,
  tone = "plain",
  Icon,
}: {
  label: string;
  /// `null` means the figure could not be established. Never coerced to 0.
  value: number | null;
  hint: string;
  /// `action` is the one figure a user is meant to act on, and it is the
  /// only one that gets colour. Colour on every tile would rank nothing.
  tone?: "plain" | "action";
  Icon: typeof Bot;
}) {
  return (
    <Card className="px-4">
      <div className="flex items-center gap-1.5 text-xs text-[#8b949e]">
        <Icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        {label}
      </div>
      <div
        className={`mt-1 text-2xl font-semibold tabular-nums ${
          tone === "action" ? "text-[#3fb950]" : "text-[#e6edf3]"
        }`}
      >
        {value === null ? (
          // Grey and in words, following `NotMeasured`: an absent reading
          // is not a warning, and amber would tell the user to act on
          // something the app simply did not look at.
          <span className="text-base font-normal text-[#8b949e]">Could not tell</span>
        ) : (
          value.toLocaleString()
        )}
      </div>
      <div className="mt-1 text-xs text-[#8b949e]">{hint}</div>
    </Card>
  );
}

/// Stats, charts and actions for managing Claude Code sessions (#921).
///
/// # What is on this page, and what was cut
///
/// #921 lists five candidates -- sessions over time, sessions that ended
/// without a `SessionEnd`, longest-running, busiest directories, version
/// spread -- and asks for a cut rather than all five, on one test: *does
/// this help me see what is going on, or resurrect something?* Four
/// panels ship. Each rejection below is a decision with a measurement
/// behind it, taken on the real 1,461-session corpus on this machine.
///
/// ## Shipped
///
/// **1. Three tiles: running, resumable, archived.** The page's reason to
/// exist. `resumable` is the headline -- see the correction below.
///
/// **2. The resumable list.** The actionable surface, and the centrepiece:
/// the newest of the sessions that can be resumed back into the tree they
/// came from, each with its command on one click.
///
/// **3. Sessions started per day.** The one chart. It earns its place on
/// the second-order reading: the shape is how you notice that you started
/// 34 sessions yesterday and can resume 248 of 1,461 ever, which is the
/// fact behind the 83% whose directory is gone.
///
/// **4. A scan-health line.** Absent-is-not-zero applied to the statistics
/// themselves: when the live registry could not be read, or records in it
/// could not be used, every figure above is wrong by an unknown amount and
/// the page has to say so.
///
/// ## Cut, and why
///
/// **Longest-running session.** Cut as a number that would be a lie with
/// a plausible shape. The only duration available is last activity minus
/// first activity, and a session that ran for four minutes and then sat
/// open in a terminal for eight hours measures as eight hours. Nothing in
/// the data distinguishes working from idling -- the transcript records
/// turns, not attention -- so the figure would rank abandoned sessions
/// above intense ones while looking like a measure of effort. A wrong
/// number with a credible shape is worse than no number.
///
/// **Version spread.** Cut as a fact about upgrades, not about sessions.
/// Measured here: 37 distinct `claude` versions across the corpus, the
/// top one holding 95 sessions. It answers "how often do I update Claude
/// Code", which is a real question and not one this page is for -- and
/// neither seeing what is going on nor resurrecting anything turns on it.
///
/// **Busiest directories.** The closest call, and cut on measurement. 665
/// distinct working directories over 1,461 sessions, and the top entries
/// are `.claude/worktrees/agent-*` and `.worktrees/*` -- agent worktrees,
/// since deleted. So a leaderboard of them is a ranking of directories
/// that mostly do not exist, and its rows lead nowhere: you cannot resume
/// into a tree that is gone. The 17% that DO exist are already the
/// resumable list, which is the same information ordered by what you can
/// act on rather than by count. A `RepoTable`-shaped panel here would be
/// the page's most visually assertive element sitting on its weakest
/// footing, which is the caveat `RepoTable`'s own comment exists for.
///
/// **Total sessions as a hero number.** Cut as vanity. "1,461 sessions
/// ever" answers nothing; it appears only as the denominator under the
/// tiles, which is the one job it does.
///
/// # The correction to #921's headline figure
///
/// #921 proposes that the most valuable number is "how many sessions are
/// resurrection candidates -- dead pid, no clean end", and calls it the
/// feature's whole reason to exist. The premise is right and the predicate
/// is wrong, measurably: **it matches zero sessions.**
///
/// A pid reaches `claude_run` only from the `SessionStart` hook (#912,
/// #913). All 1,461 sessions here were imported from transcripts, and a
/// transcript import is forbidden from writing a run at all -- migration
/// 11 declares `claude_run.pid NOT NULL` precisely so an unobserved
/// process cannot be recorded as an observed one. Nor is that transient:
/// sessions that ran before the hook was installed can never acquire a pid
/// retroactively, which is the entire history on any machine that adopts
/// Headstate after using Claude Code.
///
/// The predicate that selects the actionable set is **not running, and its
/// directory still exists**: 248 of 1,461. That is `resumable`, it is the
/// page's headline, and #921's predicate ships beside it as
/// `orphaned_runs` -- honest at 0 today, and the sharper signal once the
/// hook is observing, since `SessionEnd` does not fire on SIGKILL.
/// `never_observed` is what lets a reader tell "nothing crashed" from
/// "nothing was watched".
///
/// # Absent is not zero, and a chart is the worst place to break it
///
/// Four distinct failures, four renderings, none of them a zero:
///
/// | condition | rendering |
/// |---|---|
/// | the aggregate query failed | `QueryError` with the reason. **Not** a page of zeros. |
/// | the live registry could not be read | the page, with a banner; `running` renders "could not tell" |
/// | registry records could not be used | the page, with a banner saying `running` is a floor |
/// | the cache is genuinely empty | "no sessions", and an offer to rescan -- only when the read SUCCEEDED |
///
/// The error arm is ordered FIRST, before the empty one. That ordering is
/// the half of #846's fix its own guard cannot check, and it matters more
/// here than on a list: a zeroed struct would draw 30 chart columns and a
/// "0 resumable" tile that look exactly like a measured quiet month,
/// because **a flat line does not look absent**. On this machine that
/// would be a confident "nothing to resume" over 248 resumable sessions.
///
/// # No `Date.now()` in render
///
/// `now` comes from the poll's `dataUpdatedAt` via `useClaudeOverview`,
/// the way `Sparkline` and `HealthConditions` take it. `yarn lint` forbids
/// the clock read, and the rule is right for its own reason as well: a
/// re-render would otherwise shift every "2 days ago" under unchanged
/// data.
///
/// # Why this route is lazy, and the trap in proving it
///
/// This page reaches `recharts` through `stats/SessionsChart` ->
/// `ui/chart`, and #838's boundary is the ROUTE in `App.tsx`, not the
/// chart: `recharts` is 9.3 MB on disk, the launch chunk is 31% smaller
/// for keeping it off, and the measured cost of getting this wrong is the
/// launch chunk back from 945,919 to 1,378,820 bytes and ~8ms on the
/// median time to React's first commit.
///
/// So this component must be reached ONLY through `lazy(() => import(...))`
/// with no static import of it anywhere in `App.tsx`, and it must be added
/// to the `it.each` table in `App.lazy.test.tsx` in the same change --
/// that guard reads `App.tsx?raw` and is SOURCE-SHAPE ONLY, so a new
/// charting route nobody added to the table sails through CI while
/// silently regressing the bundle. There is no bundle-size gate to catch
/// it (`vite.config.ts` has no `manualChunks`, by documented choice).
///
/// Verified on the build rather than inferred -- `VITE_TARGET=mobile yarn
/// build` then `grep -c recharts` over each chunk, with the counts recorded
/// in the pull request.
///
/// The SESSIONS page (#917) is deliberately NOT lazy, and the split is the
/// point: the page you open to get work back after a crash must not wait
/// on a chunk fetch, and the page you open to look at charts can.
export function ClaudeOverviewPage() {
  const { query, now, rescan } = useClaudeOverview(true);
  const { data, isLoading, isError, error, refetch } = query;
  const [rescanning, setRescanning] = useState(false);

  const onRescan = async () => {
    setRescanning(true);
    try {
      await rescan();
      toast.success("Re-read the Claude Code transcripts.");
    } catch (e: unknown) {
      // Named rather than swallowed. A rescan that could not read
      // `~/.claude/projects` and then silently refreshed unchanged
      // aggregates is the button that looks like it worked.
      toast.error(`Could not rescan: ${errorMessage(e)}`);
    } finally {
      setRescanning(false);
    }
  };

  const header = (
    <div className="flex items-start justify-between gap-3">
      <div>
        <h2 className="text-base font-semibold">Claude Code overview</h2>
        <p className="text-xs text-[#8b949e]">
          What is running, what can be resumed, and how sessions accumulate.
        </p>
      </div>
      <button
        type="button"
        // `void`-wrapped rather than passed directly: an async handler
        // returns a promise into an attribute that expects `void`, which
        // `@typescript-eslint/no-misused-promises` rejects -- and rightly,
        // since a rejection there would be unhandled. `onRescan` catches
        // its own failures and reports them as a toast, so there is
        // nothing left for a caller to await.
        onClick={() => void onRescan()}
        disabled={rescanning}
        className="flex shrink-0 items-center gap-1.5 rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#161b22] disabled:opacity-50"
      >
        <RotateCw
          className={`h-3.5 w-3.5 ${rescanning ? "animate-spin" : ""}`}
          aria-hidden="true"
        />
        {rescanning ? "Rescanning…" : "Rescan"}
      </button>
    </div>
  );

  // ERROR FIRST, before loading and before empty. The ordering is the
  // point (#846): a rejected query must never reach the arms below, where
  // `data` is undefined and every figure would read as absent-or-zero.
  if (isError) {
    return (
      <div className="flex flex-col gap-4">
        {header}
        <QueryError
          title="Could not read the Claude Code sessions"
          message={errorMessage(error)}
          onRetry={() => void refetch()}
        >
          {/* Says what is NOT being claimed. Without this, an error panel
              on a stats page still leaves the reader wondering whether the
              numbers they cannot see are zero. */}
          <p className="mx-auto mt-2 max-w-lg text-sm text-[#8b949e]">
            No figures are shown rather than zeroes: a chart of zeros reads as a
            quiet month, which is not what happened.
          </p>
        </QueryError>
      </div>
    );
  }

  if (isLoading || !data) {
    return (
      <div className="flex flex-col gap-4">
        {header}
        <div className="min-h-40" aria-busy="true" />
      </div>
    );
  }

  const { counts, activity, resumable, live_failure, live_unreadable } = data;
  // The ONE place `running` becomes null. A registry we could not list
  // gives no answer about what is running, and rendering 0 there is
  // #841's fail-open in the place a user acts on it: "nothing is running"
  // is what makes a Resume button look safe.
  const running = live_failure === null ? counts.running : null;

  return (
    <div className="flex flex-col gap-4">
      {header}

      {/* The scan-health line, ABOVE the figures it qualifies. Below them
          it would be read after the numbers had already been believed. */}
      {live_failure !== null ? (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span>
            Could not tell which sessions are running: {live_failure}. The figures
            below count stored history, which is unaffected.
          </span>
        </div>
      ) : live_unreadable.length > 0 ? (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span>
            {live_unreadable.length} live session{" "}
            {live_unreadable.length === 1 ? "record" : "records"} could not be
            used, so &ldquo;running&rdquo; is at least {counts.running} rather
            than exactly {counts.running}. {live_unreadable[0]}
          </span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Tile
          label="Running now"
          value={running}
          Icon={Play}
          hint={
            live_failure !== null
              ? "the live session registry could not be read"
              : "checked against the process table, not just the registry"
          }
        />
        <Tile
          label="Resumable"
          value={counts.resumable}
          Icon={RotateCw}
          tone="action"
          hint="not running, and the directory they ran in still exists"
        />
        <Tile
          label="Directory gone"
          value={counts.archived}
          Icon={FolderX}
          // Stated as normal, because it IS: 83% of the real corpus. A
          // reader who takes this for damage would go looking for a
          // problem that is just how agent worktrees work.
          hint="resumable by id, but they would land wherever you run the command"
        />
      </div>

      <div className="text-xs text-[#8b949e]">
        {counts.sessions.toLocaleString()} sessions in the cache
        {counts.cwd_unknown > 0 ? (
          <>
            {" · "}
            {counts.cwd_unknown.toLocaleString()} whose directory could not be
            checked, counted as neither
          </>
        ) : null}
        {/* #921's predicate, stated rather than hidden -- including when
            it is zero, because the zero is the finding. A reader who
            expected a crash count needs to know the difference between
            "nothing crashed" and "nothing was watched". */}
        {counts.never_observed === counts.sessions ? (
          <>
            {" · "}
            no session has been observed by the hook yet, so a crashed-process
            count is not available
          </>
        ) : counts.orphaned_runs > 0 ? (
          <>
            {" · "}
            {counts.orphaned_runs.toLocaleString()} run
            {counts.orphaned_runs === 1 ? "" : "s"} started and never reported
            ending
          </>
        ) : null}
      </div>

      {/* The centrepiece, ABOVE the chart. The chart is context; this is
          the thing a user came to do. */}
      <Card className="px-4">
        <div className="text-sm font-semibold">Ready to resume</div>
        <div className="text-xs text-[#8b949e]">
          {resumable.length === 0
            ? "sessions whose directory still exists"
            : `the ${resumable.length} most recent of ${counts.resumable.toLocaleString()} — newest activity first`}
        </div>
        {resumable.length === 0 ? (
          <div className="py-8 text-center text-sm text-[#8b949e]">
            {/* Only reachable when the read SUCCEEDED, because the error
                arm returned above. So this is a real answer and is worded
                as one. */}
            No session can be resumed into the directory it ran in.
            {counts.archived > 0 ? (
              <>
                {" "}
                All {counts.archived.toLocaleString()} of them ran somewhere that
                no longer exists.
              </>
            ) : null}
          </div>
        ) : (
          <ul className="mt-3 flex flex-col divide-y divide-[#30363d]">
            {resumable.map((s) => (
              <ResumableRow key={s.session_id} session={s} now={now} />
            ))}
          </ul>
        )}
      </Card>

      <SessionsChart points={activity} days={ACTIVITY_DAYS} />
    </div>
  );
}

/// One row of the resumable list.
///
/// The action is a COPY of `cd <dir> && claude --resume <id>`, never a
/// spawned terminal. That is the house answer and it is already argued in
/// `claudify_command`: macOS has no default-terminal concept at all, so
/// there is no way to know whether to open Terminal.app or iTerm, and on
/// Linux `x-terminal-emulator` is Debian-only. The clipboard works
/// identically everywhere and lands the user in their OWN shell.
///
/// The `cd` is included because `claude --resume <id>` adopts the
/// INVOKING directory rather than the recorded one -- measured in #918 --
/// so a bare command resurrects a session pointed at the wrong tree, which
/// is worse than failing because it looks like it worked. Every row here
/// is one whose directory exists, which is what makes the `cd` safe to
/// include unconditionally; the three-case treatment for gone and
/// could-not-check directories belongs to #918's session list, and those
/// rows never reach this panel.
function ResumableRow({
  session,
  now,
}: {
  session: ClaudeResumable;
  /// Epoch ms from the poll. NOT `Date.now()`: `yarn lint` forbids the
  /// clock read during render, and a re-render would otherwise shift every
  /// "2 days ago" under unchanged data.
  now: number;
}) {
  const copy = async () => {
    // Single quotes around the path, so a directory containing `$(...)`
    // or `;` is a path and not a substitution in the shell the user
    // pastes into. The id is quoted for the same reason: it comes from a
    // filename with no format check.
    const quoted = (s: string) => `'${s.replace(/'/g, "'\\''")}'`;
    const command = session.cwd
      ? `cd ${quoted(session.cwd)} && claude --resume ${quoted(session.session_id)}`
      : `claude --resume ${quoted(session.session_id)}`;
    const failure = await copyText(command);
    if (failure) {
      // The REASON, not "could not copy". `copyText` distinguishes an
      // insecure context from a rejected write, and the two have
      // different remedies.
      toast.error(`Could not copy the command: ${failure}`);
      return;
    }
    toast.success("Copied the resume command.");
  };

  return (
    <li className="flex items-center gap-3 py-2">
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm text-[#e6edf3]">
          {/* Claude's own `aiTitle` names 1,459 of 1,461 sessions. The
              two without one show their id rather than an invented name:
              a fabricated title cannot be told from a real one. */}
          {session.name ?? session.session_id}
        </div>
        <div className="truncate text-xs text-[#8b949e]">
          {session.cwd ? (
            <span title={session.cwd}>{pathBasename(session.cwd)}</span>
          ) : null}
          {session.git_branch ? <> · {session.git_branch}</> : null}
          {session.last_activity_at ? (
            <> · {relativeTime(session.last_activity_at, new Date(now))}</>
          ) : (
            // Grey and in words. "No recorded activity" is not "active
            // just now", and a missing timestamp must not render as the
            // freshest row on the page.
            <> · no recorded activity</>
          )}
        </div>
      </div>
      <button
        type="button"
        onClick={() => void copy()}
        className="shrink-0 rounded border border-[#30363d] px-2.5 py-1 text-xs text-[#e6edf3] hover:bg-[#161b22]"
      >
        Copy resume
      </button>
    </li>
  );
}
