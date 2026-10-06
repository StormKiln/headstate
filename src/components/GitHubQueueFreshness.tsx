import { githubQueueSummary, type GitHubQueueStatus } from "../lib/githubQueueSummary";
import type { SourceSelection } from "../store/sourceSelection";
import { relativeTime } from "../lib/time";

/** The production inventory footer, independent of updater/settings controls. */
export function GitHubQueueFreshness({ github, selection = "github" }: { github: GitHubQueueStatus; selection?: SourceSelection }) {
  const summary = githubQueueSummary(github);
  return <>
    <span className="flex items-center gap-1.5">
      <span className={`h-1.5 w-1.5 rounded-full ${summary.warning ? "bg-[#d29922]" : "bg-[#3fb950]"}`} aria-hidden="true" />
      <span title={summary.explanation} aria-label={summary.explanation}>
        {selection === "both" ? "GitHub · " : ""}{summary.text}
      </span>
    </span>
    {summary.updatedAt !== undefined && summary.updatedAt > 0 ? <span>
      {selection === "both" ? "GitHub updated" : "Updated"} {relativeTime(new Date(summary.updatedAt).toISOString())}
    </span> : null}
  </>;
}
