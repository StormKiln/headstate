import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { HelpButton } from "../HelpButton";
import { Card } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import type { HistoryPoint } from "@/types/pr";

/// The windows a stats page offers, in days.
///
/// Exported and declared ONCE, here beside the buttons that render it, so
/// no second renderer of the same list can drift into offering a different
/// window -- the failure `VIEWS`, `HEALTH_PAGES` and `CLAUDE_PAGES` exist
/// to prevent. `StatsPage` carried a hand-maintained copy of these three
/// numbers and rendered a second, markup-identical group from it a few
/// hundred pixels above this one, which is the disagreement the scope
/// page's own comment warned about (#980).
///
/// 30 is the default because it is the shortest window in which a monthly
/// cadence of work is visible at all, and it is what the unscoped page
/// defaulted to. The default lives with `days` in `StatsPage`, which owns
/// that state because `ScopeCounts` and the series query read it too.
export const RANGES = [7, 14, 30];

const config = {
  merged: { label: "Merged", color: "var(--chart-merged)" },
  opened: { label: "Opened", color: "var(--chart-opened)" },
};

/// The two series, in draw order, each with the stroke dash that tells it
/// apart without relying on its colour.
///
/// #3fb950 and #58a6ff measure 1.01:1 against EACH OTHER, so the two
/// lines are effectively the same shade wherever hue is not perceived --
/// the one pairing in the app where colour carried the whole distinction.
/// `undefined` leaves the line solid rather than emitting
/// `strokeDasharray="none"`, which recharts would animate against.
const SERIES = [
  { key: "opened", dash: undefined },
  { key: "merged", dash: "4 3" },
] as const;

/// Daily opened and merged counts as overlaid gradient areas.
///
/// Deliberately NOT stacked. The two series measure overlapping
/// populations -- a PR opened and merged on the same day appears in both --
/// so a stacked total would be a number that means nothing. Overlaid, the
/// gap between the lines is readable as the backlog growing or draining,
/// which is the actual signal.
///
/// The trailing day is always partial, so the last point dips by
/// construction. That is left visible rather than trimmed: the shape is
/// still informative, and the delta cards (which exclude today) carry the
/// numbers people quote.
export function ActivityChart({
  points,
  days,
  onDaysChange,
}: {
  points: HistoryPoint[];
  days: number;
  onDaysChange: (d: number) => void;
}) {
  return (
    <Card className="px-4">
      <div className="flex items-center justify-between">
        <div>
          <div className="inline-flex items-center text-sm font-semibold">
            Pull request activity
            {/* Beside the chart the UTC boundary actually distorts,
                not on a page-level banner. */}
            <HelpButton topic="stats-timezone" />
          </div>
          {/* Buckets come from GitHub's bare date qualifiers, which it
              evaluates in UTC, so a 6pm Pacific merge lands in the next
              day's column. Aggregates are unaffected (the shift is
              uniform); only the per-day shape moves. Disclosed rather than
              silently wrong -- offset-qualifying the ranges is tracked
              separately. */}
          <div className="text-xs text-[#8b949e]">
            Opened and merged per day (UTC)
          </div>
          {/* A static key, because hue was the only thing telling the two
              series apart. The tooltip names them, but a tooltip is a
              HOVER affordance -- unavailable to a keyboard, to touch, and
              to anyone reading the chart rather than pointing at it.
              "Opened and merged per day" above names both series without
              saying which is which.

              Each swatch repeats its series' stroke dash, so the key is
              legible when the two blues and greens are not separable:
              opened is solid, merged is dashed, and that difference
              survives greyscale printing and every form of colour
              blindness. */}
          <div className="mt-1 flex items-center gap-3 text-xs text-[#8b949e]">
            {SERIES.map(({ key, dash }) => (
              <span key={key} className="inline-flex items-center gap-1.5">
                <svg width="16" height="2" aria-hidden="true" className="shrink-0">
                  <line
                    x1="0"
                    y1="1"
                    x2="16"
                    y2="1"
                    stroke={config[key].color}
                    strokeWidth="2"
                    strokeDasharray={dash}
                  />
                </svg>
                {config[key].label}
              </span>
            ))}
          </div>
        </div>
        <div className="flex gap-1">
          {RANGES.map((r) => (
            <button
              key={r}
              type="button"
              aria-pressed={days === r}
              onClick={() => onDaysChange(r)}
              className={`rounded px-2 py-1 text-xs ${
                days === r
                  ? "bg-[#1f6feb] text-white"
                  : "text-[#8b949e] hover:bg-[#161b22]"
              }`}
            >
              {r}d
            </button>
          ))}
        </div>
      </div>

      {points.length === 0 ? (
        <div className="py-16 text-center text-sm text-[#8b949e]">
          No activity in this period.
        </div>
      ) : (
        <ChartContainer config={config} className="mt-4 h-56 w-full">
          <AreaChart data={points} margin={{ left: 0, right: 0, top: 4, bottom: 0 }}>
            <defs>
              {(["merged", "opened"] as const).map((k) => (
                <linearGradient key={k} id={`fill-${k}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor={config[k].color} stopOpacity={0.7} />
                  <stop offset="95%" stopColor={config[k].color} stopOpacity={0.05} />
                </linearGradient>
              ))}
            </defs>
            <CartesianGrid vertical={false} stroke="#30363d" />
            <XAxis
              dataKey="date"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              minTickGap={24}
              tick={{ fill: "#8b949e", fontSize: 11 }}
              // Full ISO dates collide at 30 points; month-day is enough.
              tickFormatter={(v: string) => v.slice(5)}
            />
            <YAxis
              tickLine={false}
              axisLine={false}
              width={32}
              tick={{ fill: "#8b949e", fontSize: 11 }}
              allowDecimals={false}
            />
            <ChartTooltip content={<ChartTooltipContent indicator="dot" />} />
            {SERIES.map(({ key, dash }) => (
              <Area
                key={key}
                dataKey={key}
                type="monotone"
                stroke={config[key].color}
                fill={`url(#fill-${key})`}
                strokeWidth={2}
                strokeDasharray={dash}
              />
            ))}
          </AreaChart>
        </ChartContainer>
      )}
    </Card>
  );
}
