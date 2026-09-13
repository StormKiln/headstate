import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { Card } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import type { ClaudeDayCount } from "@/types/pr";

/// The one series. A record rather than a literal because `ChartContainer`
/// takes a config map, and the same map supplies the tooltip's label.
const config = {
  started: { label: "Sessions started", color: "var(--chart-opened)" },
};

/// Claude Code sessions started per day (#921).
///
/// # Why this chart, and why only this one
///
/// #921 offers five chart candidates and asks for a cut. This is the one
/// that survives the test the requester set -- *does it help me see what
/// is going on, or resurrect something?* -- and it survives on the
/// second-order reading rather than the first: a bar per day does not by
/// itself resurrect anything, but the SHAPE answers "am I abandoning
/// work?". Thirty-four sessions started yesterday against 248 resumable
/// all-time is a legible fact about how this machine is used, and it is
/// the fact behind the 83% of sessions whose directory is gone.
///
/// The four that were cut, and why, are argued in `ClaudeOverviewPage`.
///
/// # Bars, not an area
///
/// `ActivityChart` uses stacked gradient areas because it draws two
/// overlapping series and the GAP between them is the signal. This is one
/// series of counts of discrete events, where each column is a day's
/// tally and nothing is interpolated between them. An area chart would
/// draw a slope between Tuesday's 9 and Wednesday's 34 that claims a
/// continuum the data does not have -- sessions start at instants, not at
/// rates. Same library, same wrapper, different mark for a different
/// measure, which is the `Leaderboard` precedent: it declines recharts
/// entirely rather than reuse a mark that does not fit its measure.
///
/// # Every day is drawn, including the empty ones
///
/// `points` arrives from the backend with all 30 days present and
/// zero-filled, and this component must not filter them out. A series of
/// only the days that had activity draws a dense chart with no gaps, so a
/// week off reads as a week of steady work at whatever the neighbouring
/// values were. The axis would lie about the shape, which is the only
/// thing the chart is for.
///
/// A zero here is a MEASURED zero. The absent case never reaches this
/// component: a rejected query is an error the page renders instead, for
/// the reason `ClaudeOverviewPage` states at length -- a flat line does
/// not look absent.
///
/// # Render cost at 30 points
///
/// 30 bars, one `<rect>` each. Deliberately not 1,461 points: the
/// aggregation into days happens in SQL-adjacent Rust, so what crosses
/// the bridge and what recharts lays out is 30 objects rather than the
/// corpus. That is the reason this chart is affordable on a 10-second
/// poll at all, and the reason there is no windowing or virtualisation
/// here to get wrong.
export function SessionsChart({
  points,
  days,
}: {
  points: ClaudeDayCount[];
  /// How many days the window covers, for the subtitle. Passed in rather
  /// than taken as `points.length` so the sentence states the INTENDED
  /// window: a short array is a bug worth seeing as a mismatch between
  /// the words and the bars, not one the label papers over.
  days: number;
}) {
  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">Sessions started per day</div>
      {/* UTC is stated because the buckets are UTC days: a 9pm Pacific
          session lands in the next day's column. Disclosed rather than
          silently wrong, exactly as `ActivityChart` discloses the same
          boundary for GitHub's date qualifiers. */}
      <div className="text-xs text-[#8b949e]">
        the last {days} days, by the day each session began (UTC)
      </div>
      {points.length === 0 ? (
        // Reachable only if the backend returned an empty window, which
        // is a bug rather than a state -- `fill_window` always returns
        // ACTIVITY_DAYS buckets. Said plainly rather than drawn as an
        // empty chart, because an axis with no bars reads as "no activity"
        // and this is "no data arrived".
        <div className="py-16 text-center text-sm text-[#8b949e]">
          No daily buckets were returned, so this period cannot be drawn.
        </div>
      ) : (
        <ChartContainer config={config} className="mt-4 h-48 w-full">
          <BarChart data={points} margin={{ left: 0, right: 0, top: 4, bottom: 0 }}>
            <CartesianGrid vertical={false} stroke="#30363d" />
            <XAxis
              dataKey="day"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              minTickGap={24}
              tick={{ fill: "#8b949e", fontSize: 11 }}
              // Full ISO dates collide at 30 columns; month-day fits.
              tickFormatter={(v: string) => v.slice(5)}
            />
            <YAxis
              tickLine={false}
              axisLine={false}
              width={32}
              tick={{ fill: "#8b949e", fontSize: 11 }}
              // Sessions are whole things. A "2.5" gridline on a count
              // invites the reader to interpolate a quantity that cannot
              // exist.
              allowDecimals={false}
            />
            <ChartTooltip content={<ChartTooltipContent indicator="dot" />} />
            <Bar dataKey="started" fill={config.started.color} radius={2} />
          </BarChart>
        </ChartContainer>
      )}
    </Card>
  );
}
