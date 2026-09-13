import { ArrowDown, ArrowUp, Minus } from "lucide-react";
import { HelpButton } from "../HelpButton";
import { Card } from "@/components/ui/card";
import { formatPct, pctChange } from "@/lib/stats";
import type { Periods } from "@/types/pr";

/// A single headline number with its change against the prior period.
///
/// `delta` is null when both periods were empty and Infinity when only the
/// prior one was, so only a finite value earns an arrow and a colour --
/// otherwise "no activity" would render as a green +0%.
function DeltaCard({
  label,
  value,
  delta,
  window: win,
  polarity = "more-is-better",
}: {
  label: string;
  value: string;
  delta: number | null;
  window: string;
  /// Whether a rise is good news.
  ///
  /// Applying one rule to every card painted "Opened this week +150%"
  /// green beside "Merged this week -67%" red -- the UI cheering a growing
  /// backlog. Intake is `neutral`: the chart's own comment names the GAP
  /// between opened and merged as the signal, not either count alone.
  polarity?: "more-is-better" | "neutral";
}) {
  // THREE cases, not two, because `formatPct` already renders three and
  // this row has to agree with the words beside it (#950):
  //
  //   delta === null   "--"    nothing to compare. No direction exists.
  //   !isFinite        "new"   growth from zero. Unambiguously a RISE.
  //   finite           "+N%"   a measured change, signed.
  //
  // These were collapsed into one `!finite` branch, which put a `Minus`
  // glyph in front of the word "new". A brand-new user has zero of
  // everything in the previous period, so `pctChange` returns `Infinity`
  // (`stats.ts:8`: `previous === 0` and `current !== 0`) for every card --
  // making "a minus sign beside the word new" the FIRST thing the app
  // shows about their work, on every card, on first launch.
  //
  // `Minus` stays for `null` alone, where it is right: `previous === 0`
  // AND `current === 0` is a genuine absence of a reading, and no arrow
  // would be honest about it.
  const isNull = delta === null;
  const isNew = !isNull && !Number.isFinite(delta as number);
  const finite = !isNull && !isNew;
  const up = isNew || (finite && (delta as number) >= 0);
  const Icon = isNull ? Minus : up ? ArrowUp : ArrowDown;
  // Tone follows the DIRECTION, so "new" is coloured like the rise it is
  // -- subject to polarity, which is why a growing backlog is not
  // cheered. `neutral` and the no-reading case both stay grey: one
  // because the direction carries no verdict, the other because there is
  // no direction.
  const tone =
    isNull || polarity === "neutral"
      ? "text-[#8b949e]"
      : up
        ? "text-[#3fb950]"
        : "text-[#f85149]";
  return (
    <Card className="px-4">
      <div className="text-xs text-[#8b949e]">{label}</div>
      <div className="mt-1 text-2xl font-semibold tabular-nums">{value}</div>
      <div
        className={`mt-1 flex items-center gap-1 text-xs ${tone}`}
      >
        <Icon className="h-3 w-3 shrink-0" aria-hidden="true" />
        <span>{formatPct(delta)}</span>
        <span className="text-[#8b949e]">{win}</span>
      </div>
    </Card>
  );
}

/// The four headline figures.
///
/// Every window is stated in words ("vs previous 7 days") rather than left
/// implicit: the periods deliberately exclude today, so a bare percentage
/// would be quietly comparing something different from what the reader
/// assumes.
export function DeltaCards({ periods }: { periods: Periods }) {
  const netFlow = periods.opened_week_current - periods.week_current;
  return (
    <>
      {/* Why "+150% opened" is not green: intake and throughput are
          different axes, and only one of them is scored. */}
      <div className="flex items-center gap-1 text-xs text-[#8b949e]">
        <span>Change against the previous period</span>
        <HelpButton topic="stats-deltas" />
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
      <DeltaCard
        label="Merged this week"
        value={String(periods.week_current)}
        delta={pctChange(periods.week_current, periods.week_previous)}
        window="vs previous 7 days"
      />
      <DeltaCard
        label="Opened this week"
        value={String(periods.opened_week_current)}
        delta={pctChange(periods.opened_week_current, periods.opened_week_previous)}
        window="vs previous 7 days"
        polarity="neutral"
      />
      <DeltaCard
        label="Merged this month"
        value={String(periods.month_current)}
        delta={pctChange(periods.month_current, periods.month_previous)}
        window="vs previous 30 days"
      />
      {/* Net flow replaces a second copy of "Median cycle time", which
          already appears (with a p90) in the insight row below and could
          only render a grey minus here, having no prior period to compare
          against. Backlog delta is the number neither existing card
          answered: is my WIP growing? Computed from data already fetched. */}
      <DeltaCard
        label="Net backlog"
        value={netFlow > 0 ? `+${netFlow}` : String(netFlow)}
        delta={null}
        window="opened minus merged, this week"
        polarity="neutral"
      />
    </div>
    </>
  );
}
