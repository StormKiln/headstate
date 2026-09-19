import { useMemo, useState } from "react";
import {
  NET_PROCESSES_POLL_MS,
  NET_PROCESSES_SAMPLE_MS,
  useNetworkProcesses,
  useSystemFootprint,
  useSystemHealth,
  useHealthAlerts,
  useBackgroundHealth,
  useSystemHealthHistory,
  useUiPrefs,
} from "@/api/hooks";
import { QueryError, errorMessage } from "./QueryError";
import { formatSize } from "@/lib/worktrees";
import {
  CAPACITY_BAR_MAX,
  NAMEPLATE_MARK,
  THERMAL_MEANING,
  barColor,
  capacityBarFill,
  capacityColor,
  capacityMeaning,
  cycleMeaning,
  formatRate,
  formatUptime,
  formatWatts,
  interfaceRates,
  netProcessRates,
  peakRate,
  peakWatts,
  percentOf,
  powerColor,
  powerDirection,
  powerSeries,
  splitOnGaps,
  thermalColor,
  type NetProcessReading,
  type Point,
  type RatePoint,
} from "@/lib/health";
import type {
  FootprintProcess,
  FootprintProcessGroup,
  HealthGpu,
  HealthPowerFlow,
  AlertReport,
  HealthSample,
} from "@/types/pr";
import { toast } from "sonner";
import { revealLog } from "@/api/tauri";
import { LogPanel } from "./LogPanel";
import { BackgroundHealthNotice } from "./BackgroundHealthNotice";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { useConnectionState } from "@/api/connection";
import { type HealthPage, type View, useFilters } from "@/store/filters";
import { healthPagesFor } from "./SystemHealthSidebar";
import { useIsMobile } from "@/lib/useIsMobile";
import { ChevronLeft } from "lucide-react";

/// The machine's health: what it is doing now, and the last 24 hours.
///
/// # This view has no repository
///
/// Every other view is scoped by the repo the sidebar selected. This one
/// describes the MACHINE, so a repo picker beside it would be a control
/// that changes nothing -- see `App.tsx`, which renders no sidebar repo
/// list and no `FilterBar` for it.
///
/// # Whose machine
///
/// On the phone this view describes the PAIRED DESKTOP, never the phone
/// itself. A companion reporting its own battery would be answering a
/// question nobody asked: the user opened it to check on the machine
/// they left running.
///
/// The wording is switched on `IS_MOBILE_BUILD`, not on `useIsMobile()`.
/// Which machine is being described is a fact about the BUILD, and
/// `useIsMobile()` is also true for a narrow desktop window -- where a
/// page headed "the desktop's health" would be describing the very
/// machine it is running on. See the ground rules on #590.
///
/// # Two rules the panels below exist to keep
///
/// 1. **Absent is not zero.** Every optional field on the Rust `Sample`
///    is `null` when the platform does not expose it, and this file
///    renders every one of those as "Not measured". A 0 in that slot
///    would be a measurement the app never took, which is the same
///    class of lie as reporting "no updates" for a check that failed
///    to run.
/// 2. **Gaps are gaps.** The sampler only runs while the app is open,
///    so the series has holes. `splitOnGaps` (in `lib/health`) breaks
///    the line across them rather than joining two points hours apart,
///    because the whole value of a 24-hour chart is being able to trust
///    the shape of the line.
///
/// The pure logic behind both rules lives in `@/lib/health`, so it is
/// testable without rendering; this file is the layout.

/// Nothing measured, said the same way everywhere.
///
/// One component rather than a repeated string so that the phrase, the
/// colour, and the fact that it is NOT a number are decided in one
/// place. Grey, deliberately: an absent reading is not a warning, and
/// amber would tell the user to act on something the app simply did
/// not look at.
function NotMeasured({ children }: { children?: React.ReactNode }) {
  return (
    <span className="text-[#8b949e]">
      Not measured{children ? <span className="ml-1">{children}</span> : null}
    </span>
  );
}

/// A labelled figure, with the absent case handled once.
function Stat({
  label,
  value,
  hint,
}: {
  label: string;
  /// `null` means "the platform did not report this", and renders as
  /// `NotMeasured`. It is never coerced to 0.
  value: string | null;
  hint?: string;
}) {
  return (
    <div className="min-w-24">
      <div className="text-xs text-[#8b949e]">{label}</div>
      <div className="text-sm font-medium tabular-nums text-[#e6edf3]">
        {value === null ? <NotMeasured /> : value}
      </div>
      {hint ? <div className="text-xs text-[#8b949e]">{hint}</div> : null}
    </div>
  );
}

/// A horizontal utilisation bar.
///
/// `percent` is clamped rather than trusted: a disk can report more
/// used than total during a snapshot, and a bar wider than its track
/// would break the layout for a rounding artefact.
///
/// # `null` is not 0 (#960)
///
/// `percentOf` returns `null` when the platform did not report a usable
/// total -- an unusual filesystem, a container mount, a machine with swap
/// disabled, a read the collector could not complete. Six call sites used
/// to launder that with `?? 0`, and the result was a full-width bar
/// sitting EMPTY and GREEN (`barColor(0)` is the healthy tone) directly
/// above its own row's "Not measured" label: the two halves of one row
/// disagreeing, with the wrong half the one the eye reads first.
///
/// Worse for assistive tech, because `role="meter"` made it an
/// affirmative numeric claim: a screen reader was told the value IS 0.
/// So `aria-valuenow` is OMITTED for `null` and `aria-valuetext` says
/// "Not measured" instead -- a meter that expresses absence rather than
/// zero.
///
/// Grey and unfilled, not amber: the same reasoning `NotMeasured`'s doc
/// gives -- "an absent reading is not a warning, and amber would tell the
/// user to act on something the app simply did not look at." And the bar
/// is still RENDERED rather than hidden, so a row does not silently lose
/// its layout when a sibling `Stat` is healthy.
///
/// A genuine `0` still renders as a real, measured 0% -- distinct from
/// `null`, and with the clamp intact.
function Bar({ percent, label }: { percent: number | null; label?: string }) {
  if (percent === null) {
    return (
      <div
        className="h-2 w-full overflow-hidden rounded-full bg-[#30363d]"
        role="meter"
        // No `aria-valuenow`: the meter has no value, and asserting 0
        // is the bug. `aria-valuetext` is how a meter says "unknown".
        aria-valuetext="Not measured"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={label}
      />
    );
  }
  const clamped = Math.max(0, Math.min(100, percent));
  return (
    <div
      className="h-2 w-full overflow-hidden rounded-full bg-[#30363d]"
      role="meter"
      aria-valuenow={Math.round(clamped)}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={label}
    >
      <div
        className="h-full rounded-full"
        style={{ width: `${clamped}%`, backgroundColor: barColor(clamped) }}
      />
    </div>
  );
}

/// A panel: the repeated card shell, so the five below cannot drift.
function Panel({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-4">
      <h2 className="text-sm font-semibold text-[#e6edf3]">{title}</h2>
      {subtitle ? <p className="mt-0.5 text-xs text-[#8b949e]">{subtitle}</p> : null}
      <div className="mt-3">{children}</div>
    </section>
  );
}

/// A 24-hour sparkline that draws its gaps as gaps.
///
/// Hand-rolled SVG rather than the recharts setup `stats/ActivityChart`
/// uses. Recharts joins a series across a `null` unless every gap is
/// materialised as an explicit null-valued row, and the series here is
/// not evenly spaced at all -- the x axis is real time, and the gaps
/// are the point. One polyline per measured run says exactly that, with
/// no library configuration standing between the data and the picture.
function Sparkline({
  points,
  max,
  label,
  now,
  color = "#58a6ff",
}: {
  points: Point[];
  /// The top of the y axis. Passed in rather than derived so a CPU
  /// chart is always 0-100 and does not silently rescale to make a 4%
  /// blip look like a spike.
  max: number;
  label: string;
  /// The right-hand edge of the 24-hour window, in epoch ms.
  ///
  /// Passed in rather than read from `Date.now()` here, for two
  /// reasons. It keeps this a pure function of its props -- the lint
  /// rule that caught it is right, since a re-render would otherwise
  /// shift the axis under an unchanged series. And it lets the caller
  /// anchor the window on the LIVE SAMPLE's own timestamp, so the
  /// chart's "now" is the moment the app actually measured rather than
  /// the moment React happened to paint.
  now: number;
  color?: string;
}) {
  const runs = useMemo(() => splitOnGaps(points), [points]);
  const measured = runs.reduce((n, r) => n + r.length, 0);

  if (measured === 0) {
    return (
      <div
        className="flex h-16 items-center justify-center rounded border border-dashed border-[#30363d] text-xs"
        role="img"
        aria-label={`${label}: no history yet`}
      >
        <NotMeasured>— no history yet</NotMeasured>
      </div>
    );
  }

  // A fixed viewBox with `preserveAspectRatio="none"`: the chart
  // stretches to whatever width the panel has, and the maths below
  // stays in one coordinate system.
  const W = 300;
  const H = 48;
  // The window is always the full 24 hours the backend retains, not
  // the extent of the data. Otherwise ten minutes of history would
  // stretch across the whole width and read as a day of it -- and the
  // gap at the start, which is exactly "we were not running", would
  // vanish by rescaling.
  //
  // Anchored on `now` rather than on the newest point, so a series that
  // stops six hours ago sits at the LEFT of the window with six hours
  // of empty band after it. Ending the axis at the last point would
  // push that stretch off the chart entirely -- and "we have not
  // measured anything since lunchtime" is precisely the fact this view
  // must not hide.
  const end = now;
  const start = end - 24 * 60 * 60 * 1000;
  const span = end - start;
  const x = (t: number) => ((t - start) / span) * W;
  const y = (v: number) => H - (Math.max(0, Math.min(max, v)) / max) * H;

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      className="h-16 w-full"
      role="img"
      aria-label={`${label} over the last 24 hours, ${runs.length} measured ${
        runs.length === 1 ? "period" : "periods"
      }`}
      data-testid={`sparkline-${label}`}
      // Read by the test, and by anyone wondering whether a break in
      // the line is real: it is the count of separate runs the data
      // actually contains.
      data-runs={runs.length}
    >
      {/* A faint band behind the whole window, so a short line reads as
          "we measured this much of the day" rather than as the whole
          day. The unmeasured stretches are the band showing through --
          the gap is visible as absence, which is what it is. */}
      <rect x={0} y={0} width={W} height={H} fill="#0d1117" />
      {runs.map((run) => {
        const key = `${run[0].t}-${run[run.length - 1].t}`;
        if (run.length === 1) {
          // One point is not a line. Drawn as a dot rather than dropped:
          // a single measurement between two long closures is real, and
          // silently discarding it would under-report the history.
          return (
            <circle
              key={key}
              cx={x(run[0].t)}
              cy={y(run[0].v as number)}
              r={1.5}
              fill={color}
            />
          );
        }
        return (
          <polyline
            key={key}
            fill="none"
            stroke={color}
            strokeWidth={1.5}
            vectorEffect="non-scaling-stroke"
            points={run.map((p) => `${x(p.t)},${y(p.v as number)}`).join(" ")}
          />
        );
      })}
    </svg>
  );
}

/// One process, as the CPU and Memory detail pages list it.
///
/// CPU is NOT clamped to 100 and NOT drawn as a `Bar`, unlike every
/// other percentage on this page. `FootprintProcess.cpu_percent` is a
/// share of one core, so a process using three of them legitimately
/// reads 280% -- a bar would peg at full and hide exactly the process
/// worth seeing, and clamping the number would report a busy process as
/// a merely saturated one.
///
/// Rendered by the individual (un-grouped) table on both detail pages;
/// `ProcessGroupRow` is its grouped counterpart and is deliberately a
/// second component rather than a mode of this one -- see there. The
/// two tables there carry their own `<thead>` rather than sharing one,
/// because a group has no PID column (#721).
function ProcessRow({ p, hint }: { p: FootprintProcess; hint?: string }) {
  return (
    <tr className="border-t border-[#30363d]">
      {/* `max-w-0` is what makes the name column give (#973). A table
          cell's width is content-driven, so a bare `truncate` on a
          `<td>` does nothing at all -- `max-w-0` is the idiom the
          network process table two panels down this same file already
          uses (`:2283`), and it is the one these two tables did not
          receive. Without it the widest real process name on this
          machine, `com.apple.MobileSoftwareUpdate.CleanupPreparePathService`,
          is 407.8px at `text-sm`: a 533.6px table in a 326px panel,
          207.6px of overflow, and because nothing contained it the
          WHOLE PAGE scrolled sideways and the PID/CPU/Memory columns --
          the answer to "what is eating my CPU" -- sat off-screen.

          `md:truncate` rather than a bare `truncate`, so the name
          WRAPS below `md` and is truncated at or above it. The `title`
          below recovers a truncated name with a hover, but a hover is
          unreachable on touch and #973's whole subject is the phone --
          so the phone gets the name in full over two lines instead of
          an ellipsis it cannot open. `md` is Tailwind's 768px, which is
          `MOBILE_BREAKPOINT` by construction (see `useIsMobile`), so
          the two cannot disagree about what "narrow" means and this
          needs no hook: it is one column's wrapping, not a layout
          fork. `break-words` lets a 56-character dotted bundle id
          break at all -- it contains no spaces, so it would otherwise
          refuse to wrap and push the table wide again. */}
      <td className="max-w-0 py-1 pr-2 break-words md:truncate" title={p.name}>
        <span className="text-[#e6edf3]">{p.name}</span>
        {hint ? <span className="ml-2 text-xs text-[#8b949e]">{hint}</span> : null}
      </td>
      {/* The PID is here because this panel's job ends at naming the
          cost: a user who wants to know what a 900%-CPU `git` is doing
          needs something to type into `ps`, and the name alone does not
          identify one of forty. */}
      {/* And it is COPYABLE as of #943, because the sentence above says
          the reader is going to type it into `ps` -- and a number that has
          to be retyped is the workflow the comment describes rather than
          the one it intends. `p.pid` is always present on a footprint row;
          the absent case lives on the network table, which has one. */}
      <td className="py-1 pr-2 text-right text-xs tabular-nums text-[#8b949e]">
        <CopyPid pid={p.pid} />
      </td>
      <td className="py-1 pr-2 text-right tabular-nums text-[#e6edf3]">
        {p.cpu_percent.toFixed(0)}%
      </td>
      <td className="py-1 text-right tabular-nums text-[#e6edf3]">
        {formatSize(p.memory)}
      </td>
    </tr>
  );
}

/// One at-a-glance pressure reading, above the panels.
///
/// # Why a number AND a bar
///
/// The bar answers "roughly how full" at a glance, which is the whole
/// point of this row; the digits answer "how full", which is what
/// someone reaches for the moment the bar looks interesting. Showing
/// only the bar makes the second question require scrolling to a panel,
/// and showing only the number makes the first question require
/// reading. They cost the same together.
///
/// # Colour is never the only cue
///
/// `barColor`'s three bands are the same ones the panels below use, so
/// the row and the detail cannot disagree about what counts as
/// trouble. But the percentage is always rendered as text, so a reader
/// who cannot distinguish the bands loses nothing -- which is what
/// #581/#582 ask for generally, applied here because this row is the
/// most prominent thing on the page.
///
/// # A missing reading is not a zero
///
/// `percent` is `null` when the platform did not report the underlying
/// figure, and this renders "Not measured" with no bar at all. The
/// alternative -- a confident green 0% -- would be the page's own rule
/// broken in the place people look first.
/// The conditions that are true right now, above the numbers (#864).
///
/// # Why this panel exists
///
/// Twelve orphaned busy-loops ran 8.5 hours at ~50% of a core each, load
/// average 53, and System Health showed nothing. The rules had evaluated
/// them correctly the whole time: `health_alerts` returns every current
/// condition, and no TypeScript called it. The page's own comment further
/// down assumed the other half of the design existed -- "alert fires, the
/// user opens this page, and the number that answers why should be
/// [there]" -- so the page was built to EXPLAIN an alert it could never
/// show.
///
/// # Silence and not-watching are different answers
///
/// `lastSampleAgoMs` is the point of the second branch. A health tool
/// only samples while it is running, so "no conditions are true" and "we
/// have not looked since yesterday" are opposite claims that would
/// otherwise render identically as an empty panel. The overnight runaway
/// is exactly the case where the app may have been closed, so the gap is
/// the MORE likely of the two and must not be reported as good news.
/// That is the rule the rest of this codebase states as absent-is-not-zero
/// (#769, #841): a reading we could not take is never evidence of health.
///
/// The staleness bar is deliberately generous. The sampler runs at 60s,
/// and a missed poll or two is normal; ten minutes means something
/// stopped.
const SAMPLE_STALE_MS = 10 * 60 * 1000;

/// A pid, and a button that puts it on the clipboard (#943).
///
/// # Why this exists
///
/// Three separate comments in this file already named copy-into-a-shell as
/// the intended workflow -- the process table's PID column is "the column a
/// reader copies into `ps`", the network table reasons about a reader
/// pasting a pid "into `kill`", and the CPU watch notice ends "Worth a
/// look" -- and none of the three provided a copy affordance. The number
/// was bare text in a `<td>`, so the workflow the app expected was: read a
/// number off the screen, retype it into a terminal.
///
/// `copyText` rather than `navigator.clipboard.writeText`, because the
/// naive spelling has a failure mode that produces NO feedback at all: the
/// property access throws synchronously in an insecure context, before
/// either handler is attached, so the click does nothing visible. That is
/// the bug that made an earlier copy button look inert.
///
/// # Surface class: none
///
/// No Tauri command is involved at all -- this is the browser clipboard --
/// so it is not gated and works identically on the phone. Copying a pid
/// there is genuinely useful: it is what a user forwards to themselves, or
/// reads out, when the Mac holding the process is in another room.
///
/// # Never rendered for an absent pid
///
/// The caller decides. A network row whose label carried no parseable pid
/// renders `NotMeasured` and no button, and a grouped process row has no
/// PID column to attach one to; a collapsed health notice carries `null`
/// and gets nothing. A disabled button offering to copy a dash would be a
/// control whose only outcome is putting "—" on the clipboard.
function CopyPid({ pid }: { pid: number }) {
  const copy = async () => {
    const failure = await copyText(String(pid));
    if (failure) {
      // The REASON. An insecure context is a configuration and retrying
      // will not help; a rejected write usually means the window is not
      // focused, which the user can fix. "Could not copy" covers both and
      // helps with neither.
      toast.error(`Could not copy the PID: ${failure}`);
      return;
    }
    toast.success(`Copied PID ${pid}.`);
  };

  return (
    <button
      type="button"
      onClick={() => void copy()}
      // The number is the label, so a screen reader hears the pid rather
      // than "copy button" forty times down a table -- and `aria-label`
      // says what pressing it does, which the digits alone do not.
      aria-label={`Copy PID ${pid}`}
      title={`Copy PID ${pid}`}
      className="tap-target rounded px-1 tabular-nums text-[#58a6ff] hover:bg-[#21262d]"
    >
      {pid}
    </button>
  );
}

/// The retry on the two 24-hour-history failure states (#946).
///
/// One component for both because the two states say different sentences
/// about the same rejected query -- "the charts above are empty" on the
/// overview, "any chart here is empty" on a detail page -- and the CONTROL
/// is the same control. Two copies of it would be two chances for one of
/// them to be dropped, which is how there came to be four failure states
/// with no retry while twenty `QueryError` sites all had one.
///
/// Deliberately not `QueryError`: these two states are a line of amber
/// prose beside charts that are merely empty, not a panel replacing a
/// failed read. `QueryError` is a full-width red alert and would make a
/// missing chart look like a broken page -- the current readings above it
/// are fine, which is the whole point of both sentences.
///
/// **Surface class: none.** `system_health_history` is `Read`, so the phone
/// retries exactly as the desktop does.
function HistoryRetry({ onRetry }: { onRetry: () => void }) {
  return (
    <button
      type="button"
      onClick={onRetry}
      className="tap-target ml-2 rounded border border-[#d29922]/40 px-2 py-0.5 text-xs text-[#d29922] hover:bg-[#d29922]/10"
    >
      Try again
    </button>
  );
}

/// The retry on the process-table and network-table failure lines (#946).
///
/// These three states are a bare `<p>` inside a panel whose heading has
/// already said what the panel is for, so the retry is a text button on the
/// same line rather than a bordered one: a panel-sized `QueryError` here
/// would replace a heading that is still accurate.
///
/// # Why this is a retry and `PartialScanNotice` is not
///
/// A partial scan deliberately offers nothing to press, because a second
/// identical walk will not read what the first could not -- the refusal is
/// a fact about the filesystem, not about the attempt. These three are
/// query REJECTIONS: `system_footprint` and `system_network_processes`
/// either answered or did not, and asking again is the whole remedy. The
/// distinction is why this component is separate from that one rather than
/// a shared "something went wrong" affordance.
///
/// **Surface class: none.** Both commands are already `Read`.
///
/// The network one is the highest-value retry in the view and the one that
/// was most conspicuously missing: that read costs ~5 seconds of `nettop`,
/// which is why its poll cadence is deliberately slow -- and a slow
/// cadence is exactly what makes a manual retry worth the most. It does NOT
/// shorten that cadence; `surface.rs` is explicit that the phone must call
/// it "on the Network page's slow cadence only, never beside the health
/// poll", and a one-off re-issue at the user's request is not a cadence.
function InlineRetry({ onRetry }: { onRetry: () => void }) {
  return (
    <button
      type="button"
      onClick={onRetry}
      className="tap-target ml-2 rounded px-1 text-sm text-[#58a6ff] hover:bg-[#161b22]"
    >
      Try again
    </button>
  );
}

/// Exported for `SystemHealthPage.stale.test.tsx`, which asserts which
/// of two CAUSES the stale banner names (#1145). Reaching that arm
/// through the page means driving four queries into a specific
/// combination of states; the component is pure and takes the one fact
/// under test as a prop.
export function HealthConditions({
  alerts,
  failed,
  error,
  onRetry,
  sampledAt,
  now,
  samplerDegraded = false,
}: {
  alerts: AlertReport[] | undefined;
  failed: boolean;
  /// Whether the sampler itself is failing (#1145).
  ///
  /// Decides which of TWO causes the stale banner names. An old last
  /// reading means either the app was closed or the sampler ran and
  /// could not write, and the banner asserted the first
  /// unconditionally. Defaults false, which keeps the previous wording
  /// when the answer is not known -- the honest fallback, since "the
  /// app was not watching" is the far more common cause.
  samplerDegraded?: boolean;
  /// The rejection, for the failed arm (#946). Threaded down rather than
  /// dropped: "the rules did not run" is the state where the app says its
  /// own verdicts are untrustworthy, and it was the only one of the four
  /// that did not even show what went wrong.
  error: unknown;
  /// The query's own `refetch`. In scope at the parent the whole time and
  /// simply never wired to a control -- so the user's recourse was to wait
  /// out the poll, leave the view and come back, or restart the app.
  ///
  /// A user-initiated one-off. It does NOT shorten `refetchInterval`, which
  /// `useHealthAlerts` cadences to the history deliberately because "the
  /// rules read history, so asking faster than the series grows re-derives
  /// an identical answer".
  onRetry: () => void;
  /// The live sample's own timestamp, epoch ms.
  sampledAt: number;
  /// The right-hand edge of "recent", epoch ms.
  ///
  /// Passed in rather than read from `Date.now()` here, for exactly the
  /// reason `Sparkline` above states: it keeps this a pure function of
  /// its props, which the impure-render lint rule is right to insist on.
  /// The caller resolves it once per poll.
  now: number;
}) {
  // A failed read is not "nothing is wrong". Same rule as the panel
  // below it, and the reason this is a branch rather than `?? []`.
  if (failed) {
    return (
      <div
        className="rounded-md border border-[#f85149]/40 bg-[#f85149]/5 px-3 py-2 text-xs text-[#f85149]"
        role="status"
      >
        {/* VERBATIM (#946). "This is not a clean result — the rules did
            not run" is load-bearing: it is the sentence that stops a
            failed evaluation reading like a clean bill of health, and a
            retry button is an addition beside it rather than a reason to
            reword it. */}
        Could not check for health conditions. This is not a clean result — the rules did not
        run.
        {/* The REASON, which this state alone was not showing -- the one
            state where the app is telling the user its verdicts are
            untrustworthy was also the one with the least to go on. */}
        {errorMessage(error) ? (
          <span className="mt-1 block text-[#8b949e]">{errorMessage(error)}</span>
        ) : null}
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={onRetry}
            className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-[#e6edf3] hover:bg-[#21262d]"
          >
            Try again
          </button>
          {/* The log, on this state only (#946). This is precisely where
              the cause would be written and the view has never offered
              it: `reveal_log` has exactly one caller in the whole
              frontend, behind Settings' "Show the log".

              `reveal_log` is `Class::Local`, so this is gated on
              `IS_MOBILE_BUILD` the way `ClaudeCodePage`'s reveal buttons
              are -- and the sentence in its place says whose Finder it
              would need, rather than leaving a reader unable to tell
              "there is no log" from "not from here". */}
          {IS_MOBILE_BUILD ? (
            // No reveal button -- `reveal_log` is still `Class::Local`
            // and there is still no Finder here. What changed is that
            // the apology is no longer the whole offer: `LogPanel`
            // below is `Class::Read` and shows the text, which is what
            // a phone diagnosing a failure actually needed (#1147).
            <span className="text-[#8b949e]">
              The desktop&rsquo;s diagnostic log cannot be opened from here — there is no
              Finder on this device. It can be read below.
            </span>
          ) : (
            <button
              type="button"
              onClick={() => {
                void revealLog().then(
                  (path) => toast.success("Showed the log", { description: path }),
                  (e: unknown) =>
                    toast.error("Could not show the log", {
                      description: typeof e === "string" ? e : undefined,
                    }),
                );
              }}
              className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-[#e6edf3] hover:bg-[#21262d]"
            >
              Show the log
            </button>
          )}
        </div>
        {/* The log itself, on the same state that offers to reveal it
            (#946, #1147). This is precisely where the cause is written,
            and until now the view could only point at a file. */}
        <LogPanel />
      </div>
    );
  }

  const lastSampleAgoMs = now - sampledAt;
  const stale = lastSampleAgoMs > SAMPLE_STALE_MS;

  if (alerts && alerts.length === 0 && stale) {
    return (
      <div
        className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2 text-xs text-[#d29922]"
        role="status"
      >
        No conditions found, but the last reading is{" "}
        {Math.round(lastSampleAgoMs / 60_000)} minutes old, so a problem in the gap would
        not have been seen.{" "}
        {/* "The app was not watching" was asserted unconditionally and
            is one of TWO possible causes (#1145). The sampler logs a
            failed write and carries on, so an identical gap is produced
            by an app that ran the whole time and could not record. The
            page cannot tell from the timestamp -- but `background_health`
            can, so the cause is stated only when it is known. */}
        {samplerDegraded
          ? "The app was running: the sampler could not record, and the banner above says why."
          : "The app was not watching for most of that time."}
      </div>
    );
  }

  if (!alerts || alerts.length === 0) return null;

  return (
    <div className="flex flex-col gap-2" role="group" aria-label="Active health conditions">
      {alerts.map((a) => (
        <div
          key={a.key}
          className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2"
          role="status"
        >
          <p className="text-sm font-semibold text-[#d29922]">{a.title}</p>
          <p className="mt-0.5 text-xs text-[#8b949e]">{a.body}</p>
          {/* The pid the notice's own last sentence sends the reader
              looking for (#943). Absent rather than fabricated where the
              condition is about the machine, or about several processes of
              one name -- so a notice with no button is one that genuinely
              has no single process, not one whose button was forgotten.
              `CopyPid` states that case at length. */}
          {a.pid === null ? null : <CopyPid pid={a.pid} />}
        </div>
      ))}
    </div>
  );
}

function PressureCard({
  label,
  percent,
  detail,
  className,
}: {
  label: string;
  /// `null` when the platform did not report it. Never coerced to 0.
  percent: number | null;
  /// The absolute figures behind the percentage, e.g. "12.4 GB of 16 GB".
  /// A percentage alone cannot distinguish a nearly-full small disk
  /// from a nearly-full large one.
  detail?: string;
  /// Grid placement from the caller, for the odd card in a two-column
  /// phone row (#966). Placement only -- the card's own padding, border
  /// and colours are not the caller's to override, because the three
  /// cards reading as one row depends on their being identical.
  className?: string;
}) {
  return (
    <div
      className={`flex min-w-0 flex-col gap-1.5 rounded-md border border-[#30363d] bg-[#0d1117] px-3 py-2.5${
        className === undefined ? "" : ` ${className}`
      }`}
    >
      <span className="text-xs text-[#8b949e]">{label}</span>
      {percent === null ? (
        <>
          {/* No `truncate` and no `overflow-hidden`, deliberately:
              clipping this to "Not measu" would be worse than the wrap
              it replaces, and the phrase is not abbreviable -- the
              whole reason `NotMeasured` exists is that the words, the
              colour and the fact that it is NOT A NUMBER are decided in
              one place, so shortening it for layout reintroduces the
              unexplained dash. The fix for its 118.4px is the 151px
              content box the two-column row gives it (#966), not a
              shorter string. */}
          <span className="text-lg font-semibold text-[#8b949e]">Not measured</span>
          <div className="h-1.5" />
        </>
      ) : (
        <>
          <span className="text-lg font-semibold tabular-nums text-[#e6edf3]">
            {percent.toFixed(0)}%
          </span>
          <div
            className="h-1.5 overflow-hidden rounded-full bg-[#21262d]"
            role="img"
            aria-label={`${label}: ${percent.toFixed(0)} percent`}
          >
            <div
              className="h-full rounded-full"
              style={{
                width: `${Math.min(100, Math.max(0, percent))}%`,
                backgroundColor: barColor(percent),
              }}
            />
          </div>
        </>
      )}
      {detail ? <span className="text-[11px] text-[#8b949e]">{detail}</span> : null}
    </div>
  );
}

/// One GPU's readings.
///
/// Split out because a machine can have two -- an Intel Mac with
/// integrated and discrete graphics reports both -- and a panel that
/// assumed one would hide whichever it did not pick.
///
/// # Unified memory is stated, not implied
///
/// On Apple Silicon the GPU has no memory of its own: `memory_used` is
/// a share of the same physical pool the Memory panel above reports.
/// Left unsaid, a reader comparing the two panels would add the GPU's
/// gigabytes to the system's and conclude the machine has more RAM than
/// it does -- so the hint on the figure and the note under it both say
/// which pool this is. That is the specific misreading #686 asks to
/// prevent.
function GpuCard({ gpu }: { gpu: HealthGpu }) {
  const memPct =
    gpu.memory_used === null || gpu.memory_total === null
      ? null
      : percentOf(gpu.memory_used, gpu.memory_total);

  return (
    <div className="flex flex-col gap-2">
      <div className="text-sm font-medium text-[#e6edf3]">{gpu.name}</div>
      <div className="flex flex-wrap gap-6">
        {/* Null rather than 0 for a GPU that reported no utilization:
            an idle GPU and an unreadable one are opposite answers, and
            this is the field where they look most alike. */}
        <Stat
          label="Utilization"
          value={
            gpu.utilization_percent === null
              ? null
              : `${gpu.utilization_percent.toFixed(0)}%`
          }
        />
        <Stat
          label="Memory in use"
          value={gpu.memory_used === null ? null : formatSize(gpu.memory_used)}
          hint={gpu.unified_memory && gpu.memory_used !== null ? "Shared with system" : undefined}
        />
        <Stat
          label={gpu.unified_memory ? "Allocated pool" : "Total VRAM"}
          value={gpu.memory_total === null ? null : formatSize(gpu.memory_total)}
        />
      </div>
      {gpu.utilization_percent !== null ? (
        <Bar percent={gpu.utilization_percent} label={`${gpu.name} utilization`} />
      ) : null}
      {memPct !== null ? (
        <div className="text-xs text-[#8b949e]">
          {memPct.toFixed(0)}% of the pool currently allocated to the GPU
        </div>
      ) : null}
      {gpu.unified_memory ? (
        <p className="text-xs leading-relaxed text-[#8b949e]">
          This GPU uses <em>unified memory</em>: it shares one physical pool with
          the CPU rather than having its own. The figures above are a share of
          the same memory the Memory panel reports — not additional memory, so
          they should not be added to it.
        </p>
      ) : null}
    </div>
  );
}

export function SystemHealthPage() {
  // Both queries are enabled unconditionally HERE, because this
  // component only mounts while the view is open -- `App` renders it
  // for `view === "system-health"` and nothing else. That is what
  // stops the polling: the query unmounts, its observer goes, and the
  // interval with it. Gating inside the hook on a prop instead would
  // leave the timer running whenever the caller forgot to pass false.
  const live = useSystemHealth(true);
  const history = useSystemHealthHistory(true);
  const alerts = useHealthAlerts(true);
  // Whether the loops that PRODUCE everything on this page are
  // still working (#1145). The page reports the machine; this reports
  // whether the reporting itself is running.
  const background = useBackgroundHealth();
  // Which page of the view is open. Read HERE rather than in each
  // detail component so the live sample, the history and the error and
  // loading states are fetched once and shared: a drill-down that
  // mounted its own `useSystemHealth` would be a second observer on the
  // same query key, and switching pages would show a reading taken at a
  // different instant from the one the overview just showed.
  const healthPage = useFilters((f) => f.healthPage);

  // Who this page is about. On the desktop build it is always the
  // machine underfoot, so the connection state is not even consulted --
  // `useConnectionState` is `local` there by construction.
  const connection = useConnectionState();
  const desktopName =
    IS_MOBILE_BUILD && "desktop" in connection ? connection.desktop : null;
  const subject = IS_MOBILE_BUILD ? `${desktopName ?? "the desktop"}'s` : "this machine's";
  const unreachable = IS_MOBILE_BUILD && connection.kind === "unreachable";

  const samples = useMemo(() => history.data ?? [], [history.data]);
  const toPoint = (pick: (s: HealthSample) => number | null): Point[] =>
    samples.map((s) => ({ t: Date.parse(s.sampled_at), v: pick(s) }));

  // Built once per history change rather than inline, so the two charts
  // do not rebuild on every five-second live tick.
  const cpuSeries = useMemo(
    () => toPoint((s) => s.cpu_percent),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [samples],
  );
  const memSeries = useMemo(
    // Used as a PERCENTAGE of total, not raw bytes: the y axis is then
    // comparable across machines and across a chart whose total never
    // changes anyway.
    () => toPoint((s) => percentOf(s.memory.used, s.memory.total)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [samples],
  );
  // The FIRST GPU only. A second one would need its own series, and a
  // machine with two is rare enough that a chart per GPU is not worth
  // the vertical space -- the live figures above already cover both.
  //
  // A sample taken before this shipped, or on a platform that reports
  // no GPU, has no entry and contributes `null`, which `splitOnGaps`
  // breaks the line across. That is the correct reading: the app was
  // running and did not measure a GPU, which is not the same as a GPU
  // that was idle.
  const gpuSeries = useMemo(
    () => toPoint((s) => s.gpus[0]?.utilization_percent ?? null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [samples],
  );

  if (live.isError && live.data === undefined) {
    // Health is never served from the companion's cached snapshot the
    // way `get_cached` is -- there is no stored copy of a reading that
    // is only meaningful at the instant it was taken. So on the phone
    // an unreachable desktop arrives here, as a genuine error, and the
    // page says so instead of rendering panels of zeros. A zero in a
    // memory bar is a measurement; "not reachable" is the absence of
    // one, and this codebase does not let the two look alike.
    return (
      <QueryError
        title={
          unreachable
            ? `Cannot reach ${desktopName ?? "the desktop"}`
            : `Could not read ${subject} health`
        }
        message={
          unreachable
            ? "Its health can only be read while it is reachable, and nothing here is stored from before, so there is nothing to show."
            : errorMessage(live.error)
        }
        onRetry={() => void live.refetch()}
      />
    );
  }

  const s = live.data;
  if (!s) {
    // "Not looked yet" and "looked and found nothing" are opposite
    // answers, so the first load says it is loading rather than
    // rendering empty panels full of dashes.
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center text-sm text-[#8b949e]">
        Reading {subject} health…
      </div>
    );
  }

  const memUsedPct = percentOf(s.memory.used, s.memory.total);
  const swapPct = percentOf(s.memory.swap_used, s.memory.swap_total);
  // "Now" for everything time-relative on this page: the moment the
  // live sample was taken, which the Rust side stamps. Reading the
  // clock during render instead would make this component impure -- a
  // repaint triggered by anything at all would silently slide the
  // chart's axis and the boot time. Because the live query re-polls
  // every few seconds, this advances on its own anyway.
  const sampledAt = Date.parse(s.sampled_at);

  // "Now", without reading the clock during render (the rule `Sparkline`
  // above states). React Query stamps this when the live sample resolved,
  // so it advances once per poll rather than on every repaint -- which is
  // also the honest edge for "how old is the newest reading": the last
  // moment we actually heard from the sampler.
  //
  // When the query has never resolved, `dataUpdatedAt` is 0, and `s` is a
  // placeholder; falling back to `sampledAt` makes the age zero rather
  // than 56 years, so a cold start does not accuse the app of not
  // watching.
  const renderedAt = live.dataUpdatedAt || sampledAt;

  // The root volume for the pressure card. The Disk panel below still
  // lists every mount -- this row answers "is the machine about to run
  // out", and on every platform we support that question is about the
  // volume the OS is on. `find` rather than a sum across mounts: adding
  // an almost-full external drive to a roomy boot disk would produce a
  // percentage describing no actual volume.
  const rootDisk = s.disks.find((d) => d.is_root) ?? s.disks[0];
  const rootUsed =
    rootDisk === undefined
      ? null
      : percentOf(rootDisk.total - rootDisk.available, rootDisk.total);

  // The drill-downs (#687). Rendered INSTEAD of the overview, not
  // beside it: each one is the same class the overview summarises, at
  // the depth the overview cannot afford. Everything below this branch
  // is the overview exactly as it was -- the landing page is unchanged
  // by design, and these are additions reached from it.
  //
  // One of them is conditional. A machine with no discoverable GPU has
  // no GPU PAGE, not an empty one (#717) -- the same rule that decides
  // whether the overview draws a GPU panel, and the reason the sidebar
  // does not offer the row.
  //
  // Checked HERE as well as in the sidebar, because the sidebar filters
  // on the CURRENT sample and a GPU can leave one: an eGPU unplugged,
  // or the very first sample landing after the page was opened from a
  // stale cache. Falling through to the overview is the honest answer
  // -- there would be nothing truthful to put on the page, and an empty
  // one claims a device was found and could not be read.
  const offered = healthPagesFor(s.gpus.length).some((p) => p.id === healthPage);
  if (healthPage !== "overview" && offered) {
    return (
      <DetailPage
        page={healthPage}
        sample={s}
        samples={samples}
        sampledAt={sampledAt}
        historyFailed={history.isError}
        onRetryHistory={() => void history.refetch()}
        alerts={alerts.data}
        alertsFailed={alerts.isError}
        alertsError={alerts.error}
        onRetryAlerts={() => void alerts.refetch()}
        renderedAt={renderedAt}
      />
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {/* On the phone, the class list itself -- there is no persistent
          sidebar at this width, so the drill-downs need a door on the
          page they drill down from. Above the pressure cards because
          it is navigation: what the page can show, before what it is
          showing. Desktop renders nothing here; the sidebar has it. */}
      <HealthPageNav gpuCount={s.gpus.length} />
      {/* Above the pressure cards and the nav: a condition that is
          already true outranks the numbers it was derived from. #864. */}
      {/* ABOVE the conditions, deliberately. A failing sampler means
          the conditions below were computed from incomplete data, so a
          reader has to meet that fact before the verdicts it qualifies
          (#1145). Renders nothing while the loops are working. */}
      <BackgroundHealthNotice tasks={background.data} now={renderedAt} />
      <HealthConditions
        alerts={alerts.data}
        failed={alerts.isError}
        error={alerts.error}
        onRetry={() => void alerts.refetch()}
        sampledAt={sampledAt}
        now={renderedAt}
        samplerDegraded={
          background.data?.some((t) => t.task === "health-sampler" && t.degraded) ?? false
        }
      />
      {/* Whose machine, said once, at the top, on the phone only.
          `ConnectionBanner` already names the paired desktop, but it
          is chrome that sits above every view alike -- it says which
          desktop this app is talking to, not what the page under it
          is describing. On a page full of CPU and battery readings
          rendered on a device that has its own CPU and battery, that
          distinction is the whole point, so it is stated on the
          content rather than left to be inferred from the chrome.
          Not repeated per panel: eight panels each captioned with the
          same hostname is noise that stops being read. */}
      {IS_MOBILE_BUILD ? (
        <p className="text-xs text-[#8b949e]">
          Showing the health of{" "}
          <span className="text-[#c9d1d9]">{desktopName ?? "the paired desktop"}</span>, not
          this phone.
        </p>
      ) : null}
      {/* The question people open this page with, answered before
          anything has to be read. Everything below is unchanged; this
          is an addition, not a reorganisation (#683).

          # Two columns on a phone, three from `sm` up (#966)

          The goal this row was built for -- a glance that survives on
          the phone -- is right, and "stacking them would push the
          panels below the fold on exactly the device where a glance
          matters most" is still true, so `grid-cols-1` is NOT the fix
          here even though it is what every other grid in this file
          does.

          What was wrong was the premise that "these are short numbers".
          The numbers are short (`94%` is 36.2px); the CAPTIONS are not,
          and neither is the absence text. `grid-cols-3 gap-2` at a
          390px viewport gives (358 - 16) / 3 = 114px per track, and
          `PressureCard`'s `px-3` leaves a 90.0px content box. Measured
          against the shipped Geist face: the Disk caption
          `183 GB free of 494 GB` is 113.3px (over by 23px, so it wrapped
          to three lines) and `Not measured` at `text-lg` is 118.4px
          (over by 28px, so the calmest state in the row became its
          tallest cell). Three cells at three different heights is not a
          row, and a row is the one property this thing exists for.

          Two tracks give (358 - 8) / 2 = 175px, a 151px content box --
          which clears both of those with room, and clears the worse
          `1.2 TB free of 2.0 TB` (101.9px) too.

          `percent === null` is NOT exotic, which is why the absence
          width is sized for rather than treated as an edge case: it is
          the normal state for Disk on a machine with no root disk
          identified, and for CPU wherever `cpu_per_core` comes back
          empty (`health/collect.rs`). A first-run user on a non-macOS
          machine can see two of these three cards in that state at
          once. */}
      <div
        className="grid grid-cols-2 gap-2 sm:grid-cols-3 sm:gap-4"
        role="group"
        aria-label="System pressure at a glance"
      >
        <PressureCard
          label="CPU"
          percent={s.cpu_percent}
          detail={s.load ? `Load ${s.load[0].toFixed(2)}` : undefined}
        />
        <PressureCard
          label="Memory"
          percent={memUsedPct}
          detail={`${formatSize(s.memory.used)} of ${formatSize(s.memory.total)}`}
        />
        {/* The odd card spans the pair below `sm` rather than sitting
            alone in a half-width cell beside a gap. It is the card with
            the LONGEST caption of the three -- `183 GB free of 494 GB`
            -- so the full width goes to the cell that needs it most,
            and a two-plus-one arrangement still reads as one block
            above the panels rather than as a stack. Heights stay equal
            where it matters: the grid equalises each row, and CPU and
            Memory share theirs. */}
        <PressureCard
          className="col-span-2 sm:col-span-1"
          label="Disk"
          percent={rootUsed}
          detail={
            rootDisk === undefined
              ? undefined
              : `${formatSize(rootDisk.available)} free of ${formatSize(rootDisk.total)}`
          }
        />
      </div>

      {/* Two columns on a desktop, one on a phone. No media query
          needed: the grid does it, and every panel below is written
          to survive either width. */}
      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <Panel title="CPU" subtitle="Load averages, per-core use, and the last 24 hours">
          <div className="flex flex-wrap gap-6">
            {/* One Stat per window rather than "1.2 / 0.9 / 0.7":
                the three numbers answer different questions (is it
                busy NOW / has it been busy) and a slash-joined triple
                makes people count positions to read one of them. */}
            <Stat
              label="Load (1m)"
              value={s.load ? s.load[0].toFixed(2) : null}
            />
            <Stat label="Load (5m)" value={s.load ? s.load[1].toFixed(2) : null} />
            <Stat label="Load (15m)" value={s.load ? s.load[2].toFixed(2) : null} />
            <Stat
              label="CPU"
              value={s.cpu_percent === null ? null : `${s.cpu_percent.toFixed(0)}%`}
            />
          </div>
          {/* Windows reports no load average at all, so the note is
              shown rather than three silent dashes -- otherwise the
              absence looks like a bug in Headstate. */}
          {s.load === null ? (
            <p className="mt-2 text-xs text-[#8b949e]">
              This platform does not report load averages.
            </p>
          ) : null}

          <div className="mt-4">
            <div className="text-xs text-[#8b949e]">Per core</div>
            {s.cpu_per_core.length === 0 ? (
              <p className="mt-1 text-sm">
                <NotMeasured />
              </p>
            ) : (
              <div className="mt-2 flex flex-col gap-1">
                {s.cpu_per_core.map((v, i) => (
                  <div key={i} className="flex items-center gap-2">
                    <span className="w-10 shrink-0 text-xs tabular-nums text-[#8b949e]">
                      #{i}
                    </span>
                    <Bar percent={v} label={`Core ${i}`} />
                    <span className="w-10 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                      {v.toFixed(0)}%
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="mt-4">
            <div className="text-xs text-[#8b949e]">CPU over the last 24 hours</div>
            <Sparkline points={cpuSeries} max={100} label="CPU" now={sampledAt} />
            <GapNote />
          </div>
        </Panel>

        <Panel title="Memory" subtitle="What is in use, and what the system can still reclaim">
          <div className="flex flex-wrap gap-6">
            <Stat label="Used" value={formatSize(s.memory.used)} />
            {/* "Available" is NOT total minus used, and the hint says
                so: cache counts as used and is also reclaimable, so
                the two numbers do not add up to the total and a reader
                who assumes they do will think one of them is wrong. */}
            <Stat
              label="Available"
              value={formatSize(s.memory.available)}
              hint="Reclaimable, incl. cache"
            />
            <Stat label="Total" value={formatSize(s.memory.total)} />
            <Stat
              label="Pressure"
              value={memUsedPct === null ? null : `${memUsedPct.toFixed(0)}%`}
            />
          </div>
          <div className="mt-3">
            <Bar percent={memUsedPct} label="Memory used" />
          </div>

          <div className="mt-4 flex flex-wrap gap-6">
            {/* A machine with swap disabled reports a zero total, which
                is not "0% swap used" -- it is "there is no swap". */}
            <Stat
              label="Swap used"
              value={
                s.memory.swap_total === 0
                  ? null
                  : `${formatSize(s.memory.swap_used)} of ${formatSize(s.memory.swap_total)}`
              }
              hint={s.memory.swap_total === 0 ? "No swap configured" : undefined}
            />
            <Stat
              label="Swap"
              value={swapPct === null ? null : `${swapPct.toFixed(0)}%`}
            />
          </div>

          <div className="mt-4">
            <div className="text-xs text-[#8b949e]">Memory used over the last 24 hours</div>
            <Sparkline
              points={memSeries}
              max={100}
              label="Memory"
              color="#3fb950"
              now={sampledAt}
            />
            <GapNote />
          </div>
        </Panel>

        {/* No GPU panel at all when nothing was discoverable, rather
            than a panel of "Not measured" rows.

            This is the one place on the page where an empty panel
            would be worse than no panel. Elsewhere an absent reading
            sits beside present ones and "Not measured" is informative
            -- this machine has no battery, that platform has no load
            average. A GPU panel containing nothing but absences says
            "we found a GPU and could not read it", which on Windows
            and on Intel/NVIDIA Linux is not what happened: we did not
            look, because there is no unprivileged way to. The issue
            asks for exactly this ("must not render the panel at all if
            there is nothing truthful to put in it").

            On the phone this panel describes the DESKTOP's GPU like
            every other panel here, and needs nothing extra to do so --
            the sample it renders was taken on that machine. */}
        {s.gpus.length > 0 ? (
          <Panel
            title={s.gpus.length === 1 ? "GPU" : "GPUs"}
            subtitle="Utilization and the memory the graphics device is holding"
          >
            <div className="flex flex-col gap-5">
              {s.gpus.map((g, i) => (
                <GpuCard key={`${g.name}-${i}`} gpu={g} />
              ))}
            </div>
            <div className="mt-4">
              <div className="text-xs text-[#8b949e]">
                {s.gpus.length === 1
                  ? "GPU utilization over the last 24 hours"
                  : `${s.gpus[0].name} utilization over the last 24 hours`}
              </div>
              <Sparkline
                points={gpuSeries}
                max={100}
                label="GPU"
                color="#a371f7"
                now={sampledAt}
              />
              <GapNote />
            </div>
          </Panel>
        ) : null}

        <Panel title="Disk" subtitle="Every mounted volume">
          {s.disks.length === 0 ? (
            <p className="text-sm">
              <NotMeasured />
            </p>
          ) : (
            <div className="flex flex-col gap-3">
              {s.disks.map((d) => {
                const used = d.total - d.available;
                const usedPct = percentOf(used, d.total);
                return (
                  <div key={d.mount}>
                    <div className="flex items-baseline justify-between gap-2">
                      <span className="truncate text-sm text-[#e6edf3]">
                        {d.mount}
                        {/* The volume the app lives on is the one a
                            user filling their disk cares about first,
                            so it is named rather than left to be
                            guessed from the mount point. */}
                        {d.is_root ? (
                          <span className="ml-2 rounded bg-[#30363d] px-1.5 py-0.5 text-xs text-[#8b949e]">
                            system
                          </span>
                        ) : null}
                      </span>
                      <span className="shrink-0 text-xs tabular-nums text-[#8b949e]">
                        {formatSize(d.available)} free of {formatSize(d.total)}
                      </span>
                    </div>
                    <div className="mt-1">
                      <Bar percent={usedPct} label={`${d.mount} used`} />
                    </div>
                    <div className="mt-1 text-xs tabular-nums text-[#8b949e]">
                      {usedPct === null ? (
                        <NotMeasured />
                      ) : (
                        `${formatSize(used)} used (${usedPct.toFixed(0)}%)`
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </Panel>

        <Panel title="Battery and thermal">
          <div className="flex flex-wrap gap-6">
            {/* A desktop with no battery reports null. "Not measured"
                is right there too: the app did not read a battery,
                and 0% would say the machine is about to die. */}
            <Stat
              label="Battery"
              value={s.battery === null ? null : `${s.battery.percent.toFixed(0)}%`}
              hint={
                s.battery === null
                  ? IS_MOBILE_BUILD
                    ? "That desktop has no battery"
                    : "No battery on this machine"
                  : undefined
              }
            />
            <Stat
              label="Power"
              value={s.battery === null ? null : s.battery.on_ac ? "On AC" : "On battery"}
            />
            {/* The RATE, on the overview (#773).

                The issue asks for this here specifically, and the
                reason is #720: its "discharging while plugged in"
                alert fires, the user opens this page, and the number
                that answers "why did I get that alert" should be
                visible without a click. The detail page has the chart
                and the current and voltage behind it; this is the one
                figure.

                Only rendered where there IS a battery -- a desktop's
                absent rate is not a separate fact from its absent
                battery, and two "Not measured" rows for one absence is
                noise. A battery whose platform does not publish the
                flow gets the row, because that IS a separate fact. */}
            {s.battery !== null ? (
              <Stat
                label="Rate"
                value={s.battery.power ? formatWatts(s.battery.power.watts) : null}
                hint={
                  s.battery.power
                    ? powerDirection(s.battery.power.watts) === "idle"
                      ? "Not moving"
                      : powerDirection(s.battery.power.watts) === "charging"
                        ? "Into the battery"
                        : "Out of the battery"
                    : "This platform does not publish it"
                }
              />
            ) : null}
          </div>
          {s.battery !== null ? (
            <div className="mt-3">
              <Bar percent={s.battery.percent} label="Battery charge" />
            </div>
          ) : null}
          {/* The one reading here that is a FAULT rather than a state,
              said in words on the panel a reader lands on. Colour is
              never the only cue on this page, and this is the case
              #720 alerts about -- so the sentence appears wherever the
              rate does. */}
          {s.battery?.power && powerDirection(s.battery.power.watts) === "discharging" && s.battery.on_ac ? (
            <p className="mt-2 text-xs leading-relaxed text-[#f85149]">
              Losing charge while plugged in: the machine is drawing more than
              the adapter supplies, or the adapter is not really charging.
            </p>
          ) : null}

          <div className="mt-4">
            <div className="text-xs text-[#8b949e]">Thermal pressure</div>
            <div
              className="text-sm font-medium capitalize"
              style={{ color: s.thermal ? thermalColor(s.thermal) : undefined }}
            >
              {s.thermal ?? <NotMeasured />}
            </div>
            {/* The sentence the issue asks for, and the reason this
                panel is not called "Temperature".

                "Thermal pressure: nominal" invites exactly one wrong
                reading -- that it is a temperature in some unit the
                reader has not spotted. It is not: it is the platform's
                own coarse verdict on how hard it is having to work to
                stay cool. Degrees would need the SMC, which on macOS
                is readable only with elevated privileges
                (`powermetrics --samplers smc` wants sudo), and a
                reading that works only for someone running the app as
                root is worse than an honest qualitative one. Saying so
                costs a line and prevents the misreading. */}
            <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
              {s.thermal && THERMAL_MEANING[s.thermal]
                ? `${THERMAL_MEANING[s.thermal]} `
                : ""}
              This is the system's own thermal <em>pressure</em> rating, not a
              temperature. Headstate cannot read degrees: that needs elevated
              privileges, so the app reports the label the operating system
              already publishes instead.
            </p>
          </div>
        </Panel>

        <Panel
          title="Network and uptime"
          subtitle="Totals since boot, not rates"
        >
          <div className="flex flex-wrap gap-6">
            <Stat label="Uptime" value={formatUptime(s.uptime_secs)} />
            {/* Derived from uptime rather than read separately, so the
                two can never disagree by a second and look wrong.

                Subtracted from the SAMPLE's timestamp, not from the
                clock at render time: uptime was measured at that
                instant, so pairing it with a later "now" drifts the
                boot time forward on every repaint -- and a boot time
                that moves is obviously wrong to anyone watching it. */}
            <Stat
              label="Booted"
              value={new Date(sampledAt - s.uptime_secs * 1000).toLocaleString()}
            />
          </div>

          <div className="mt-4">
            <div className="text-xs text-[#8b949e]">Interfaces</div>
            {s.networks.length === 0 ? (
              <p className="mt-1 text-sm">
                <NotMeasured />
              </p>
            ) : (
              <table className="mt-2 w-full text-sm">
                <thead>
                  <tr className="text-left text-xs text-[#8b949e]">
                    <th className="font-normal">Interface</th>
                    <th className="font-normal text-right">In</th>
                    <th className="font-normal text-right">Out</th>
                  </tr>
                </thead>
                <tbody>
                  {s.networks.map((n) => (
                    <tr key={n.name} className="border-t border-[#30363d]">
                      <td className="py-1 pr-2 truncate">{n.name}</td>
                      <td className="py-1 text-right tabular-nums text-[#8b949e]">
                        {formatSize(n.rx_bytes)}
                      </td>
                      <td className="py-1 text-right tabular-nums text-[#8b949e]">
                        {formatSize(n.tx_bytes)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {/* Cumulative since boot is a surprising choice to a reader
                expecting a speed, so it is stated rather than left to
                be inferred from a number that only ever goes up. */}
            <p className="mt-2 text-xs text-[#8b949e]">
              Totals since the machine booted, not current speeds.
            </p>
          </div>
        </Panel>
      </div>

      {history.isError ? (
        // The live panels above are fine; only the charts are missing.
        // Said in place rather than replacing the page, because the
        // current numbers are the half of this view people open it for.
        <div className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-4 py-2 text-xs text-[#d29922]">
          {/* Unchanged wording (#946): this sentence's job is to keep an
              empty chart from reading as a flat one, and a retry beside
              it does not change what it has to say. */}
          The 24-hour history could not be loaded, so the charts above are
          empty. The current readings are unaffected.
          <HistoryRetry onRetry={() => void history.refetch()} />
        </div>
      ) : null}
    </div>
  );
}

/// The one-line explanation of why a chart has holes in it.
///
/// Beside each chart rather than once at the top of the page: a break
/// in a line is noticed while looking at that line, and an explanation
/// two panels away is one nobody reads at the moment they need it.
function GapNote() {
  return (
    <p className="mt-1 text-xs text-[#8b949e]">
      {IS_MOBILE_BUILD
        ? // The phone has a second way to produce a hole that the
          // desktop does not: the desktop can be sampling perfectly
          // while this phone is unable to reach it. Both are honest
          // gaps and neither is interpolated, but they are different
          // facts, and saying only the first would assert the desktop
          // was off during a period it may have been running fine.
          "Breaks in the line are periods with no reading — either Headstate was not running on that desktop, or this phone could not reach it. Nothing is filled in across them."
        : "Breaks in the line are periods when Headstate was not running. Nothing is filled in across them."}
    </p>
  );
}

/// The class list, on the phone, as cards on the overview (#687).
///
/// # Why the phone gets this and the desktop does not
///
/// The desktop keeps the class list in the sidebar, which is where
/// navigation lives in every other view. The phone has no persistent
/// sidebar -- `App` puts it in a sheet behind a hamburger -- and that
/// sheet holds the VIEW switcher. Burying a second level of navigation
/// one gesture deeper inside it would make the drill-downs something
/// you have to already know are there.
///
/// So on the phone the classes are on the page itself, at the top of
/// the overview, as tappable cards. The overview is the landing page
/// either way; this just makes the doors visible on the device that
/// cannot show a column of them.
///
/// # Viewport, not build
///
/// `useIsMobile()` rather than `IS_MOBILE_BUILD`, because this is a
/// LAYOUT question: a desktop window dragged under 768px has no sidebar
/// either -- `App` puts it in the same sheet -- so it needs the same
/// door. Which machine is being DESCRIBED is the build question, and
/// that is decided elsewhere on this page. Getting these two the wrong
/// way round is the category error #598 fixed.
function HealthPageNav({ gpuCount }: { gpuCount: number }) {
  const isMobile = useIsMobile();
  const setHealthPage = useFilters((f) => f.setHealthPage);
  if (!isMobile) return null;

  // "overview" is skipped: this row IS the overview, so a card leading
  // to where you already are would be a control that does nothing.
  //
  // `healthPagesFor` is the same filter the sidebar applies, called
  // rather than reimplemented so the phone and the desktop cannot drift
  // into offering different pages -- which is what `HEALTH_PAGES` being
  // one array exists to prevent, and would be undone by a second copy
  // of the GPU rule here.
  const pages = healthPagesFor(gpuCount).filter((p) => p.id !== "overview");

  return (
    <nav aria-label="System health sections">
      <ul className="grid grid-cols-2 gap-2">
        {pages.map(({ id, label, blurb, Icon }) => (
          <li key={id}>
            <button
              type="button"
              onClick={() => setHealthPage(id)}
              // `tap-target` is the app's own minimum-size utility, the
              // same one the header's nav button uses. A card this
              // small is otherwise easy to miss with a thumb.
              className="tap-target flex w-full flex-col items-start gap-0.5 rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2.5 text-left hover:border-[#8b949e]"
            >
              <span className="flex items-center gap-1.5 text-sm font-medium text-[#e6edf3]">
                <Icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                {label}
              </span>
              {/* The blurb, only here. A sidebar row is read beside the
                  page it opens; a card is read before anything is open
                  and has to say what is behind it. */}
              <span className="text-xs leading-snug text-[#8b949e]">{blurb}</span>
            </button>
          </li>
        ))}
      </ul>
    </nav>
  );
}

/// One class's detail page: the header, and the right body under it.
///
/// A shell around a switch rather than five independent pages, because
/// every one of them needs the same three things -- the title, the way
/// back on a phone, and the same live sample -- and five copies of that
/// header is five chances for one of them to lose the back link. Which
/// is the failure that matters: on a phone the back link is the ONLY
/// way off a detail page.
function DetailPage({
  page,
  sample,
  samples,
  sampledAt,
  historyFailed,
  onRetryHistory,
  alerts,
  alertsFailed,
  alertsError,
  onRetryAlerts,
  renderedAt,
}: {
  page: Exclude<HealthPage, "overview">;
  /// The current conditions, for the CPU page (#864). Passed down from
  /// the parent's single observer rather than re-queried: someone who
  /// drilled into CPU because the machine is slow should see the runaway
  /// alert on the page that explains it, not only on the overview they
  /// navigated away from.
  alerts: AlertReport[] | undefined;
  alertsFailed: boolean;
  /// The alerts rejection and its retry (#946). Forwarded from the
  /// parent's single observer rather than re-queried here, exactly as
  /// `alerts` itself is: a second observer on the same key would poll
  /// twice and could show a different answer from the overview.
  alertsError: unknown;
  onRetryAlerts: () => void;
  /// "Now" for the staleness check, resolved once by the parent rather
  /// than read from the clock here -- same rule as `Sparkline`'s `now`.
  renderedAt: number;
  sample: HealthSample;
  /// The 24-hour series, already fetched by the parent. Passed down
  /// rather than re-queried so a drill-down does not open a second
  /// observer on the same key.
  samples: HealthSample[];
  /// The live sample's own timestamp, which is "now" for everything
  /// time-relative on this page -- see the overview's note on why the
  /// wall clock is not read during render.
  sampledAt: number;
  historyFailed: boolean;
  onRetryHistory: () => void;
}) {
  const isMobile = useIsMobile();
  const setHealthPage = useFilters((f) => f.setHealthPage);
  const meta = healthPagesFor(sample.gpus.length).find((p) => p.id === page);

  return (
    <div className="flex flex-col gap-4">
      {/* The way back, on the phone only. The desktop has the sidebar,
          where the current page is highlighted and Overview is one
          click away -- a back link there would be a second control for
          something already on screen. On the phone it is the only one,
          which is why it is a real control and not a gesture. */}
      {isMobile ? (
        <button
          type="button"
          onClick={() => setHealthPage("overview")}
          className="tap-target -ml-1 flex items-center gap-1 self-start rounded px-1 text-sm text-[#58a6ff] hover:underline"
        >
          <ChevronLeft className="h-4 w-4" aria-hidden="true" />
          System health overview
        </button>
      ) : null}

      {/* An h1 rather than an h2: on a detail page this class IS the
          subject, and the panels below it are its sections. */}
      <h1 className="text-base font-semibold text-[#e6edf3]">{meta?.label ?? page}</h1>

      {/* The CPU page only (#864). Someone arrives here because the
          machine is slow, and the runaway rules are the one thing on
          this page that can NAME the cause rather than plot it. The
          other drill-downs have no rule that fires on them, so an
          empty panel there would be furniture. */}
      {page === "cpu" ? (
        <HealthConditions
          alerts={alerts}
          failed={alertsFailed}
          error={alertsError}
          onRetry={onRetryAlerts}
          sampledAt={sampledAt}
          now={renderedAt}
        />
      ) : null}

      {page === "cpu" ? (
        <CpuDetail sample={sample} samples={samples} sampledAt={sampledAt} />
      ) : page === "memory" ? (
        <MemoryDetail sample={sample} samples={samples} sampledAt={sampledAt} />
      ) : page === "disk" ? (
        <DiskDetail sample={sample} />
      ) : page === "gpu" ? (
        <GpuDetail sample={sample} samples={samples} sampledAt={sampledAt} />
      ) : page === "network" ? (
        <NetworkDetail sample={sample} samples={samples} sampledAt={sampledAt} />
      ) : (
        <PowerDetail sample={sample} samples={samples} sampledAt={sampledAt} />
      )}

      {historyFailed ? (
        // Said here as well as on the overview, for the same reason the
        // gap note is repeated beside each chart: an explanation on
        // another page is one nobody reads at the moment they need it.
        // And the RETRY is repeated for the same reason again (#946): a
        // control on the page you navigated away from is one you do not
        // have.
        <div className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-4 py-2 text-xs text-[#d29922]">
          The 24-hour history could not be loaded, so any chart here is empty.
          The current readings are unaffected.
          <HistoryRetry onRetry={onRetryHistory} />
        </div>
      ) : null}
    </div>
  );
}

/// The machine's top processes by one measure — the answer to "why".
///
/// # This is the point of #687
///
/// "CPU is at 80%" is a symptom. This is the answer, and it is the one
/// thing a panel of aggregates structurally cannot say.
///
/// # The top eight, and saying so
///
/// The Rust side returns at most `TOP_N` (eight) rows and the count of
/// everything running. Both halves matter. Eight rows is an answer;
/// 1400 rows is the filtering problem handed back to the reader. But
/// eight rows presented as if they were the whole machine is a
/// different lie, so the count of what is NOT shown is printed under
/// the table. A reader who needs the rest then knows to reach for
/// Activity Monitor rather than concluding this is everything.
///
/// # Not measured, not empty
///
/// An older desktop that has not been updated returns a `Footprint`
/// with no `top_cpu` field at all, which arrives as `undefined` -- and
/// that is not the same fact as "nothing is running", which cannot
/// happen on a booted machine. The two are rendered differently for the
/// same reason every other absence on this page is.
function TopProcesses({
  processes,
  groups,
  processCount,
  /// Which column is the reason these rows were chosen. The other is
  /// still shown -- a process is interesting for both -- but only one
  /// of them explains the ordering, and a table sorted by a column the
  /// reader is not looking at reads as a table sorted wrongly.
  by,
}: {
  processes: FootprintProcess[] | undefined;
  /// The same rows summed by name, from the same reading (#721).
  ///
  /// `undefined` when the desktop is too old to send them, which is a
  /// different fact from an empty list and is why the toggle is hidden
  /// rather than shown offering a mode that cannot be entered.
  groups: FootprintProcessGroup[] | undefined;
  processCount: number | undefined;
  by: "cpu" | "memory";
}) {
  // Individual is the DEFAULT, and deliberately.
  //
  // It is the view that makes no inference at all: each row is one
  // process the kernel reported, with its own PID. Grouping by name is
  // a heuristic -- it merges genuinely unrelated programs that happen
  // to share a name -- and a heuristic that is on by default is one
  // nobody chose. So the honest view is the one you land on, and the
  // useful-but-inferred one is one click away.
  //
  // # Not persisted, on purpose
  //
  // `useUiPrefs` would have made this sync desktop-to-phone for almost
  // nothing, and it was considered. The argument against it is what a
  // grouped row LOOKS like: `acme-agent (26)` at 13% is a claim about
  // twenty-six processes, and someone returning next week to a view
  // they set once and forgot will read it as one process at 13% -- a
  // wrong number with no visible cause. A view mode that changes what
  // a row MEANS is different from one that changes how rows are packed
  // (`density`, which does persist, and where being wrong costs
  // nothing).
  //
  // It is also the same call `healthPage` itself makes for the same
  // reason: this state answers one question in one sitting. Local
  // `useState` rather than the store, so it also resets when you leave
  // the page -- there is no cross-component reader for it.
  const [grouped, setGrouped] = useState(false);

  if (processes === undefined) {
    return (
      <p className="text-sm">
        <NotMeasured>
          {IS_MOBILE_BUILD
            ? "— that desktop is running a version of Headstate that does not report this."
            : "— no process list came back."}
        </NotMeasured>
      </p>
    );
  }
  if (processes.length === 0) {
    // Not reachable on a booted machine -- something is always running,
    // including us -- but rendered as a sentence rather than an empty
    // table so that if it ever does happen it says so instead of
    // looking like a table that failed to paint.
    return <p className="text-sm text-[#8b949e]">No processes were reported.</p>;
  }

  // The toggle is only offered when the grouped rows actually arrived.
  // An older desktop sends none, and a control that switches to an
  // empty view is worse than no control -- the user would read it as a
  // machine on which nothing groups.
  const canGroup = groups !== undefined;
  // Guarded on `canGroup` rather than on `grouped` alone: nothing can
  // set `grouped` while the toggle is hidden, but a later edit could,
  // and the failure would be an empty table under a heading.
  const showGrouped = grouped && canGroup;

  // How many processes the visible rows actually account for. Under
  // grouping a row is many processes, so the count of what is NOT shown
  // is the sum of the counts rather than the number of rows -- which is
  // what keeps the sentence below true in both modes. Getting this
  // wrong is subtle and plausible: "1436 total minus 8 rows" reads
  // perfectly and is off by however many siblings each group holds.
  const shown = showGrouped
    ? (groups ?? []).reduce((n, g) => n + g.count, 0)
    : processes.length;
  const rest = processCount === undefined ? null : Math.max(0, processCount - shown);
  // A group whose CPU sum is over fewer members than the group claims.
  // Rare -- it needs the platform to have failed on one process's
  // accounting -- but a partial total presented as a complete one is
  // exactly the lie this view exists to refuse.
  const partial = showGrouped
    ? (groups ?? []).filter((g) => g.cpu_unmeasured > 0)
    : [];

  return (
    <div>
      {canGroup ? (
        <div
          className="mb-3 flex items-center gap-1"
          role="group"
          aria-label={`How ${by === "cpu" ? "CPU" : "memory"} rows are counted`}
        >
          {/* Two buttons rather than a checkbox labelled "Grouped".
              These are two ways of counting the same machine, and a
              checkbox makes one of them the absence of the other --
              which reads as "Grouped: off" rather than as "Individual",
              and hides that the default is itself a choice. */}
          {(
            [
              ["Individual", false],
              ["Grouped", true],
            ] as const
          ).map(([label, value]) => (
            <button
              key={label}
              type="button"
              onClick={() => setGrouped(value)}
              // `aria-pressed`, not `aria-current`: these ARE toggles.
              // The sidebar's pages are navigation and use
              // `aria-current="page"`; this changes what the table in
              // front of you means without moving you anywhere.
              aria-pressed={showGrouped === value}
              className={`tap-target rounded-md border px-2.5 py-1 text-xs ${
                showGrouped === value
                  ? "border-[#1f6feb] bg-[#1f6feb] text-white"
                  : "border-[#30363d] bg-[#21262d] text-[#e6edf3] hover:border-[#8b949e]"
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      ) : null}

      {/* The table gets its OWN horizontal scroll rather than letting
          the excess reach the document (#973). Before this the table
          measured 533.6px inside a 326px panel at 390px, and with no
          container the 207.6px of overflow scrolled the WHOLE PAGE
          sideways -- the pressure row, the sparklines and the headings
          all slid when a reader tried to reach the process names.

          A scrolling table is acceptable where a scrolling page is not,
          and this table specifically needs the affordance: the headings
          ALONE are 231.0px of the 326px budget (`Process` 45.0 +
          `PID` 19.4 + `CPU (of one core)` 97.0 + `Memory` 45.6 + 24px
          of padding), and none of those strings may be shortened --
          `CPU (of one core)` carries the denominator that stops a
          three-core process reading as a bug, and the PID is the column
          a reader copies into `ps`. So the columns stay and become
          reachable, instead of being dropped or clipped.

          `min-w-full` on the table rather than `w-full`: inside a
          scroll container `w-full` would resolve against the scroll
          width and let the numeric columns collapse, while `min-w-full`
          keeps the table filling the panel when it fits and lets it
          exceed it when it does not. */}
      <div className="overflow-x-auto">
        <table className="min-w-full text-sm">
          <thead>
            <tr className="text-left text-xs text-[#8b949e]">
              <th className="font-normal">Process</th>
              {/* No PID column under grouping, rather than an empty or
                  a "first PID" one. A group has no PID: printing one of
                  the twenty-six would name a process the row is not
                  about, and it is the column a reader copies into `ps`. */}
              {showGrouped ? null : <th className="font-normal text-right">PID</th>}
              {/* Same heading as the footprint panel's, and for the same
                  reason: `cpu_percent` is a share of ONE core, so a
                  process using three legitimately reads 280%. Under a
                  bare "CPU" that looks like a bug. */}
              <th
                className={`font-normal text-right ${by === "cpu" ? "text-[#e6edf3]" : ""}`}
              >
                CPU (of one core)
              </th>
              <th
                className={`font-normal text-right ${by === "memory" ? "text-[#e6edf3]" : ""}`}
              >
                Memory
              </th>
            </tr>
          </thead>
          <tbody>
            {showGrouped
              ? // Keyed on the NAME, which is what identifies a group --
                // and unlike the individual rows, it is unique here by
                // construction: one row per distinct name.
                (groups ?? []).map((g) => <ProcessGroupRow key={g.name} group={g} />)
              : /* Keyed on the PID, not the name: several processes of one
                   app share a name, and keying on it would make React reuse
                   one row's DOM for another process. */
                processes.map((p) => <ProcessRow key={p.pid} p={p} />)}
          </tbody>
        </table>
      </div>
      {rest !== null && rest > 0 ? (
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          {showGrouped ? (
            <>
              The {(groups ?? []).length} biggest by summed{" "}
              {by === "cpu" ? "CPU" : "memory"}, covering {shown} of{" "}
              {processCount} processes running. The other {rest} are not listed
              — on an ordinary machine almost all of them are idle, and a list
              of every process is not an answer to what is using this one.
            </>
          ) : (
            <>
              The {processes.length} biggest by {by === "cpu" ? "CPU" : "memory"},
              of {processCount} processes running. The other {rest} are not
              listed — on an ordinary machine almost all of them are idle, and a
              list of every process is not an answer to what is using this one.
            </>
          )}
        </p>
      ) : null}
      {showGrouped ? (
        // What grouping IS, said where it is in effect. Rows named
        // `acme-agent (26)` are a claim about twenty-six processes that
        // share a name -- not about one program, which the app cannot
        // actually establish without walking ancestry it deliberately
        // does not walk. See `ProcessGroup` on the Rust side.
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          Rows are every process of that name added together, with the count in
          brackets. It groups by <em>name</em>, not by which process started
          which — so two unrelated programs that happen to share a name are one
          row here. Switch to Individual to see them as the system reports them.
        </p>
      ) : null}
      {partial.length > 0 ? (
        // Absent is not zero, inside a sum. A process whose CPU the
        // platform would not report is not added in as 0 -- it is left
        // out and counted here, so a total over 25 of 26 is never
        // presented as a total over 26.
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          {partial.map((g) => g.name).join(", ")}{" "}
          {partial.length === 1 ? "has" : "have"} processes whose CPU this
          machine did not report. Those are left out of the total rather than
          counted as zero, so the summed figure covers fewer processes than the
          count in brackets.
        </p>
      ) : null}
    </div>
  );
}

/// One grouped row: every process of a name, summed (#721).
///
/// Separate from `ProcessRow` rather than a mode of it. The two render
/// different numbers of columns (a group has no PID) and different
/// first cells, and switching one component on a flag would make the
/// individual table's markup depend on a feature it does not have.
///
/// CPU is NOT clamped and NOT drawn as a bar, for the same reason as
/// `ProcessRow` and more so: this is a sum of per-core shares, so
/// twenty-six processes at 0.5% legitimately reads 13%, and a group
/// genuinely using four cores reads 400%.
function ProcessGroupRow({ group: g }: { group: FootprintProcessGroup }) {
  return (
    <tr className="border-t border-[#30363d]">
      {/* `max-w-0` + `md:truncate`, the same containment `ProcessRow`
          takes and for the same #973 measurement -- see the comment
          there. `flex` on the inner row rather than truncating the
          `<td>` text directly, because this cell has TWO children and
          the count must not be the half that gets ellipsised: a summed
          row read as a single process is a wrong number with no visible
          cause, which is exactly what the count exists to prevent. So
          the NAME truncates and the count is `shrink-0` beside it --
          `Outliers`' rule, applied to a table cell. */}
      <td className="max-w-0 py-1 pr-2" title={g.name}>
        <div className="flex items-baseline gap-1.5">
          <span className="min-w-0 break-words text-[#e6edf3] md:truncate">{g.name}</span>
          {/* The count is not decoration: it is what stops a summed row
              being read as a single process. `(1)` is printed too --
              omitting it for groups of one would make the presence of a
              number mean "several", which is a second thing to learn. */}
          <span className="shrink-0 text-xs tabular-nums text-[#8b949e]">({g.count})</span>
        </div>
      </td>
      <td className="py-1 pr-2 text-right tabular-nums text-[#e6edf3]">
        {g.cpu_percent.toFixed(0)}%
        {g.cpu_unmeasured > 0 ? (
          // Marked on the row as well as explained under the table: a
          // reader comparing two rows needs to know THIS one is a
          // partial sum at the moment they read it.
          <span
            className="ml-1 text-xs text-[#8b949e]"
            title={`${g.cpu_unmeasured} of ${g.count} processes reported no CPU figure and are not in this total`}
          >
            *
          </span>
        ) : null}
      </td>
      <td className="py-1 text-right tabular-nums text-[#e6edf3]">
        {formatSize(g.memory)}
      </td>
    </tr>
  );
}

/// The CPU page: load, every core, the 24-hour line, and who is why.
function CpuDetail({
  sample: s,
  samples,
  sampledAt,
}: {
  sample: HealthSample;
  samples: HealthSample[];
  sampledAt: number;
}) {
  // The footprint carries the machine-wide lists since #687. Enabled
  // here for the same reason the overview enables it: this component
  // only mounts while the page is open, so the poll stops when it
  // unmounts. Cheap on the five-second cadence -- 19-24ms for 1436
  // processes, measured; see the Rust module docs.
  const fp = useSystemFootprint(true);
  // `null` when the platform reported no per-core figures, so every
  // reader below asks "did we measure this" rather than reading a length
  // of 0 as a core count. Absent is not zero (`caches/mod.rs:550`), and a
  // machine with zero cores does not exist (#958).
  const cores = s.cpu_per_core.length === 0 ? null : s.cpu_per_core.length;
  const series = useMemo(
    () => samples.map((x) => ({ t: Date.parse(x.sampled_at), v: x.cpu_percent })),
    [samples],
  );

  return (
    <div className="flex flex-col gap-4">
      {/* The processes FIRST, above the aggregates. The overview
          already answered "how busy"; someone who clicked through to
          this page did so because that number was interesting, and the
          next thing they want is the name of what is doing it. Putting
          per-core bars above it would make them scroll past the
          symptom to reach the cause. */}
      <Panel
        title="What is using the CPU"
        subtitle="The biggest consumers on this machine, right now"
      >
        {fp.isError && fp.data === undefined ? (
          <p className="text-sm text-[#8b949e]">
            {/* Wording unchanged, retry added (#946). `fp.refetch` was in
                scope here the whole time. */}
            Could not read the process list: {errorMessage(fp.error)}
            <InlineRetry onRetry={() => void fp.refetch()} />
          </p>
        ) : fp.data === undefined ? (
          <p className="text-sm text-[#8b949e]">Reading…</p>
        ) : (
          <TopProcesses
            processes={fp.data.top_cpu}
            groups={fp.data.top_cpu_grouped}
            processCount={fp.data.process_count}
            by="cpu"
          />
        )}
      </Panel>

      <Panel title="Load and use" subtitle="Now, and averaged over three windows">
        <div className="flex flex-wrap gap-6">
          <Stat
            label="CPU"
            value={s.cpu_percent === null ? null : `${s.cpu_percent.toFixed(0)}%`}
          />
          <Stat label="Load (1m)" value={s.load ? s.load[0].toFixed(2) : null} />
          <Stat label="Load (5m)" value={s.load ? s.load[1].toFixed(2) : null} />
          <Stat label="Load (15m)" value={s.load ? s.load[2].toFixed(2) : null} />
          {/* `null`, not `0`, when the platform reported no cores.
              `Stat`'s own doc says the value "is never coerced to 0" --
              and this call site passed a template string, which made
              `Stat` render the literal "0" and skip the `NotMeasured`
              branch entirely. No machine has zero cores; an empty
              `cpu_per_core` means nothing was discoverable (#958). */}
          <Stat label="Cores" value={cores === null ? null : `${cores}`} />
        </div>
        {s.load === null ? (
          <p className="mt-2 text-xs text-[#8b949e]">
            This platform does not report load averages.
          </p>
        ) : cores === null ? (
          // Load averages WITHOUT a core count, which is a real
          // platform state and the one this paragraph used to get
          // wrong. The guard was `s.load === null`, so on a machine
          // that reports load but not per-core usage the prose rendered
          // "On this machine's 0 cores, a load near 0 means it is busy
          // but keeping up" -- telling a reader that a machine at load
          // 2.41 with work queuing was coping.
          //
          // The paragraph exists to stop the single most misread number
          // on this page being misread, so in that state it caused
          // precisely the error it is for. The general explanation is
          // still worth giving; the arithmetic is not, because the
          // number it needs was never measured.
          <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
            A load average is the number of processes wanting to run, not a
            percentage, so whether a given figure is high depends on how many
            cores the machine has. This platform did not report its core count,
            so there is nothing to compare these against.
          </p>
        ) : (
          // What a load average MEANS, which the overview has no room
          // for and which is the single most misread number on this
          // page: it is a count of runnable work, not a percentage, so
          // whether 4.0 is bad depends entirely on the core count.
          <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
            A load average is the number of processes wanting to run, not a
            percentage. On this machine&apos;s {cores} cores, a load near{" "}
            {cores} means it is busy but keeping up; well above that means work
            is queuing.
          </p>
        )}
      </Panel>

      <Panel title="Per core" subtitle="In the platform's own core order">
        {s.cpu_per_core.length === 0 ? (
          <p className="text-sm">
            <NotMeasured />
          </p>
        ) : (
          <div className="flex flex-col gap-1">
            {s.cpu_per_core.map((v, i) => (
              <div key={i} className="flex items-center gap-2">
                <span className="w-10 shrink-0 text-xs tabular-nums text-[#8b949e]">
                  #{i}
                </span>
                <Bar percent={v} label={`Core ${i}`} />
                <span className="w-10 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                  {v.toFixed(0)}%
                </span>
              </div>
            ))}
          </div>
        )}
        {/* Why one core at 100% beside seven idle ones is normal, and
            why the average underneath it is not a reassurance. A
            single-threaded build pins exactly one core, and the mean
            of that on a 10-core machine is 10% -- which the overview
            reports, correctly, and which reads as an idle machine to
            someone whose editor is unresponsive. */}
        {s.cpu_per_core.length > 1 ? (
          <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
            One core at 100% while the rest are idle is normal: most work is
            single-threaded, and it pins one core. The whole-machine figure
            averages across all {s.cpu_per_core.length}, so it can look calm
            while something is entirely stuck.
          </p>
        ) : null}
      </Panel>

      <Panel title="The last 24 hours" subtitle="Whole-machine CPU use">
        <Sparkline points={series} max={100} label="CPU" now={sampledAt} />
        <GapNote />
      </Panel>
    </div>
  );
}

/// The Memory page: pressure, swap, the 24-hour line, and who holds it.
function MemoryDetail({
  sample: s,
  samples,
  sampledAt,
}: {
  sample: HealthSample;
  samples: HealthSample[];
  sampledAt: number;
}) {
  const fp = useSystemFootprint(true);
  const usedPct = percentOf(s.memory.used, s.memory.total);
  const swapPct = percentOf(s.memory.swap_used, s.memory.swap_total);
  const series = useMemo(
    () =>
      samples.map((x) => ({
        t: Date.parse(x.sampled_at),
        v: percentOf(x.memory.used, x.memory.total),
      })),
    [samples],
  );

  return (
    <div className="flex flex-col gap-4">
      {/* Processes first, for the same reason as the CPU page. */}
      <Panel
        title="What is holding the memory"
        subtitle="The biggest resident sets on this machine, right now"
      >
        {fp.isError && fp.data === undefined ? (
          <p className="text-sm text-[#8b949e]">
            {/* Same read, same remedy, same control as the CPU page's
                (#946). One query serves both pages, so a retry on either
                fixes both. */}
            Could not read the process list: {errorMessage(fp.error)}
            <InlineRetry onRetry={() => void fp.refetch()} />
          </p>
        ) : fp.data === undefined ? (
          <p className="text-sm text-[#8b949e]">Reading…</p>
        ) : (
          <>
            <TopProcesses
              processes={fp.data.top_memory}
              groups={fp.data.top_memory_grouped}
              processCount={fp.data.process_count}
              by="memory"
            />
            {/* Why these figures do not add up to "used", which is the
                first thing anyone tries with a list like this. Shared
                libraries are counted in every process holding them, so
                summing resident sets over-counts -- and someone who
                sums eight rows and gets more than the machine has will
                assume the numbers are wrong rather than the method. */}
            <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
              These are resident sets — physical memory each process holds
              now. They do not add up to the total in use: memory shared
              between processes, which is most of a system&apos;s libraries,
              is counted once in every process holding it.
            </p>
          </>
        )}
      </Panel>

      <Panel title="Pressure" subtitle="What is in use, and what can still be reclaimed">
        <div className="flex flex-wrap gap-6">
          <Stat label="Used" value={formatSize(s.memory.used)} />
          <Stat
            label="Available"
            value={formatSize(s.memory.available)}
            hint="Reclaimable, incl. cache"
          />
          <Stat label="Total" value={formatSize(s.memory.total)} />
          <Stat
            label="Pressure"
            value={usedPct === null ? null : `${usedPct.toFixed(0)}%`}
          />
        </div>
        <div className="mt-3">
          <Bar percent={usedPct} label="Memory used" />
        </div>
        {/* The same warning the overview's hint gives in four words,
            with room here to say why it matters: a reader who subtracts
            "used" from "total" and does not get "available" concludes
            one of the three is wrong. */}
        <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
          Used and available do not add up to the total, and that is correct:
          cached file data counts as used and is also reclaimable the moment
          something needs it. A machine at 90% used is not a machine that is
          nearly out.
        </p>
      </Panel>

      <Panel title="Swap" subtitle="Memory the system has moved to disk">
        <div className="flex flex-wrap gap-6">
          <Stat
            label="Swap used"
            value={
              s.memory.swap_total === 0
                ? null
                : `${formatSize(s.memory.swap_used)} of ${formatSize(s.memory.swap_total)}`
            }
            hint={s.memory.swap_total === 0 ? "No swap configured" : undefined}
          />
          <Stat label="Swap" value={swapPct === null ? null : `${swapPct.toFixed(0)}%`} />
        </div>
        {s.memory.swap_total === 0 ? (
          // A machine with swap disabled reports a zero total, which is
          // "there is no swap" and not "0% of it is used".
          <p className="mt-2 text-xs text-[#8b949e]">
            This machine has no swap configured, so nothing can be paged out.
          </p>
        ) : (
          <div className="mt-3">
            <Bar percent={swapPct} label="Swap used" />
          </div>
        )}
      </Panel>

      <Panel title="The last 24 hours" subtitle="Memory used, as a share of total">
        <Sparkline
          points={series}
          max={100}
          label="Memory"
          color="#3fb950"
          now={sampledAt}
        />
        <GapNote />
      </Panel>
    </div>
  );
}

/// The Disk page: every volume on this machine, and nothing else.
///
/// # Why there is no "which of this is ours" panel (#796)
///
/// There was one, behind a Measure button, summing worktrees, build
/// artifacts, virtualenvs and Docker images. It was removed, and the
/// absence is the design rather than an omission.
///
/// This page answers a MACHINE question -- "is this disk filling up" --
/// and every volume above is part of that answer. "Which of that is
/// Headstate's" is a different question, and it was already answered in
/// three places that can act on the answer: the Worktrees page removes
/// a worktree, the Artifacts page prunes a build directory, the Docker
/// page reclaims image and cache space. This panel could only report,
/// and it reported worse -- figures stale the moment they were taken,
/// behind a click, restating three pages that were already correct.
///
/// The rejected alternative was keeping it and making it actionable
/// (remove buttons, per-row drill-ins). That is the three pages, built
/// a second time on a summary, and the second implementation is the one
/// that disagrees with the first.
///
/// Removing it also dropped a caller of `size_worktrees`, which is the
/// ~13s call on a real 147-worktree machine (#661). Nothing on this
/// page is slow now, which is why there is no Measure affordance left
/// anywhere in this view.
///
/// # Why there IS a signpost, which is not the same thing (#955)
///
/// #796's argument above is sound and this is not a counter-argument to
/// it. It rejects three specific things, and a LINK is none of the three:
///
/// 1. **Stale figures.** A link computes nothing, so there is nothing to go
///    stale. This is the load-bearing distinction: the panel was worse than
///    the three pages because it reported, and it reported numbers that
///    aged the moment they were taken.
/// 2. **A second implementation of removal that can disagree with the
///    first.** A link is the opposite of that. It sends the reader TO the
///    one implementation instead of restating it, which is why no figure,
///    no size and no remove button appears below.
/// 3. **A slow Measure affordance.** `size_worktrees` is the ~13s call and
///    nothing here invokes it. "No Measure affordance left anywhere in this
///    view" is preserved literally.
///
/// What #796 establishes is that this page must not REPORT Headstate's
/// share of the disk. It does not establish that the page must not POINT AT
/// the pages that can act -- and the comment's own closing paragraph is a
/// list of exactly those three pointers, written where only a maintainer
/// will ever read it. Measured on the development machine: the system
/// volume at 73% (652 GB of 926 GB) and `docker system df` reporting
/// 25.2 GB reclaimable by commands this app already ships. A first-time
/// user reads the percentage and is on their own, because knowing that
/// Worktrees, Artifacts and Docker are where space comes from is precisely
/// the knowledge they do not have.
///
/// So: a signpost, with no numbers in it. It says where, never how much.
///
/// **Surface class: none.** `setView` against the Zustand store, no Tauri
/// command and no IPC. It works on the phone, where all three target views
/// are reachable (`MOBILE_HIDDEN_VIEWS` is empty). The removal commands
/// behind those views stay exactly as classed -- `Destructive`, with their
/// step-up -- and none of them is put on this page.
function DiskDetail({ sample: s }: { sample: HealthSample }) {
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Volumes" subtitle="Every mounted volume on this machine">
        {s.disks.length === 0 ? (
          <p className="text-sm">
            <NotMeasured />
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {s.disks.map((d) => {
              const used = d.total - d.available;
              const usedPct = percentOf(used, d.total);
              return (
                <div key={d.mount}>
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="truncate text-sm text-[#e6edf3]">
                      {d.mount}
                      {d.is_root ? (
                        <span className="ml-2 rounded bg-[#30363d] px-1.5 py-0.5 text-xs text-[#8b949e]">
                          system
                        </span>
                      ) : null}
                    </span>
                    <span className="shrink-0 text-xs tabular-nums text-[#8b949e]">
                      {formatSize(d.available)} free of {formatSize(d.total)}
                    </span>
                  </div>
                  <div className="mt-1">
                    <Bar percent={usedPct} label={`${d.mount} used`} />
                  </div>
                  <div className="mt-1 text-xs tabular-nums text-[#8b949e]">
                    {usedPct === null ? (
                      <NotMeasured />
                    ) : (
                      `${formatSize(used)} used (${usedPct.toFixed(0)}%)`
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
        {/* Why a Mac lists a dozen volumes that are not disks. Without
            this, a reader counts nine read-only mounts and concludes
            the page is broken. */}
        <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
          Everything the operating system has mounted, which on macOS includes
          several read-only system volumes that share one physical disk. The
          one marked <span className="text-[#c9d1d9]">system</span> is where
          Headstate and the OS live.
        </p>
      </Panel>

      {/* No process list here, deliberately, and the reason is not the
          one the CPU and Memory pages have. The kernel does not
          attribute disk USE to a process the way it attributes CPU and
          resident memory -- a file belongs to whoever wrote it, which
          may be a process that exited months ago. Naming "the processes
          using the disk" would mean per-process I/O rates, which is a
          different measurement, needs elevated privileges on macOS, and
          answers "who is writing" rather than "what is full". So the
          volumes above are the whole page: see the doc comment for why
          "which of this is Headstate's" is not here either (#796) -- and
          for why a signpost to the pages that CAN act is not that. */}
      <ReclaimSignpost />
    </div>
  );
}

/// Where Headstate can reclaim disk space, as links and nothing else (#955).
///
/// The doc comment on `DiskDetail` above carries the whole argument,
/// including why this does not contradict #796's removal of the "which of
/// this is ours" panel. The short version: that panel reported, this points.
///
/// # No figures, by construction and not by restraint
///
/// This component reads no size, calls no command, and takes no props. It
/// therefore CANNOT show a stale number, which is the objection #796
/// actually raises -- and the absence is structural rather than a rule
/// someone has to keep remembering. Adding a size here would mean adding a
/// query, which is a visible change rather than a quiet drift.
///
/// # It respects `hidden_views`
///
/// A user who hid the Docker view does not want to be sent to it. `hidden`
/// means "I do not want to see this", and honouring it here is the same
/// courtesy `ViewSwitcher` extends -- with the difference that a jump has no
/// current-view escape hatch to fall back on, so a link to a hidden view
/// would land the reader on a page their own switcher does not list. When
/// every target is hidden the whole signpost goes, rather than leaving a
/// sentence promising links that are not there.
function ReclaimSignpost() {
  const setView = useFilters((f) => f.setView);
  const { prefs } = useUiPrefs();
  const hidden = new Set(prefs?.hidden_views ?? []);

  // Three views and not four: virtualenvs are a SECTION of the Artifacts
  // page (`ArtifactsPage` renders `VenvSection`), not a view of their own,
  // so a fourth link would be a second name for the same destination.
  // Named in the Artifacts entry's own wording instead.
  const targets: ReadonlyArray<{ view: View; label: string; what: string }> = [
    { view: "worktrees", label: "Worktrees", what: "agent worktrees and their build output" },
    { view: "artifacts", label: "Artifacts", what: "build directories and Poetry virtualenvs" },
    { view: "docker", label: "Docker", what: "images, volumes and build cache" },
  ];
  const offered = targets.filter((t) => !hidden.has(t.view));
  if (offered.length === 0) return null;

  return (
    <Panel
      title="Where Headstate can reclaim space"
      // Says what this is and what it is NOT, in the subtitle, because the
      // reader's next question after a 73% bar is "how much of that is
      // mine" and this panel deliberately does not answer it. Leaving that
      // unsaid would make the panel look like a measurement that failed.
      subtitle="The pages that can remove things. Nothing here is measured — each page reports its own sizes"
    >
      <div className="flex flex-wrap gap-2">
        {offered.map((t) => (
          <button
            key={t.view}
            type="button"
            // Plain `setView`, with no `setFilter` after it -- which is why
            // #920's ordering trap does not apply. That bug was `setFilter`
            // writing into `filtersByView[state.view]`, the view being left;
            // with no filter to write there is no order to get wrong. The
            // three destinations are machine-wide lists and there is nothing
            // on a disk volume to narrow them BY: a volume is not a
            // repository, so inventing a filter here would be a jump that
            // pre-narrowed the reader's view on no evidence.
            onClick={() => setView(t.view)}
            className="tap-target flex flex-col items-start rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-left hover:bg-[#21262d]"
          >
            <span className="text-sm text-[#58a6ff]">{t.label} →</span>
            <span className="text-xs text-[#8b949e]">{t.what}</span>
          </button>
        ))}
      </div>
      {/* The honest caveat, and the reason there is no total. Without it a
          reader could take three links as an implied claim that these are
          where the 652 GB went -- which nothing here measured. */}
      <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
        Whether any of this accounts for the figures above is not something this page
        measured. Each page counts and removes its own, which is why the sizes are there
        rather than here.
      </p>
    </Panel>
  );
}

/// One interface's throughput over the last 24 hours (#719).
///
/// # Why a rate needs more care than a percentage
///
/// Every other chart on this page plots a value the sample already
/// carries. This one plots a DIFFERENCE between two samples, and a
/// difference has two ways to lie that a direct reading does not:
///
/// 1. A pair spanning a period the app was closed differences two
///    readings hours apart, which would draw a whole day's traffic as
///    one minute's throughput.
/// 2. A counter that RESET -- an interface going down, a reboot --
///    differences to a large negative number.
///
/// Both are handled in `interfaceRates`, and both come back the same
/// way: as a `null` value. That is deliberate, because `splitOnGaps`
/// already breaks a run on a null, so a reset draws as a gap without
/// this component knowing anything about resets. The alternative --
/// clamping a reset to zero -- would render a reboot as a quiet
/// minute, which is a claim about the traffic rather than an admission
/// that the counter is no longer comparable.
function InterfaceHistory({
  name,
  rx,
  tx,
  now,
}: {
  name: string;
  rx: RatePoint[];
  tx: RatePoint[];
  now: number;
}) {
  // ONE ceiling for both directions, so in and out are visually
  // comparable. Scaling each to its own peak would draw a trickle of
  // upload at the same height as a saturated download, which is the
  // opposite of what a reader takes from two stacked charts.
  const peak = Math.max(peakRate(rx) ?? 0, peakRate(tx) ?? 0);
  const resets = [...rx, ...tx].filter((p) => p.reason === "reset").length;

  if (peak <= 0) {
    return (
      <div>
        <div className="text-sm text-[#e6edf3]">{name}</div>
        <p className="mt-1 text-xs">
          <NotMeasured>— no throughput recorded yet</NotMeasured>
        </p>
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate text-sm text-[#e6edf3]">{name}</span>
        <span className="shrink-0 text-xs tabular-nums text-[#8b949e]">
          peak {formatRate(peak)}
        </span>
      </div>
      <div className="mt-1 flex flex-col gap-1">
        <div>
          <div className="text-xs text-[#8b949e]">In</div>
          <Sparkline points={rx} max={peak} label={`${name} received`} now={now} />
        </div>
        <div>
          <div className="text-xs text-[#8b949e]">Out</div>
          <Sparkline
            points={tx}
            max={peak}
            label={`${name} sent`}
            now={now}
            color="#a371f7"
          />
        </div>
      </div>
      {/* Named rather than left as an unexplained break in the line. A
          reader who sees a gap assumes the app was closed; a counter
          reset is a different fact about the machine, and it is the one
          the user can act on -- it means the interface went down or the
          machine rebooted. */}
      {resets > 0 ? (
        <p className="mt-1 text-xs text-[#8b949e]">
          {resets === 1 ? "One break" : `${resets} breaks`} in the line{" "}
          {resets === 1 ? "is" : "are"} a counter reset — this interface went
          down, or the machine rebooted. Byte counts start again from zero, so
          there is no rate to draw across it.
        </p>
      ) : null}
    </div>
  );
}

/// How many processes the network table lists at once.
///
/// Eight, matching `TOP_N` on the CPU and Memory pages for the same
/// reasons argued there: it outlasts one application (a browser is five
/// or six processes), it fits without scrolling on a phone, and below
/// the top few essentially everything is moving nothing. Deliberately
/// the same number, so a reader moving between the three pages is
/// comparing lists of the same shape.
const TOP_NET_PROCESSES = 8;

/// Which processes are using the network (#718).
///
/// # This panel is the reason the page has a cadence of its own
///
/// Every other reading on this page rides the five-second health poll.
/// This one costs ~5 SECONDS per reading -- `nettop` samples for a
/// whole interval before printing, and no flag shortens it -- which is
/// the entire poll interval, so it runs on `NET_PROCESSES_POLL_MS`
/// instead and only while this component is mounted. See the hook,
/// where the cadence is argued, and `health::netproc` for the
/// measurements.
///
/// # The two things this panel must say out loud
///
/// Both are consequences of that cost, and both would read as bugs if
/// left unexplained:
///
/// 1. **The first reading takes about five seconds.** A spinner sitting
///    for five seconds with no explanation is its own bug, so the
///    waiting state says what is happening and how long it takes.
/// 2. **One reading is not a rate.** The counts are cumulative since
///    each process started, so the first reading can only be ordered by
///    lifetime totals -- which ranks a process that pulled 6 GB last
///    week above one saturating the link right now. The panel says so
///    while that is what it is showing, and switches wording once it
///    has two readings to difference.
function NetworkProcesses() {
  // `true`: this component only mounts on the Network page, so the
  // poll starts when the page opens and stops when it closes. That is
  // the whole containment strategy for a five-second subprocess.
  const q = useNetworkProcesses(true);

  // The last TWO readings, because a rate needs two and TanStack hands
  // out one. Each is stored with its ARRIVAL TIME rather than with the
  // nominal cadence: a rate must be divided by the interval that
  // actually elapsed, and this one slips whenever the machine is busy
  // or the laptop was asleep between readings.
  //
  // Adjusted DURING RENDER rather than in an effect, which is React's
  // own "adjusting state when a prop changes" pattern: React discards
  // the render and re-runs this component immediately, before anything
  // is committed to the DOM, so there is no flash of a table computed
  // from the stale pair. An effect would paint the old rates once
  // first, and on this panel that is visible -- the whole table's
  // numbers would change a frame after the reading landed.
  const [seen, setSeen] = useState<{
    prev: NetProcessReading | null;
    cur: NetProcessReading | null;
  }>({ prev: null, cur: null });
  // Keyed on the ARRIVAL TIMESTAMP, not on the array identity. TanStack
  // re-renders with the same `data` reference for reasons that are not
  // a new reading, and rotating on one of those would difference a
  // reading against itself and draw 0 B/s across the whole table. It is
  // also what stops this render-phase update from looping: the
  // condition is false on the re-render it causes.
  const arrived = q.dataUpdatedAt;
  const fresh =
    q.data !== undefined && arrived !== 0 && seen.cur?.t !== arrived
      ? { prev: seen.cur, cur: { t: arrived, processes: q.data } }
      : seen;
  if (fresh !== seen) setSeen(fresh);
  const { prev, cur } = fresh;

  const rates = useMemo(
    () => (prev === null || cur === null ? null : netProcessRates(prev, cur)),
    [prev, cur],
  );

  // Rates when there are two readings, lifetime totals when there is
  // one. Both are sorted by the sum of the two directions: a process
  // that is only uploading and one that is only downloading are equally
  // interesting, and ranking by received alone would bury the first.
  const rows = useMemo(() => {
    if (rates !== null) {
      return [...rates]
        .sort((a, b) => b.in_rate + b.out_rate - (a.in_rate + a.out_rate))
        .slice(0, TOP_NET_PROCESSES);
    }
    if (cur === null) return [];
    return [...cur.processes]
      .sort((a, b) => b.bytes_in + b.bytes_out - (a.bytes_in + a.bytes_out))
      .slice(0, TOP_NET_PROCESSES)
      .map((p) => ({
        name: p.name,
        pid: p.pid,
        in_rate: null,
        out_rate: null,
        bytes_in: p.bytes_in,
        bytes_out: p.bytes_out,
      }));
  }, [rates, cur]);

  if (q.isError) {
    return (
      <p className="text-sm text-[#8b949e]">
        {/* The highest-value retry in the view (#946). This read costs ~5
            seconds of `nettop`, which is why its poll cadence is
            deliberately slow -- so waiting out the poll is the most
            expensive of the four "just wait" recourses and the manual
            re-issue is worth the most. The cadence is untouched. */}
        Could not read the per-process network table: {errorMessage(q.error)}
        <InlineRetry onRetry={() => void q.refetch()} />
      </p>
    );
  }

  // The five-second wait, named. This is the state that would otherwise
  // be an unexplained spinner, and it is a state the user will see
  // every single time they open this page.
  if (cur === null) {
    return (
      <p className="text-sm leading-relaxed text-[#8b949e]">
        Measuring… this takes about {NET_PROCESSES_SAMPLE_MS / 1000} seconds.
        macOS only reports per-process network use by sampling for a full
        interval before it answers, so the first figures cannot arrive sooner.
        Nothing is stuck.
      </p>
    );
  }

  if (cur.processes.length === 0) {
    // Not "no processes are using the network" — on the platforms that
    // return nothing here, nothing was measured at all. Which of the
    // two it is depends on the platform, and #705's precedent is that
    // an evidenced "cannot be read unprivileged" is said plainly rather
    // than rendered as an empty table.
    return (
      <p className="text-sm leading-relaxed">
        <NotMeasured />
        <span className="ml-1 block text-[#8b949e]">
          Only macOS reports network use per process without elevated
          privileges. On Linux <code>/proc/&lt;pid&gt;/net/dev</code> is
          per-namespace rather than per-process — every process in the root
          namespace reads the same whole-machine totals the panels below
          already show — and the tools that do attribute traffic
          (<code>nethogs</code>, eBPF) need <code>CAP_NET_ADMIN</code> or more.
          On Windows the unprivileged route sees TCP only, missing the UDP and
          QUIC that carry most of a browser&apos;s traffic, so it is not built
          rather than built wrong.
        </span>
      </p>
    );
  }

  const rated = rates !== null;

  if (rows.length === 0) {
    // Reachable, and not the same fact as an empty reading above: every
    // process in the newer reading failed to pair with the older one.
    // The realistic cause is a long stall between readings -- a laptop
    // that slept -- after which the whole table has turned over. Said
    // as a sentence rather than as an empty table under headings,
    // which would read as a panel that failed to paint.
    return (
      <p className="text-sm leading-relaxed text-[#8b949e]">
        None of the {cur.processes.length} processes in this reading were
        present in the previous one, so none of them has an interval to
        measure. That usually means a long stall between readings — the whole
        table has turned over. Rates return with the next pair.
      </p>
    );
  }

  return (
    <div>
      <table className="w-full text-sm">
        <thead>
          <tr className="text-left text-xs text-[#8b949e]">
            <th className="font-normal">Process</th>
            <th className="font-normal text-right">PID</th>
            {/* The headings change with what the numbers MEAN. Showing
                a lifetime total under a heading that says "/s" is the
                misreading this whole panel is arranged to avoid. */}
            <th className="font-normal text-right">{rated ? "In" : "Received"}</th>
            <th className="font-normal text-right">{rated ? "Out" : "Sent"}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            // Keyed on the PID where there is one: several processes of
            // one application share a name, and keying on the name
            // would make React reuse one row's DOM for another process.
            <tr key={r.pid === null ? `n:${r.name}` : `p:${r.pid}`}>
              <td className="max-w-0 truncate pr-2 text-[#e6edf3]" title={r.name}>
                {r.name}
              </td>
              <td className="text-right tabular-nums text-[#8b949e]">
                {/* Absent is not zero, and it is not a guess either. A
                    row whose label carried no parseable PID gets a dash
                    rather than a fabricated number a reader might paste
                    into `kill`.

                    Which is also why the copy button is ABSENT there
                    rather than disabled (#943): this comment is the one
                    that names pasting into `kill` as the workflow, so a
                    control offering to copy a dash into it would be the
                    fabricated number arriving by another route. */}
                {r.pid === null ? <NotMeasured /> : <CopyPid pid={r.pid} />}
              </td>
              <td className="text-right tabular-nums text-[#e6edf3]">
                {r.in_rate === null ? formatSize(r.bytes_in) : formatRate(r.in_rate)}
              </td>
              <td className="text-right tabular-nums text-[#e6edf3]">
                {r.out_rate === null ? formatSize(r.bytes_out) : formatRate(r.out_rate)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {rated ? (
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          Rates over the last interval, from the difference between two
          readings. The {rows.length} busiest of {cur.processes.length}{" "}
          processes with network accounting — a process that started or exited
          between the two readings has no interval to measure and is not
          listed, rather than being shown at zero.
        </p>
      ) : (
        // The single-reading state, and the sentence that keeps it from
        // being read as a rate. This is the ~5-to-20-second window
        // between the page opening and the second reading landing.
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          These are <em>lifetime totals</em> since each process started, not
          current speeds — macOS reports cumulative counters, so a single
          reading cannot be a rate. Rates appear once a second reading lands,
          about {(NET_PROCESSES_POLL_MS + NET_PROCESSES_SAMPLE_MS) / 1000}{" "}
          seconds from now. Until then a process that moved a lot last week
          outranks one saturating the link right now.
        </p>
      )}
      {/* The cadence, and nothing defending it (#1088). What followed
          the colon argued the sampling cost to a developer and named
          "the poll driving the rest of this view" -- an internal the
          reader cannot see, act on, or check. The cadence itself is the
          fact: it tells the reader how fresh these numbers are and that
          they stop when the page closes. */}
      <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
        Re-read every {NET_PROCESSES_POLL_MS / 1000} seconds, and only while
        this page is open.
      </p>
    </div>
  );
}

/// The Network page: what is using the network, throughput over 24
/// hours, and totals since boot.
///
/// # Why the process panel comes first
///
/// The same argument the CPU page makes: the overview already answered
/// "is the machine using the network", and someone who clicked through
/// did so because that number was interesting. The next thing they want
/// is the NAME of what is doing it (#718). The interface charts below
/// are the aggregate they already saw, in more detail.
///
/// # Why the history panel comes before the totals
///
/// The totals were all this page had (#719), and a cumulative counter
/// that only ever rises cannot show a spike, a stall, or a pattern: a
/// machine that pulled 40 GB overnight and one that pulled 40 GB over a
/// month look identical. The totals are still worth showing -- "how
/// much has this machine moved" is a real question -- but they are the
/// weaker answer, so they sit below the one that shows shape over time.
function NetworkDetail({
  sample: s,
  samples,
  sampledAt,
}: {
  sample: HealthSample;
  samples: HealthSample[];
  sampledAt: number;
}) {
  // The busiest first, so the interface that carried the traffic is the
  // first row rather than wherever the platform happened to list it.
  // Sorted here rather than in Rust because, unlike the process lists,
  // nothing is being dropped -- every interface is shown, so the order
  // is presentation and not selection.
  const interfaces = useMemo(
    () => [...s.networks].sort((a, b) => b.rx_bytes + b.tx_bytes - (a.rx_bytes + a.tx_bytes)),
    [s.networks],
  );
  // Differenced from the stored series. Gaps and counter resets both
  // come back as null values, which `Sparkline` already draws as
  // breaks -- see `InterfaceHistory`.
  const rates = useMemo(() => interfaceRates(samples), [samples]);
  const totalRx = interfaces.reduce((n, i) => n + i.rx_bytes, 0);
  const totalTx = interfaces.reduce((n, i) => n + i.tx_bytes, 0);

  return (
    <div className="flex flex-col gap-4">
      {/* First, for the same reason the CPU page puts its processes
          first: the overview already said the machine is using the
          network, and the question that brought the reader here is
          which program. */}
      <Panel
        title="What is using the network"
        subtitle="Per process, on this page's own slower cadence"
      >
        <NetworkProcesses />
      </Panel>

      <Panel
        title="Throughput"
        subtitle="Per interface, over the last 24 hours"
      >
        {interfaces.length === 0 ? (
          <p className="text-sm">
            <NotMeasured />
          </p>
        ) : (
          <div className="flex flex-col gap-4">
            {interfaces.map((n) => {
              const r = rates.get(n.name);
              return (
                <InterfaceHistory
                  key={n.name}
                  name={n.name}
                  rx={r?.rx ?? []}
                  tx={r?.tx ?? []}
                  now={sampledAt}
                />
              );
            })}
          </div>
        )}
        {/* Says what the breaks mean, once, above the charts that show
            them. Two different facts share the same visual treatment --
            an unmeasured period and a counter reset -- and a blank
            stretch nobody explains reads as a bug in the chart. */}
        <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
          Rates come from differencing the cumulative counters between
          samples, so a break in a line is either a period Headstate was not
          running or a counter that reset. Neither is drawn as a rate,
          because a period nobody measured has none.
        </p>
      </Panel>

      <Panel title="Since boot" subtitle="Across every interface">
        <div className="flex flex-wrap gap-6">
          <Stat label="Received" value={formatSize(totalRx)} />
          <Stat label="Sent" value={formatSize(totalTx)} />
          <Stat label="Interfaces" value={`${interfaces.length}`} />
        </div>
      </Panel>

      <Panel title="Per interface" subtitle="Busiest first">
        {interfaces.length === 0 ? (
          <p className="text-sm">
            <NotMeasured />
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {interfaces.map((n) => {
              const total = n.rx_bytes + n.tx_bytes;
              // A share of the machine's total traffic, which is what
              // makes a list of a dozen interfaces readable: it says
              // which one actually carried anything. `percentOf`
              // returns null on a zero total rather than NaN -- a
              // freshly booted machine really can have moved no bytes.
              const share = percentOf(total, totalRx + totalTx);
              return (
                <div key={n.name}>
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="truncate text-sm text-[#e6edf3]">{n.name}</span>
                    <span className="shrink-0 text-xs tabular-nums text-[#8b949e]">
                      {formatSize(n.rx_bytes)} in · {formatSize(n.tx_bytes)} out
                    </span>
                  </div>
                  <div className="mt-1">
                    <Bar
                      percent={share}
                      label={`${n.name} share of total traffic`}
                    />
                  </div>
                  <div className="mt-1 text-xs tabular-nums text-[#8b949e]">
                    {share === null ? (
                      <NotMeasured />
                    ) : (
                      `${formatSize(total)} total (${share.toFixed(0)}% of all traffic)`
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
        {/* The bar here is a SHARE, not a utilisation. Everywhere else
            on this page a bar means "how full", and using the same
            shape for a different meaning without saying so is how a
            reader concludes an interface is 80% saturated. */}
        <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
          Totals since the machine booted, not current speeds — the
          Throughput panel above has the rates. The bars show each
          interface&apos;s share of all traffic, not how saturated it is.
        </p>
      </Panel>
    </div>
  );
}
/// The GPU page: utilization over time, memory, and the pipeline
/// stages the overview collapses (#717).
///
/// # Why this page exists when the overview already has a GPU panel
///
/// #705 put GPU on the overview, where there is room for one number per
/// device. #710 gave every other class a page and left GPU out because
/// #705 had not landed. So GPU was the one class with a panel and no
/// page, and the three things a page can afford that a panel cannot are
/// exactly what was missing: a full-width 24-hour chart per device, the
/// memory figures with the unified-memory caveat stated at length, and
/// the per-renderer breakdown the overview has to collapse.
///
/// # A machine with no GPU never reaches here
///
/// The sidebar does not offer the row and `SystemHealthPage` does not
/// route to it -- see the guard there. This component therefore assumes
/// at least one GPU and does not render an "no GPU found" state, which
/// would be the empty page #717 explicitly refuses.
///
/// # Per-platform, and why the page differs between them
///
/// The verdicts are #705's and this page respects them rather than
/// re-deciding them:
///
/// - **macOS.** `ioreg` answers with a device figure, a renderer
///   figure, a tiler figure and the unified-memory pair. Every section
///   below has content.
/// - **Linux/AMD.** `amdgpu` sysfs answers with one busy percentage and
///   discrete VRAM. The stages section says the platform does not split
///   them, rather than repeating the device number under three labels.
/// - **Linux/Intel, NVIDIA, Windows.** No unprivileged reading exists
///   or none was built, so `gpus` is empty and there is no page at all.
function GpuDetail({
  sample: s,
  samples,
  sampledAt,
}: {
  sample: HealthSample;
  samples: HealthSample[];
  sampledAt: number;
}) {
  // One series per device, by INDEX rather than by name. A machine can
  // have two cards with the same reported name, and matching on the
  // name would draw one device's history under both. Index is what the
  // Rust side's ordering guarantees -- `gpu::read` sorts its Linux
  // cards and `ioreg` walks the tree in a fixed order -- so position is
  // stable between samples in a way a name is not.
  //
  // A sample taken before this GPU existed, or on a platform that
  // reported no GPU, has no entry at that index and contributes `null`,
  // which `splitOnGaps` breaks the line across. That is the correct
  // reading: the app was running and did not measure this device, which
  // is not the same as a device that was idle.
  const series = useMemo(
    () =>
      s.gpus.map((_, i) =>
        samples.map((x) => ({
          t: Date.parse(x.sampled_at),
          v: x.gpus[i]?.utilization_percent ?? null,
        })),
      ),
    [samples, s.gpus],
  );

  return (
    <div className="flex flex-col gap-4">
      {s.gpus.map((g, i) => (
        <GpuDeviceDetail
          key={`${g.name}-${i}`}
          gpu={g}
          series={series[i]}
          sampledAt={sampledAt}
          // Only shown when there are two: on a single-GPU machine an
          // index would be a number that distinguishes nothing.
          index={s.gpus.length > 1 ? i : null}
        />
      ))}

      {/* Why there is no process list on this page, said once at the
          bottom rather than as an empty panel.

          The CPU and Memory pages name the processes responsible, and a
          reader arriving here reasonably expects the same. The kernel
          does not attribute GPU work per process the way it attributes
          CPU and resident memory: on macOS the per-process GPU counters
          live behind `powermetrics`, which needs sudo, and the same
          privilege wall that keeps degrees off the Power page keeps
          these off this one. Saying so is better than the reader
          concluding the panel failed to load. */}
      {/* TRIMMED, not cut (#1088). The privilege constraint STAYS: it
          is a true fact about the reader's machine, and it is what stops
          them concluding the panel broke. Only the trailing "so
          Headstate reports..." went -- that half was the app explaining
          its own conduct, which the reader neither asked for nor can act
          on. */}
      <p className="text-xs leading-relaxed text-[#8b949e]">
        There is no list of what is using the GPU. Unlike CPU and memory, the
        system does not attribute graphics work to individual processes without
        elevated privileges — the same wall that keeps temperatures off the
        Power page.
      </p>
    </div>
  );
}

/// One GPU, at page depth.
///
/// Separate from `GpuCard` (the overview's) rather than a mode of it.
/// The two answer different questions -- "is the GPU busy" in three
/// figures, versus "what is it doing and what has it been doing" across
/// three panels -- and a single component switched on a `detail` flag
/// would be two layouts sharing a name, which is how the overview ends
/// up quietly changing when the page does.
function GpuDeviceDetail({
  gpu: g,
  series,
  sampledAt,
  index,
}: {
  gpu: HealthGpu;
  series: Point[];
  sampledAt: number;
  /// The device's position, on a machine with more than one. `null` on
  /// a single-GPU machine, where a "#0" would distinguish nothing.
  index: number | null;
}) {
  const memPct =
    g.memory_used === null || g.memory_total === null
      ? null
      : percentOf(g.memory_used, g.memory_total);

  // `?? null` rather than a truthiness check: these are OPTIONAL on the
  // wire (an older desktop, or a sample stored before #717) as well as
  // nullable, and `undefined` and `null` mean the same thing here --
  // the platform did not report a stage figure. Both must render as
  // "not measured", and neither may become a 0.
  const renderer = g.renderer_percent ?? null;
  const tiler = g.tiler_percent ?? null;
  const hasStages = renderer !== null || tiler !== null;

  return (
    <>
      <Panel
        title={index === null ? "Utilization" : `${g.name} — utilization`}
        subtitle={
          index === null
            ? "What the graphics device is doing right now"
            : `Device ${index + 1}`
        }
      >
        <div className="flex flex-wrap gap-6">
          <Stat label="Device" value={g.name} />
          {/* Null rather than 0 for a GPU that reported no utilization:
              an idle GPU and an unreadable one are opposite answers,
              and this is the field where they look most alike. */}
          <Stat
            label="Utilization"
            value={
              g.utilization_percent === null
                ? null
                : `${g.utilization_percent.toFixed(0)}%`
            }
          />
        </div>
        {g.utilization_percent !== null ? (
          <div className="mt-3">
            <Bar percent={g.utilization_percent} label={`${g.name} utilization`} />
          </div>
        ) : null}

        <div className="mt-4">
          <div className="text-xs text-[#8b949e]">
            Utilization over the last 24 hours
          </div>
          {/* The same treatment CPU and memory get, gaps and all -- the
              chart is the reason this class needed a page, since the
              overview has room for a sparkline only under the first
              device. */}
          <Sparkline
            points={series}
            max={100}
            label={index === null ? "GPU" : `GPU ${index + 1}`}
            color="#a371f7"
            now={sampledAt}
          />
          <GapNote />
        </div>
      </Panel>

      <Panel
        title="Pipeline stages"
        subtitle="Where inside the GPU the work is landing"
      >
        {hasStages ? (
          <>
            <div className="flex flex-wrap gap-6">
              <Stat
                label="Renderer"
                value={renderer === null ? null : `${renderer.toFixed(0)}%`}
              />
              <Stat
                label="Tiler"
                value={tiler === null ? null : `${tiler.toFixed(0)}%`}
              />
              <Stat
                label="Device"
                value={
                  g.utilization_percent === null
                    ? null
                    : `${g.utilization_percent.toFixed(0)}%`
                }
                hint="The figure the overview shows"
              />
            </div>
            <div className="mt-3 flex flex-col gap-2">
              {renderer !== null ? (
                <div className="flex items-center gap-2">
                  <span className="w-16 shrink-0 text-xs text-[#8b949e]">Renderer</span>
                  <Bar percent={renderer} label={`${g.name} renderer utilization`} />
                  <span className="w-10 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                    {renderer.toFixed(0)}%
                  </span>
                </div>
              ) : null}
              {tiler !== null ? (
                <div className="flex items-center gap-2">
                  <span className="w-16 shrink-0 text-xs text-[#8b949e]">Tiler</span>
                  <Bar percent={tiler} label={`${g.name} tiler utilization`} />
                  <span className="w-10 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                    {tiler.toFixed(0)}%
                  </span>
                </div>
              ) : null}
            </div>
            {/* What the two stages ARE, which is the whole reason this
                panel is worth its space. Without it, three percentages
                that do not sum to anything read as a bug -- they are
                three independent hardware stages, not a breakdown of
                one total, and a reader who adds them will think so. */}
            <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
              This GPU is a <em>tile-based deferred renderer</em>: the tiler
              sorts geometry into screen tiles and the renderer shades them.
              They are separate stages that can saturate independently, so the
              three figures do not add up to anything — each is that stage&apos;s
              own busy time. A device pinned by its tiler is short of geometry
              throughput; one pinned by its renderer is short of shading.
            </p>
          </>
        ) : (
          // NOT "Not measured" rows for the two stages. This platform
          // reports one busy figure and has no stages to report, which
          // is a different fact from a stage we failed to read -- the
          // same distinction the whole view is built on, applied to a
          // capability rather than to a reading.
          <p className="text-sm leading-relaxed text-[#8b949e]">
            This platform reports one utilization figure for the whole device
            and does not break it down by pipeline stage. macOS reports a
            renderer and a tiler figure separately; the AMD driver on Linux
            publishes a single busy percentage, which is the number above.
          </p>
        )}
      </Panel>

      <Panel
        title="Memory"
        subtitle={g.unified_memory ? "Shared with the system" : "The device's own VRAM"}
      >
        <div className="flex flex-wrap gap-6">
          <Stat
            label="In use"
            value={g.memory_used === null ? null : formatSize(g.memory_used)}
            hint={
              g.unified_memory && g.memory_used !== null ? "Shared with system" : undefined
            }
          />
          <Stat
            label={g.unified_memory ? "Allocated pool" : "Total VRAM"}
            value={g.memory_total === null ? null : formatSize(g.memory_total)}
          />
          <Stat
            label="Of the pool"
            value={memPct === null ? null : `${memPct.toFixed(0)}%`}
          />
        </div>
        {memPct !== null ? (
          <div className="mt-3">
            <Bar percent={memPct} label={`${g.name} memory in use`} />
          </div>
        ) : null}
        {g.unified_memory ? (
          // The caveat #705 established, with the room a page has to
          // state it properly. The overview says it in three lines
          // because a panel must; here the specific arithmetic error is
          // named, because this page is where someone comparing the two
          // figures ends up.
          <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
            This GPU uses <em>unified memory</em>: it has no memory of its own
            and shares one physical pool with the CPU. The figures above are a
            share of the very memory the Memory page reports —{" "}
            <span className="text-[#c9d1d9]">not additional memory</span>. Adding
            them to the system total would describe a machine with more RAM than
            this one has.
          </p>
        ) : (
          <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
            This device has its own VRAM, separate from system memory, so these
            figures are additional to what the Memory page reports rather than a
            share of it.
          </p>
        )}
      </Panel>
    </>
  );
}


/// The capacity panel, and the reading it must not mis-describe (#772).
///
/// # A capacity over 100% is a healthy battery, not a bug
///
/// This panel used to hardcode one sentence -- "a battery at N% of its
/// original capacity still charges to 100%, it just holds less than it
/// once did" -- which is correct below 100 and false at or above it.
/// The reading that opened #772 was **103%**, on a new laptop, under
/// copy telling its owner the cell had degraded.
///
/// The number itself is right and is not touched. `DesignCapacity` is a
/// nameplate figure the manufacturer guarantees rather than a ceiling,
/// so cells routinely ship a little over it. `capacityMeaning` in
/// `lib/health` picks the sentence; the branch lives there because it
/// is a claim about what is true, and it is tested without rendering.
///
/// # The bar has its own scale for the same reason
///
/// Every other bar on this page runs 0-100 and colours a high reading
/// as pressure. Both halves of that are wrong here: the value can
/// exceed 100, and high is HEALTHY. Reusing `Bar` would peg a 103% cell
/// at a saturated red — "renders as an error state", which #772 asks
/// for by name. So the track runs to `CAPACITY_BAR_MAX` and the colour
/// comes from `capacityColor`, which escalates downward.
function CapacityPanel({
  percent,
  cycles,
}: {
  percent: number;
  cycles: number | null;
}) {
  const cycleNote = cycleMeaning(percent, cycles);
  const shown = percent.toFixed(0);
  return (
    <Panel title="Battery capacity" subtitle="How much the cell can still hold">
      <div className="flex flex-wrap gap-6">
        <Stat label="Of original capacity" value={`${shown}%`} />
        <Stat label="Charge cycles" value={cycles === null ? null : `${cycles}`} />
      </div>
      <div className="mt-3">
        {/* Not `Bar`: see the note above. The track is wider than the
            reading's nominal maximum, so a cell above nameplate sits
            comfortably inside it rather than overflowing its own
            container.

            `relative`, and no longer `overflow-hidden`, so the
            nameplate tick below can be positioned against this track
            without being clipped at its own edge. The FILL carries the
            rounding and the clipping instead -- it is the only child
            that was ever being clipped. */}
        <div
          className="relative h-2 w-full rounded-full bg-[#30363d]"
          role="meter"
          aria-valuenow={Math.round(percent)}
          aria-valuemin={0}
          aria-valuemax={CAPACITY_BAR_MAX}
          aria-label="Battery capacity relative to design"
        >
          <div
            className="h-full rounded-full"
            style={{
              width: `${capacityBarFill(percent)}%`,
              backgroundColor: capacityColor(percent),
            }}
          />
          {/* The nameplate figure, drawn ON the track at the position
              the fill would reach if the cell were exactly at 100%.
              #981: this used to be a `justify-between` label pinned
              flush right, i.e. at the 120% end of the track -- so a
              cell at exactly 100% drew its fill 16.67% of the track
              SHORT of the mark named "100%", and a perfectly healthy
              battery read as falling short of nameplate. Every reading
              was under-reported by the same 1.2x.

              Positioned from `capacityBarFill(100)` rather than a
              hardcoded 83.33%, so the mark and the fill cannot drift
              apart if `CAPACITY_BAR_MAX` is ever retuned. */}
          <div
            className="absolute -top-0.5 h-3 w-px bg-[#8b949e]"
            style={{ left: `${NAMEPLATE_MARK}%` }}
            aria-hidden="true"
          />
        </div>
        {/* The 100% mark, named. Without it the bar is unreadable: a
            fill that stops four-fifths along means nothing unless the
            reader knows where the nameplate figure sits, and that is
            precisely the comparison this panel is about.

            The label rides at the tick's position rather than at the
            container's right edge, because the two have to name the
            same point to calibrate anything. `-translate-x-1/2` centres
            it on the tick, which at the 83.33% mark of a 326px phone
            panel puts its right edge almost exactly at the panel's --
            the same place it used to sit, now for the right reason. */}
        <div className="relative mt-1 h-4 text-[11px] text-[#8b949e]">
          <span className="absolute left-0">0%</span>
          <span
            className="absolute -translate-x-1/2 whitespace-nowrap"
            style={{ left: `${NAMEPLATE_MARK}%` }}
          >
            Rated capacity: 100%
          </span>
        </div>
      </div>
      <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
        This is <em>not</em> the charge above. It is how much the battery can
        hold now compared with when it was made. {capacityMeaning(percent)}
        {cycleNote ? ` ${cycleNote}` : ""}
      </p>
    </Panel>
  );
}

/// The battery's power flow: how fast, and which way (#773).
///
/// # The gap this closes
///
/// #720 added an alert for "discharging while plugged in" and it fires
/// correctly. But a user who has just been told their machine is
/// draining on mains power had nowhere in this app to see HOW FAST, or
/// to watch it while working out why. This is that number.
///
/// # The sign carries the direction
///
/// Positive is into the cell, negative out of it, normalised in Rust so
/// neither platform's encoding leaks up here. See `health::PowerFlow`
/// -- and `collect::power_flow` for the unsigned-`Amperage` trap that
/// makes a discharge read as roughly 18 quintillion milliamps if it is
/// taken at face value.
///
/// # One reading that is a fault rather than a state
///
/// Discharging is ordinary; discharging while `on_ac` is not. That
/// combination is the only thing on this card coloured red, and it is
/// said in words as well as in colour -- the same #581/#582 rule the
/// pressure cards keep.
function PowerFlowCard({
  power,
  onAc,
}: {
  power: HealthPowerFlow;
  onAc: boolean;
}) {
  const dir = powerDirection(power.watts);
  const drainingOnAc = dir === "discharging" && onAc;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-6">
        <div className="min-w-24">
          <div className="text-xs text-[#8b949e]">Power flow</div>
          <div
            className="text-lg font-semibold tabular-nums"
            style={{ color: powerColor(power.watts, onAc) }}
          >
            {formatWatts(power.watts)}
          </div>
          {/* The direction in words as well as in the sign and the
              colour. A minus sign is easy to miss at a glance and
              colour is never the only cue on this page. */}
          <div className="text-xs capitalize text-[#8b949e]">
            {dir === "idle" ? "Not moving" : dir}
          </div>
        </div>
        {/* The two factors behind the wattage. It is a PRODUCT, and a
            reader who finds the watts implausible has no way to tell
            which half is wrong without both. */}
        <Stat label="Current" value={`${power.milliamps} mA`} />
        <Stat label="Voltage" value={`${(power.millivolts / 1000).toFixed(2)} V`} />
      </div>
      {drainingOnAc ? (
        // The #720 condition, said on the page that shows the rate.
        // This is the whole reason #773 asks for the figure: the alert
        // says it is happening, and this says how fast.
        <p className="text-xs leading-relaxed text-[#f85149]">
          Losing charge while plugged in. The machine is drawing more than the
          adapter supplies, or the adapter is not really charging.
        </p>
      ) : null}
    </div>
  );
}

/// Why a platform reports no power flow, said rather than left blank.
///
/// #705's rule, applied to the one field this issue adds: a panel that
/// silently vanishes on Windows looks like a bug, and a 0 W would be
/// worse -- zero watts is a real reading a full battery on mains sits
/// at, so it is indistinguishable from a measurement.
function PowerNotMeasured() {
  return (
    <>
      <p className="text-sm">
        <NotMeasured />
      </p>
      <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
        Headstate reads the charge and drain rate on macOS (through IOKit) and
        on Linux (through <code>/sys/class/power_supply</code>), both without
        elevated privileges. Windows publishes it too, but through an interface
        Headstate does not yet read — so nothing is claimed here rather than a
        zero being shown, since zero watts is itself a real reading.
      </p>
    </>
  );
}

/// The 24-hour power-flow chart (#773).
///
/// # Two charts, not one, because the axis has a zero in the middle
///
/// Every other sparkline on this page runs 0 to a positive maximum.
/// This series crosses zero, and `Sparkline` clamps at 0 -- so a single
/// chart would draw every discharge flat along the bottom, hiding
/// exactly the half a user came to look at.
///
/// Splitting into "into the battery" and "out of the battery", each
/// scaled to the same peak magnitude, keeps both halves visible and
/// directly comparable. A sample that was flowing the other way
/// contributes `null` to the chart it is not on, which `splitOnGaps`
/// breaks the line across -- which is correct: it was NOT zero watts in
/// that direction, it was flowing the other way.
///
/// This is #719's interface-throughput shape, for the same reason it
/// took that shape: two directions of one quantity, one shared ceiling.
function PowerHistory({ series, now }: { series: Point[]; now: number }) {
  const peak = peakWatts(series);
  const charge = useMemo(
    () => series.map((p) => ({ t: p.t, v: p.v !== null && p.v > 0 ? p.v : null })),
    [series],
  );
  const drain = useMemo(
    () => series.map((p) => ({ t: p.t, v: p.v !== null && p.v < 0 ? -p.v : null })),
    [series],
  );

  if (peak === null || peak <= 0) {
    return (
      <p className="mt-1 text-xs">
        <NotMeasured>— no power readings in the last 24 hours</NotMeasured>
      </p>
    );
  }

  return (
    <div className="mt-1 flex flex-col gap-1">
      <div>
        <div className="text-xs text-[#8b949e]">Into the battery (charging)</div>
        <Sparkline points={charge} max={peak} label="Battery charging" now={now} color="#3fb950" />
      </div>
      <div>
        <div className="text-xs text-[#8b949e]">Out of the battery (draining)</div>
        <Sparkline points={drain} max={peak} label="Battery draining" now={now} color="#d29922" />
      </div>
      <div className="text-xs tabular-nums text-[#8b949e]">
        Both charts share a ceiling of {formatWatts(peak)}, so the two
        directions are comparable.
      </div>
    </div>
  );
}

/// Battery, thermal pressure and uptime — the machine's condition.
///
/// # Why these three share a page
///
/// None of them has enough to carry one alone: battery is two numbers
/// with no history behind them, thermal is a single coarse label, and
/// uptime is one figure. The brief allowed omitting them, and the
/// argument against that is that the sidebar is a list of what this
/// view can tell you about the machine -- and a class silently missing
/// from it is a question the reader assumes the app cannot answer.
///
/// What they share is real rather than convenient: all three describe
/// the machine's CONDITION rather than its work. Battery and thermal
/// are directly coupled -- a hot machine on battery is throttled and
/// draining -- and uptime is the window every other reading on this
/// page is measured within.
///
/// This page adds what the overview panel cannot: the coupling between
/// the three, said once, where all three are on screen.
function PowerDetail({
  sample: s,
  samples,
  sampledAt,
}: {
  sample: HealthSample;
  /// The 24-hour series, for the power-flow chart (#773). This page
  /// took no history at all before that issue -- battery was two live
  /// numbers -- and a rate is the one reading here that is far more
  /// useful as a shape than as an instant.
  samples: HealthSample[];
  sampledAt: number;
}) {
  const watts = useMemo(() => powerSeries(samples), [samples]);
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Battery" subtitle="Charge and power source">
        <div className="flex flex-wrap gap-6">
          <Stat
            label="Charge"
            value={s.battery === null ? null : `${s.battery.percent.toFixed(0)}%`}
            hint={
              s.battery === null
                ? IS_MOBILE_BUILD
                  ? "That desktop has no battery"
                  : "No battery on this machine"
                : undefined
            }
          />
          <Stat
            label="Power"
            value={s.battery === null ? null : s.battery.on_ac ? "On AC" : "On battery"}
          />
        </div>
        {s.battery !== null ? (
          <div className="mt-3">
            <Bar percent={s.battery.percent} label="Battery charge" />
          </div>
        ) : (
          // A desktop with no battery is not a flat battery, and this
          // is the page where that distinction has room to be stated
          // rather than left to a four-word hint.
          <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
            {IS_MOBILE_BUILD
              ? "The desktop this is describing has no battery — a mains-powered machine, not one that is flat."
              : "This machine has no battery — it is mains-powered, which is not the same as a battery at zero."}
          </p>
        )}
      </Panel>

      {/* The RATE (#773), between charge and capacity because that is
          the order of the questions: how full, how fast is that
          changing, and how much can it hold at all. Its own panel for
          the same reason capacity has one -- watts and percentages are
          different quantities, and a wattage beside a charge bar is
          read as part of it. */}
      {s.battery !== null ? (
        <Panel
          title="Charge and drain rate"
          subtitle="How fast power is moving in or out of the cell, right now"
        >
          {s.battery.power ? (
            <PowerFlowCard power={s.battery.power} onAc={s.battery.on_ac} />
          ) : (
            <PowerNotMeasured />
          )}
          {s.battery.power ? (
            <div className="mt-4">
              <div className="text-xs text-[#8b949e]">
                Power flow over the last 24 hours
              </div>
              <PowerHistory series={watts} now={sampledAt} />
              <GapNote />
            </div>
          ) : null}
        </Panel>
      ) : null}

      {/* CAPACITY, in its own panel, never in the one above.
          "Battery health" normally means capacity relative to design --
          a different number from charge, moving over years rather than
          minutes. A three-year-old laptop at 100% charge and 84%
          capacity is entirely normal, and putting the two figures side
          by side is how a reader concludes their fully-charged battery
          is somehow at 84%. Separate panel, separate heading, and the
          word "charge" appears in neither. */}
      {s.battery !== null && s.battery.capacity_percent !== null ? (
        <CapacityPanel
          percent={s.battery.capacity_percent}
          cycles={s.battery.cycle_count}
        />
      ) : s.battery !== null ? (
        // A battery whose capacity the platform will not report. Said
        // rather than silently omitted: a panel that vanishes on Linux
        // looks like a bug, and "not measured" is the honest answer.
        <Panel title="Battery capacity" subtitle="How much the cell can still hold">
          <p className="text-sm">
            <NotMeasured />
          </p>
          <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
            Capacity relative to design — a different figure from the charge
            above. Headstate reads it on macOS only; no other platform
            publishes it without elevated privileges.
          </p>
        </Panel>
      ) : null}

      <Panel title="Thermal pressure" subtitle="The system's own verdict, not a temperature">
        <div
          className="text-sm font-medium capitalize"
          style={{ color: s.thermal ? thermalColor(s.thermal) : undefined }}
        >
          {s.thermal ?? <NotMeasured />}
        </div>
        <p className="mt-2 text-xs leading-relaxed text-[#8b949e]">
          {s.thermal && THERMAL_MEANING[s.thermal] ? `${THERMAL_MEANING[s.thermal]} ` : ""}
          This is the system&apos;s own thermal <em>pressure</em> rating, not a
          temperature. Headstate cannot read degrees: that needs elevated
          privileges, so the app reports the label the operating system already
          publishes instead.
        </p>
        {/* The coupling, which is the reason these three share a page.
            Only stated when both facts are actually in hand -- an
            invented warning about a battery that does not exist would
            be the page's own rule broken. */}
        {s.thermal && s.thermal !== "nominal" && s.battery !== null && !s.battery.on_ac ? (
          <p className="mt-2 text-xs leading-relaxed text-[#d29922]">
            Warm and on battery: a machine in this state is usually being
            slowed on purpose to save both heat and charge, so the CPU page
            may read lower than the work actually wants.
          </p>
        ) : null}
      </Panel>

      <Panel title="Uptime" subtitle="How long this machine has been running">
        <div className="flex flex-wrap gap-6">
          <Stat label="Uptime" value={formatUptime(s.uptime_secs)} />
          {/* Derived from uptime and subtracted from the SAMPLE's own
              timestamp, not the clock at render time -- a boot time
              that slides forward on every repaint is obviously wrong
              to anyone watching it. */}
          <Stat
            label="Booted"
            value={new Date(sampledAt - s.uptime_secs * 1000).toLocaleString()}
          />
        </div>
        {/* Why uptime is on this page at all: it bounds everything
            else. A load average over fifteen minutes means nothing on a
            machine that booted two minutes ago. */}
        <p className="mt-3 text-xs leading-relaxed text-[#8b949e]">
          Uptime is the window every other reading here sits inside: network
          totals count from this moment, and a fifteen-minute load average
          means little on a machine that has been up for less than that.
        </p>
      </Panel>
    </div>
  );
}
