import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { Card } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import type { PluginDayCount } from "@/types/pr";

/// The one series, in `SessionsChart`'s shape for the same reason.
const config = {
  calls: { label: "Plugin calls", color: "var(--chart-opened)" },
};

/// Plugin calls per day (#1075).
///
/// # Why this is `SessionsChart` again and not a new idiom
///
/// #1075 asks for "calls over time", and the Claude overview already
/// draws counts-of-discrete-events-per-UTC-day as bars. Same measure,
/// same window, same wrapper -- so this is that component's shape with a
/// different series rather than a second charting style on an adjacent
/// page. The `ActivityChart` reasoning applies unchanged: bars because
/// each column is a day's tally and nothing is interpolated, where an
/// area would draw a slope between Tuesday and Wednesday claiming a
/// continuum that calls-at-instants do not have.
///
/// Not merged with `SessionsChart` into one generic chart, though. That
/// component's subtitle, tooltip label and `started` key all name
/// sessions, and the merge would be a `dataKey` parameter plus three
/// label parameters -- every caller passing its own copy of what the
/// chart says, which is how two charts drift while sharing a file.
///
/// # What a zero column means here
///
/// A MEASURED zero: the scan covered that day's transcripts and found no
/// plugin call in them. That is a real and common answer -- 7 of 22
/// installed plugins show any usage at all -- and it is different from
/// the absence this page is careful about elsewhere. An unmeasured
/// plugin never reaches this chart; the page renders it as a row saying
/// so, never as a flat line.
export function PluginCallsChart({
  points,
  days,
}: {
  points: PluginDayCount[];
  /// How many days the window covers, for the subtitle. Passed in rather
  /// than taken as `points.length` for `SessionsChart`'s reason: a short
  /// array should read as a mismatch between the words and the bars, not
  /// be papered over by a label that adapts to it.
  days: number;
}) {
  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">Plugin calls per day</div>
      {/* UTC, stated, because the buckets are UTC days and a 9pm Pacific
          call lands in the next day's column. The same disclosure
          `SessionsChart` makes. */}
      <div className="text-xs text-[#8b949e]">
        the last {days} days, counting only calls that were actually made (UTC)
      </div>
      {points.length === 0 ? (
        // Reachable only if the backend returned an empty window, which
        // is a bug rather than a state -- `fill_window` always returns
        // ACTIVITY_DAYS buckets. Said plainly, because an axis with no
        // bars reads as "no calls" and this is "no data arrived".
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
              tickFormatter={(v: string) => v.slice(5)}
            />
            <YAxis
              tickLine={false}
              axisLine={false}
              width={32}
              tick={{ fill: "#8b949e", fontSize: 11 }}
              // Calls are whole things; a "2.5" gridline invites the
              // reader to interpolate a quantity that cannot exist.
              allowDecimals={false}
            />
            <ChartTooltip content={<ChartTooltipContent indicator="dot" />} />
            <Bar dataKey="calls" fill={config.calls.color} radius={2} />
          </BarChart>
        </ChartContainer>
      )}
    </Card>
  );
}
