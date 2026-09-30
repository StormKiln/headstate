import { StatsPage } from "./StatsPage";
import { GitLabStatsPage } from "./GitLabStatsPage";

/** Provider pages never share queries, scope selection, or totals. */
export function ProviderStatsPage({ selection, provider, onProviderChange, gitlabHost }: {
  selection: "github" | "gitlab" | "both";
  provider: "github" | "gitlab";
  onProviderChange: (provider: "github" | "gitlab") => void;
  gitlabHost?: string;
}) {
  return <div className="flex min-h-0 flex-1 flex-col">
    {selection === "both" && <div className="flex gap-2 border-b p-2" aria-label="Statistics provider">{(["github", "gitlab"] as const).map(p => <button type="button" key={p} aria-pressed={provider === p} onClick={() => onProviderChange(p)} className="rounded border px-3 py-1">{p === "github" ? "GitHub PR Stats" : "GitLab MR Stats"}</button>)}</div>}
    {provider === "github" ? <StatsPage /> : gitlabHost ? <GitLabStatsPage host={gitlabHost} /> : <p role="status">GitLab host unavailable. Check Settings.</p>}
  </div>;
}
