import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { ChartContainer, ChartTooltip, ChartTooltipContent } from "./ui/chart";
import type { GitLabStatsReport } from "../api/gitlabStats";

export function GitLabActivityChart({ report }: { report: GitLabStatsReport }) {
  const created = new Map(report.series.map(point => [point.day, point.created]));
  const merged = new Map(report.merged_window?.series.map(point => [point.day, point.merged]) ?? []);
  const points = [];
  const start = Date.parse(report.start.slice(0, 10));
  const end = Date.parse(report.end.slice(0, 10));
  for (let at = start; at <= end && points.length < 90; at += 86_400_000) {
    const day = new Date(at).toISOString().slice(0, 10);
    points.push({ day, created: created.get(day) ?? (report.coverage.complete ? 0 : null), merged: merged.get(day) ?? (report.merged_window?.coverage.complete ? 0 : null) });
  }
  return <section aria-label="GitLab daily activity">
    <h2 className="font-semibold">Daily MR activity (UTC)</h2>
    <p className="text-sm text-muted-foreground">Created: solid blue. Merged: dashed green. Partial reads show lower bounds; missing measurements remain gaps. Today is still in progress.</p>
    <ChartContainer className="h-56 w-full" config={{ created: { label: "Created", color: "#58a6ff" }, merged: { label: "Merged", color: "#3fb950" } }}>
      <LineChart data={points} accessibilityLayer>
        <CartesianGrid vertical={false} />
        <XAxis dataKey="day" tickFormatter={day => String(day).slice(5)} />
        <YAxis allowDecimals={false} />
        <ChartTooltip content={<ChartTooltipContent />} />
        <Line type="linear" dataKey="created" stroke="var(--color-created)" dot={false} isAnimationActive={false} connectNulls={false} />
        <Line type="linear" dataKey="merged" stroke="var(--color-merged)" strokeDasharray="4 3" dot={false} isAnimationActive={false} connectNulls={false} />
      </LineChart>
    </ChartContainer>
  </section>;
}
