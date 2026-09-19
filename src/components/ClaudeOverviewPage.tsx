import { AlertTriangle, Bot, ClipboardList, FolderX, Play, RotateCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  useClaudeUsageProfile, useClaudeEventProfile, useClaudeOverview } from "../api/hooks";
import { claudeRestartList } from "../api/tauri";
import { copyText } from "../lib/clipboard";
import { restartExportText } from "../lib/restartExport";
import { IS_DESKTOP_BUILD } from "../lib/target";
import { relativeTime } from "../lib/time";
import { pathBasename } from "../lib/worktrees";
import { QueryError, errorMessage } from "./QueryError";
import { SessionsChart } from "./stats/SessionsChart";
import { Card } from "@/components/ui/card";
import { useFilters } from "@/store/filters";
import type {
  ClaudeCorpus,
  ClaudeProfile,
  ClaudeResumable,
  ClaudeTally,
} from "@/types/pr";

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
///
/// # Clickable, as of #948, and only when there is something to open
///
/// The `tone` prop below marks `action` as "the one figure a user is meant
/// to act on" -- and the tile was a `Card` wrapping three `div`s with
/// no `onClick` and no `href`. On the measured corpus that meant the page
/// coloured the number 179 to say "act on this" and dead-ended: the card
/// below it lists the 12 most recent, so 167 resumable sessions had no path
/// from this page at all.
///
/// The sessions list one click away can show every row, and the two
/// surfaces did not cross-link in either direction. The asymmetry is the
/// evidence this was an omission: `ClaudeCodePage` caps at 200 and renders
/// "Show all 1,474" under a comment stating the house rule that a short
/// list must both STATE the total and offer the rest. This page satisfied
/// the first half and dropped the second.
///
/// `onClick` is OPTIONAL, and a tile without one renders exactly as before
/// -- a `div`, not a dead button. That is what keeps the rule below
/// enforceable by construction rather than by remembering to check.
///
/// ## A `null` tile is never clickable
///
/// `value === null` means the figure could not be established, and the
/// caller must pass no `onClick` for it. A link from a tile reading "Could
/// not tell" would open a filtered list whose emptiness the reader would
/// take for an answer -- the confident-wrong-answer failure, arrived at
/// through navigation instead of a zero. This is asserted here rather than
/// only documented: the `onClick` is dropped when `value === null`, so a
/// caller who forgets gets a plain tile rather than a broken jump.
///
/// ## Surface class: none
///
/// `setClaudePage` + `setClaudeFilter` against the Zustand store. No Tauri
/// command, no IPC, so it works identically on the phone -- where it
/// matters more, because there the tile is most of the screen and the
/// sessions list is 1,474 rows with no other way to narrow them.
function Tile({
  label,
  value,
  hint,
  tone = "plain",
  onClick,
  Icon,
}: {
  label: string;
  /// `null` means the figure could not be established. Never coerced to 0.
  value: number | null;
  hint: string;
  /// `action` is the one figure a user is meant to act on, and it is the
  /// only one that gets colour. Colour on every tile would rank nothing.
  tone?: "plain" | "action";
  /// Where the figure leads, or `undefined` for a tile that leads nowhere.
  ///
  /// Ignored when `value === null`, deliberately and not defensively: a
  /// figure that could not be established has nothing to drill into, and
  /// the caller's own `null` is the same `null` the body renders in words.
  onClick?: () => void;
  Icon: typeof Bot;
}) {
  const figure = (
    <>
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
    </>
  );

  if (onClick === undefined || value === null) return <Card className="px-4">{figure}</Card>;

  return (
    <Card className="p-0">
      <button
        type="button"
        onClick={onClick}
        // "Show these" and not "Open" or an imperative: the tile's own
        // wording is what ranks it, and "Directory gone" must stay worded
        // as NORMAL rather than as damage -- 87.9% of the real corpus is
        // in it. A label that read "Fix" or "Clean up" would turn a fact
        // about how agent worktrees work into a problem to go looking for.
        aria-label={`${label}: show these sessions`}
        // `tap-target` keeps the 44px floor, and `text-left` because the
        // content is a figure block rather than a caption -- a centred
        // button would move every number on the page.
        className="tap-target w-full rounded-md px-4 py-3 text-left hover:bg-[#161b22]"
      >
        {figure}
        {/* The affordance, said in words. A whole card that is clickable
            with nothing saying so is a control a reader finds by accident,
            and on a phone there is no hover to reveal it. */}
        <div className="mt-1.5 text-xs text-[#58a6ff]">Show these sessions →</div>
      </button>
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
/// # This page and the session list now answer the same question (#984)
///
/// They did not. `resumable` counts a session as not-running the moment it
/// is absent from the running set -- 183 of 1,491 measured -- while
/// `liveness::derive` returned `Unknown` for every session the hook had
/// never observed, which was 1,490 of the same 1,491 rows. So this page
/// said "183 of these are ready to resume" and the list beside it said it
/// could not tell whether any of them was running, off one registry read.
///
/// `claude/mod.rs` states the rule that was being broken: this page
/// "derives no liveness of its own -- #917's `liveness` module owns that,
/// and two answers to one question disagree the first time either
/// changes." The fix was in `liveness::derive` rather than here, because
/// this page's reading was the one consistent with `live.rs`'s own
/// `a_missing_registry_is_a_settled_empty_answer`: a registry listing we
/// read WHOLE that does not name a session is positive evidence. Nothing
/// on this page changed; the list stopped hedging. Measured after:
/// `running 1  dead 1490  unknown 0`, and 183 of the 1,490 have a live
/// directory -- which is `resumable` exactly.
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
  // The jump out of this page (#948). One store action rather than a
  // `setClaudePage` followed by a `setClaudeFilter`, for the reason its own
  // doc comment gives at length: #920's "Show in Worktrees" bug was
  // precisely a pair of ordered writes where the natural reading order
  // filed the filter under the page being left.
  const showSessions = useFilters((f) => f.showClaudeSessions);

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
          {/* NOT "the figures below are unaffected", which is what this
              said first and is too strong. `aggregate` classifies a
              session as resumable precisely when it is NOT in the running
              set, so a registry we could not read leaves every live
              session counted as resumable -- and resuming one that is
              already alive starts a second copy of it. The over-count is
              small (never more than the handful that can be live at once)
              and the direction is the one that misleads an action, so it
              is named rather than glossed. */}
          <span>
            Could not tell which sessions are running: {live_failure}. The
            history below is unaffected, but nothing could be subtracted from
            &ldquo;resumable&rdquo; — so any session that IS running is counted
            there, and resuming one that is already alive starts a second copy.
          </span>
        </div>
      ) : live_unreadable.length > 0 ? (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {/* Both halves, because the unreadable record cuts two ways. A
              session it hides is missing from the running set, so
              "running" under-counts -- and `aggregate` then classifies
              that same session by its directory, which can put a LIVE
              session in the resumable list. Resuming one that is already
              alive starts a second copy of it, so the over-count is the
              half that can mislead an action and it has to be said. */}
          <span>
            {live_unreadable.length} live session{" "}
            {live_unreadable.length === 1 ? "record" : "records"} could not be
            used, so &ldquo;running&rdquo; is at least {counts.running} rather
            than exactly {counts.running} — and a session it hides may be
            counted as resumable while in fact still running.{" "}
            {live_unreadable[0]}
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
          // `running` is the ONE place a figure becomes null on this page,
          // and `Tile` drops the jump when it is -- so a registry we could
          // not read gives a tile that says "Could not tell" and leads
          // nowhere, rather than to a Running filter that would be empty
          // for the same unstated reason (#948).
          onClick={() => showSessions("running")}
        />
        <Tile
          label="Resumable"
          value={counts.resumable}
          Icon={RotateCw}
          tone="action"
          hint="not running, and the directory they ran in still exists"
          onClick={() => showSessions("resumable")}
        />
        <Tile
          label="Directory gone"
          value={counts.archived}
          Icon={FolderX}
          // Stated as normal, because it IS: 83% of the real corpus. A
          // reader who takes this for damage would go looking for a
          // problem that is just how agent worktrees work.
          hint="resumable by id, but they would land wherever you run the command"
          onClick={() => showSessions("gone")}
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

      <UsageProfileCard />

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
        {/* The other half of the house rule (#948). The subtitle above
            already STATES the total -- "the 12 most recent of 179" -- and
            `ClaudeCodePage`'s cap comment names both halves of the rule:
            state the total, and offer the rest. This card had the first and
            not the second, so on the measured corpus 167 resumable sessions
            had no path from this page.

            Only when there IS a rest. A footer reading "show all 12" under
            twelve rows is a control that changes nothing, and offering it
            when `resumable.length === counts.resumable` would be a link
            that leads back to what is already on screen. */}
        {resumable.length > 0 && counts.resumable > resumable.length ? (
          <button
            type="button"
            onClick={() => showSessions("resumable")}
            className="tap-target mt-3 self-start rounded px-2 text-xs text-[#58a6ff] hover:bg-[#161b22]"
          >
            Show all {counts.resumable.toLocaleString()} resumable sessions →
          </button>
        ) : null}
      </Card>

      <RestartExportCard />

      <SessionsChart points={activity} days={ACTIVITY_DAYS} />

      {/* BELOW the chart, because this is context rather than the thing a
          user came to do. The page's own rule, stated at the resumable
          card: actionable above, context below. */}
      <TroubleProfileCard />
    </div>
  );
}

/// Export the commands to restart every running session, for a reboot
/// (#1071).
///
/// # Why this is not on the resumable card above
///
/// That card is about sessions that are already STOPPED and whose
/// directory survived. This is the opposite population: the ones that are
/// running right now and are about to be stopped by the user, on purpose.
/// The two want opposite treatment of an uncertain row -- see
/// `claude/export.rs`, which argues it at length -- so putting the button
/// on the resumable card would attach it to the wrong list.
///
/// # Desktop only, and it is a capability question
///
/// `IS_DESKTOP_BUILD`, not `useIsMobile()`. The output is text to paste
/// into a terminal, and an iPhone has no terminal -- the answer does not
/// change when a desktop window is dragged narrower, which is exactly the
/// test `target.ts` states.
///
/// The COMMAND stays `Class::Read` and reachable from the phone all the
/// same: the class decides whether the companion can call it, and a
/// companion user reading "three sessions alive on my laptop, here is
/// what each was doing" is a real away-from-desk answer. What is hidden
/// here is the action, not the fact.
///
/// # On demand, never polled
///
/// The user asks this once, before a reboot. It is the same registry read
/// and process probe the session list already does every ten seconds, and
/// a second timer would double that work to answer a question nobody is
/// asking for most of the session's life.
function RestartExportCard() {
  // Both hooks run before the build check, unconditionally. An early
  // `return` above a `useState` changes the hook order between builds,
  // which React forbids -- and `IS_DESKTOP_BUILD` is a build-time
  // literal, so the whole body folds away on the phone regardless of
  // where the check sits.
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onExport = async () => {
    setBusy(true);
    try {
      const list = await claudeRestartList();
      const built = restartExportText(list);
      setText(built);
      const failure = await copyText(built);
      if (failure) {
        // NOT a failure of the export. The text is on screen and the
        // user can select it, so this says what did not happen rather
        // than implying the whole thing failed -- `copyText`
        // distinguishes an insecure context from a rejected write, and
        // the two have different remedies.
        toast.error(`The list is below, but the clipboard refused it: ${failure}`);
        return;
      }
      const total = list.running.length + list.uncertain.length;
      toast.success(
        total === 0
          ? "Nothing is running — the note below says so."
          : `Copied ${total} restart command${total === 1 ? "" : "s"}.`,
      );
    } catch (e: unknown) {
      // The reason, and NO stale text left on screen. A previous
      // export still showing under a failed refresh is a list the user
      // would save believing it was current.
      setText(null);
      toast.error(`Could not read which sessions are running: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  if (!IS_DESKTOP_BUILD) return null;

  return (
    <Card className="px-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-sm font-semibold">Before you restart</div>
          <div className="text-xs text-[#8b949e]">
            The command to bring back each session that is running now, one per
            line. Save it, reboot, then paste the lines into a terminal.
          </div>
        </div>
        <button
          type="button"
          // `void`-wrapped: an async handler returns a promise into an
          // attribute that expects `void`, which
          // `@typescript-eslint/no-misused-promises` rejects. `onExport`
          // reports its own failures as a toast, so there is nothing left
          // to await.
          onClick={() => void onExport()}
          disabled={busy}
          className="flex shrink-0 items-center gap-1.5 rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#161b22] disabled:opacity-50"
        >
          <ClipboardList className="h-3.5 w-3.5" aria-hidden="true" />
          {busy ? "Reading…" : "Export restart commands"}
        </button>
      </div>

      {text !== null ? (
        <>
          {/* Shown as well as copied, and this is the half that makes
              "save it somewhere" possible without the app picking a
              path. The user selects it and puts it where THEY want --
              a file we wrote to a directory of our choosing is a
              restart list they might never find. */}
          <textarea
            readOnly
            value={text}
            aria-label="Commands to restart the running Claude Code sessions"
            rows={Math.min(20, text.split("\n").length)}
            className="mt-3 w-full resize-y rounded border border-[#30363d] bg-[#0d1117] p-2 font-mono text-xs text-[#e6edf3]"
          />
          <p className="mt-1.5 text-xs text-[#8b949e]">
            Every line that is not a command starts with <code>#</code>, so you
            can paste the whole thing.
          </p>
        </>
      ) : null}
    </Card>
  );
}

/// The failure and denial profile across every session (#1062, #1063,
/// #1064).
///
/// # Why the cross-session view is the informative one
///
/// #1064 puts it plainly: one denial is noise, the same denial forty
/// times is a finding. A per-session view cannot tell those apart, and
/// this is the only place the difference is visible.
///
/// # The denominators are not decoration
///
/// A profile over 3 observed sessions out of 1,461 stored is a very
/// different statement from the same profile over all of them, and
/// without the denominator the two render identically. `sessions_observed`
/// is therefore shown whenever it is short of `sessions`, rather than only
/// when someone thinks to look.
///
/// The error arm precedes the loading arm per #846, and the `unobserved`
/// arm never renders as zeros — on a machine whose history predates the
/// hooks that is the state of the entire corpus, so a grid of zeros here
/// would be the original defect at full scale.
function TroubleProfileCard() {
  const { data, isError, error, isLoading } = useClaudeEventProfile();

  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">Failures and denials</div>
      <div className="text-xs text-[#8b949e]">
        what the hooks recorded across every session
      </div>
      {isError ? (
        <div className="py-6 text-center text-sm text-[#8b949e]">
          Could not read what the hooks recorded, so failures and denials are unknown
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </div>
      ) : isLoading || data === undefined ? (
        <div className="py-6 text-center text-sm text-[#8b949e]" aria-busy="true">
          Reading what the hooks recorded…
        </div>
      ) : data.observation.state === "unobserved" ? (
        /* Absent is not zero, at corpus scale. `overview.rs` measured
           1,461 of 1,461 sessions in exactly this state on the development
           machine, so this arm is the NORMAL one before the hooks go in
           and a page of zeros here would be #846 at its most convincing. */
        <div className="py-6 text-center text-sm text-[#8b949e]">
          Not recorded. No hook has observed any of your{" "}
          {data.sessions.toLocaleString()} sessions yet, so there is nothing to report — install
          the Claude Code hooks to start recording this.
        </div>
      ) : (
        <CorpusProfile corpus={data} />
      )}
    </Card>
  );
}

/// The measured half of [`TroubleProfileCard`].
function CorpusProfile({ corpus }: { corpus: ClaudeCorpus }) {
  if (corpus.observation.state === "unobserved") return null;
  const p: ClaudeProfile = corpus.observation.profile;
  const floor = corpus.observation.state === "partial";
  const empty =
    p.turn_failures.length === 0 && p.tool_failures.length === 0 && p.denials.length === 0;

  return (
    <>
      {/* The denominator, always, whenever it is short of the whole. A
          reader who cannot see that 1,458 sessions predate the hooks will
          read the profile as covering everything. */}
      {corpus.sessions_observed < corpus.sessions ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          Over the {corpus.sessions_observed.toLocaleString()} of{" "}
          {corpus.sessions.toLocaleString()} sessions a hook has observed. The rest ran before
          the hooks were installed and cannot be reported on.
        </p>
      ) : null}
      {floor ? (
        <p className="mt-2 text-xs text-[#d29922]">
          At least these — {corpus.observation.state === "partial"
            ? corpus.observation.missing.join(" and ")
            : ""}{" "}
          not installed, so anything they would have recorded is missing.
        </p>
      ) : null}

      {empty ? (
        /* A measured zero, and good news. Reachable only when something
           WAS observed, so it is allowed to say nothing went wrong. */
        <div className="py-6 text-center text-sm text-[#8b949e]">
          Nothing failed and nothing was declined across the{" "}
          {corpus.sessions_observed.toLocaleString()} observed{" "}
          {corpus.sessions_observed === 1 ? "session" : "sessions"}.
        </div>
      ) : (
        <p className="mt-2 text-xs text-[#8b949e]">
          {corpus.sessions_with_events.toLocaleString()} of{" "}
          {corpus.sessions_observed.toLocaleString()} observed{" "}
          {corpus.sessions_observed === 1 ? "session" : "sessions"} recorded something.
        </p>
      )}

      {p.turn_failures.length > 0 ? (
        <CorpusTallies label="Turns that died" tallies={p.turn_failures} />
      ) : null}
      {p.tool_failures.length > 0 ? (
        <CorpusTallies label="Tool failures" tallies={p.tool_failures} />
      ) : null}
      {p.denials.length > 0 ? (
        /* Guardrail, not damage (#1064). The heading and the sentence both
           carry that: the same denial forty times is worth a look, and it
           is worth a look because it may be a workflow being blocked, not
           because something broke. */
        <CorpusTallies
          label="Declined by auto mode"
          note="Auto mode refused these. A denial repeated many times is either a guardrail earning its keep or a workflow being quietly blocked."
          tallies={p.denials}
        />
      ) : null}
    </>
  );
}

/// One named breakdown on the overview.
///
/// Names render VERBATIM, never bucketed into "other": an error type or
/// an MCP tool name this build has never seen is exactly the case the
/// record exists to surface (#1062, #1063).
function CorpusTallies({
  label,
  note,
  tallies,
}: {
  label: string;
  note?: string;
  tallies: ClaudeTally[];
}) {
  return (
    <div className="mt-3">
      <div className="text-xs font-semibold text-[#e6edf3]">{label}</div>
      {note !== undefined ? <p className="mt-0.5 text-xs text-[#8b949e]">{note}</p> : null}
      <ul className="mt-1.5 flex flex-col divide-y divide-[#30363d]">
        {tallies.map((t) => (
          <li
            key={`${label}:${t.name ?? "\u0000"}`}
            className="flex flex-wrap items-baseline justify-between gap-x-3 py-1.5 text-xs"
          >
            <span className="min-w-0 break-all text-[#e6edf3]">
              {t.name ?? "Not recorded"}
            </span>
            <span className="shrink-0 tabular-nums text-[#8b949e]">
              {t.count.toLocaleString()}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/// One row of the resumable list.
///
/// The action is a COPY of `cd <dir> && claude --resume <id>`, never a
/// GUESSED terminal. That is the house answer and it is already argued in
/// `claudify_command`: macOS has no default-terminal concept at all, so
/// there is no way to know whether to open Terminal.app or iTerm, and on
/// Linux `x-terminal-emulator` is Debian-only. The clipboard works
/// identically everywhere and lands the user in their OWN shell.
///
/// This overview keeps the copy, deliberately, even though #1126 gave
/// the session DETAIL a launch button: the detail's button acts on the
/// one session you opened, while this is a summary row, and launching a
/// terminal from a glanceable list is a bigger gesture than the surface
/// implies. Nothing stops it later; it is simply not what this page is
/// for.
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

/// Token usage across every measured session (#1134).
///
/// Usage could only be seen one session at a time, so "what did this
/// week cost me" and "which directory consumes the most output" had no
/// answer short of opening 1,500 sessions.
///
/// TOKENS, NOT DOLLARS. `usage.rs` rules out a rate table: rates change,
/// this app cannot keep a hardcoded one true, and a quietly wrong cost
/// with a currency symbol in front of it is the confident-wrong-answer
/// failure at its worst. That decision stands here.
function UsageProfileCard() {
  const { data, isError } = useClaudeUsageProfile();

  // A failed read renders nothing rather than zeros. A card of "0
  // tokens" is indistinguishable from a quiet month, and on this page
  // that argues for a conclusion nobody measured.
  if (isError || !data) return null;
  if (data.sessionsMeasured === 0) return null;

  const n = (v: number) => v.toLocaleString();
  // The floor idiom this codebase already uses everywhere a measurement
  // is short (`ArtifactsPage`, `WorktreesPage`, `ClaudeMdPage`).
  const qualify = (v: number) =>
    data.sessionsTruncated > 0 ? `at least ${n(v)}` : n(v);

  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">Tokens across your sessions</div>
      <div className="text-xs text-[#8b949e]">
        {/* The DENOMINATOR, always. A total is only as good as what it
            covers, and saying so is what makes the number usable. */}
        summed over {n(data.sessionsMeasured)} measured session
        {data.sessionsMeasured === 1 ? "" : "s"}
        {data.sessionsTruncated > 0
          ? ` — ${n(data.sessionsTruncated)} stopped at the read budget, so these are floors`
          : ""}
      </div>

      <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-xs sm:grid-cols-4">
        <div>
          <dt className="text-[#8b949e]">Output</dt>
          <dd className="text-[#e6edf3]">{qualify(data.outputTokens)}</dd>
        </div>
        <div>
          <dt className="text-[#8b949e]">Input</dt>
          <dd className="text-[#e6edf3]">{qualify(data.inputTokens)}</dd>
        </div>
        <div>
          <dt className="text-[#8b949e]">Cache read</dt>
          <dd className="text-[#e6edf3]">{qualify(data.cacheReadTokens)}</dd>
        </div>
        <div>
          <dt className="text-[#8b949e]">Messages</dt>
          <dd className="text-[#e6edf3]">{qualify(data.messages)}</dd>
        </div>
      </dl>

      {data.models.length > 0 && (
        <p className="mt-2 text-xs text-[#8b949e]">
          {/* Per MESSAGE, because the corpus is mixed: "which model was
              this" has no single answer, which `usage.rs` measured. */}
          {data.models.map((m) => `${m.model} (${n(m.messages)})`).join(" · ")}
        </p>
      )}

      {data.byDirectory.length > 0 && (
        <div className="mt-2">
          <p className="text-xs text-[#8b949e]">Heaviest directories, by output</p>
          <ul className="mt-1 space-y-0.5">
            {data.byDirectory.map((d) => (
              <li key={d.cwd} className="flex justify-between gap-2 text-xs">
                <span className="truncate text-[#e6edf3]">{d.cwd}</span>
                <span className="shrink-0 text-[#8b949e]">{n(d.outputTokens)}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Card>
  );
}
