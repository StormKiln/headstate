import { useJiraIssue } from "@/api/jira";
import { driftFor } from "@/lib/jira/drift";
import type { FoundKey } from "@/lib/jira/keys";
import { rowId, type RowRef } from "@/lib/jira/links";
import { TriangleAlert } from "lucide-react";
import type { JiraStatusCategory } from "@/types/jira";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";
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
/// Renders nothing unless Jira is on, signed in and its links are read --
/// see `JiraState`. `row` names the pull request, worktree or branch; the
/// desktop has already found its keys. Pass `openPr` for an OPEN pull
/// request, and a ticket that disagrees with it (#2042) is marked.
export function TicketChips({ row, openPr }: { row: RowRef; openPr?: { draft: boolean } }) {
  const jira = useJira();
  if (!jira) return null;
  const found = jira.links.byRow.get(rowId(row)) ?? [];
  if (found.length === 0) return null;
  return (
    <>
      {found.map((f) => (
        <TicketChip key={f.key} found={f} openPr={openPr} />
      ))}
    </>
  );
}

function TicketChip({ found, openPr }: { found: FoundKey; openPr?: { draft: boolean } }) {
  const q = useJiraIssue(found.key);
  const where = found.source === "title" ? "the title" : "the branch name";

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
  const drift = openPr ? driftFor(issue, { draft: openPr.draft, source: found.source }) : null;
  const label = (
    <>
      {drift ? <TriangleAlert className="h-3 w-3" aria-label="Needs attention" /> : null}
      {issue.key} · {issue.status.name || "no status"}
    </>
  );
  const tip = `${drift ? `${drift.message}\n` : ""}${issue.summary}${issue.assignee ? ` — ${issue.assignee}` : ""} (from ${where})`;
  const cls = `${CHIP} ${drift ? "border-[#d29922] text-[#d29922]" : CATEGORY_CLASS[issue.status.category]}`;
  // Opens the issue in My Jira issues, which links on to Jira itself. A
  // button inside a clickable row, so the click stops here.
  return (
    <button
      type="button"
      title={tip}
      className={`${cls} hover:bg-[#21262d]`}
      // Enter on the chip must not also reach the row's own key handler,
      // as the row's checkbox does.
      onKeyDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        useJiraView.getState().select(issue.key);
        useFilters.getState().setView("jira");
      }}
    >
      {label}
    </button>
  );
}

/// The drift notices for an open pull request, spelled out (#2042). For the
/// detail view, where there is room for a sentence; rows carry the marked
/// chip instead.
export function TicketDriftNotices({ prUrl, draft }: { prUrl: string; draft: boolean }) {
  const jira = useJira();
  if (!jira) return null;
  const found = jira.links.byRow.get(rowId({ prUrl })) ?? [];
  if (found.length === 0) return null;
  return (
    <>
      {found.map((f) => (
        <DriftNotice key={f.key} found={f} draft={draft} />
      ))}
    </>
  );
}

function DriftNotice({ found, draft }: { found: FoundKey; draft: boolean }) {
  const q = useJiraIssue(found.key);
  const s = q.data;
  const drift = s?.state === "found" ? driftFor(s.issue, { draft, source: found.source }) : null;
  if (!drift) return null;
  return (
    <p role="status" className="mt-2 flex items-center gap-1.5 text-xs text-[#d29922]">
      <TriangleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      {drift.message}
    </p>
  );
}
