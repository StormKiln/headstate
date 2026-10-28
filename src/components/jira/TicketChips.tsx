import { useJiraIssue } from "@/api/jira";
import { keysForBranch, keysForPr, type FoundKey } from "@/lib/jira/keys";
import type { JiraStatusCategory } from "@/types/jira";
import { ExternalLink } from "../ExternalLink";
import { useJira } from "./jiraState";

/// Status colour by Jira's category, never by status name: "In Testing"
/// and "Code Review" are both in progress, whatever a workflow calls them.
const CATEGORY_CLASS: Record<JiraStatusCategory, string> = {
  new: "border-[#8b949e] text-[#8b949e]",
  in_progress: "border-[#58a6ff] text-[#58a6ff]",
  done: "border-[#3fb950] text-[#3fb950]",
  unknown: "border-[#8b949e] text-[#8b949e]",
};

const CHIP = "inline-flex shrink-0 items-center gap-1 rounded-full border px-2 py-0.5 font-mono text-xs";

/// The Jira tickets a pull request, worktree or branch names (#2039).
///
/// Renders nothing unless Jira is on, signed in and its projects are known
/// -- see `JiraState`. Pass `title` for a pull request; a worktree or local
/// branch has only `branch`.
export function TicketChips({ title, branch }: { title?: string; branch?: string | null }) {
  const jira = useJira();
  if (!jira) return null;
  const found =
    title === undefined ? keysForBranch(branch, jira.projects) : keysForPr({ title, head_ref: branch }, jira.projects);
  if (found.length === 0) return null;
  return (
    <>
      {found.map((f) => (
        <TicketChip key={f.key} found={f} site={jira.site} />
      ))}
    </>
  );
}

function TicketChip({ found, site }: { found: FoundKey; site: string | null }) {
  const q = useJiraIssue(found.key);
  const where = found.source === "title" ? "the title" : "the branch name";
  const fallback = site ? `https://${site}/browse/${found.key}` : null;

  if (q.isPending) {
    return (
      <span className={`${CHIP} border-[#30363d] text-[#8b949e]`} aria-busy="true" title={`${found.key}, from ${where}`}>
        {found.key}
        <span className="h-2 w-10 animate-pulse rounded bg-[#21262d]" />
      </span>
    );
  }
  if (q.isError) {
    return (
      <span
        className={`${CHIP} border-[#f85149] text-[#f85149]`}
        title={`Could not ask Jira about ${found.key}: ${String(q.error)}`}
      >
        {found.key} · not loaded
      </span>
    );
  }
  const s = q.data;
  if (s.state === "unasked") {
    return (
      <span className={`${CHIP} border-[#30363d] text-[#8b949e]`} title={`${found.key} has not been checked yet`}>
        {found.key} · not checked yet
      </span>
    );
  }
  if (s.state === "missing") {
    // Shown, not hidden: the key in the branch is a fact even when Jira
    // will not say what it refers to.
    return (
      <span
        className={`${CHIP} border-dashed border-[#30363d] text-[#8b949e]`}
        title={`Jira returned nothing for ${found.key}: it does not exist, or you cannot see it`}
      >
        {found.key} · ?
      </span>
    );
  }
  const { issue } = s;
  const href = issue.url ?? fallback;
  const label = (
    <>
      {issue.key} · {issue.status.name || "no status"}
    </>
  );
  const tip = `${issue.summary}${issue.assignee ? ` — ${issue.assignee}` : ""} (from ${where})`;
  const cls = `${CHIP} ${CATEGORY_CLASS[issue.status.category]}`;
  return href ? (
    <ExternalLink href={href} title={tip} className={`${cls} hover:bg-[#21262d]`}>
      {label}
    </ExternalLink>
  ) : (
    <span title={tip} className={cls}>
      {label}
    </span>
  );
}
