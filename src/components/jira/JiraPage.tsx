import { ArrowLeft, TriangleAlert } from "lucide-react";
import { driftFor } from "@/lib/jira/drift";
import { useIsMobile } from "@/lib/useIsMobile";
import { CATEGORY_TEXT, describe } from "@/lib/jira/describe";
import { epicOf, rollUp, type Links, type Lookup } from "@/lib/jira/tree";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";
import { useSourceSelection } from "@/store/sourceSelection";
import type { PrIdentity } from "@/types/identity";
import { ExternalLink } from "../ExternalLink";
import { JiraBrowser } from "./JiraBrowser";
import { useJiraBoard, type BoardPr } from "./useJiraBoard";

/// Open a pull request in Headstate's own detail view. The source
/// selection is widened only when it would hide this pull request.
function openInHeadstate(id: PrIdentity) {
  const provider = id.source?.provider ?? "github";
  const sel = useSourceSelection.getState();
  if (sel.selection !== "both" && sel.selection !== provider) sel.setSelection(provider);
  useFilters.getState().setView("my-prs");
  useFilters.getState().selectPr(id);
}

/// My Jira issues' main panel (#2038): the selected issue and every pull
/// request, worktree and branch of yours that names it.
export function JiraPage({ prs }: { prs: readonly BoardPr[] }) {
  const isMobile = useIsMobile();
  const { selected, select } = useJiraView();
  const board = useJiraBoard(prs);

  if (selected === null) {
    // On a phone the sidebar is a sheet, so the page is the list.
    return isMobile ? (
      <JiraBrowser prs={prs} />
    ) : (
      <p className="text-sm text-[#8b949e]">Select an issue to see its pull requests and worktrees.</p>
    );
  }

  const known = [...new Set([...board.linked, ...board.assignedKeys])];
  return (
    <div className="flex max-w-3xl flex-col gap-4">
      {isMobile ? (
        <button type="button" onClick={() => select(null)} className="flex items-center gap-1 self-start text-xs text-[#58a6ff]">
          <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All issues
        </button>
      ) : null}
      <IssueHeader k={selected} lookup={board.lookup} site={board.jira?.site ?? null} />
      <EpicRollUp k={selected} known={known} lookup={board.lookup} />
      <LinkedWork k={selected} links={board.links.get(selected)} lookup={board.lookup} />
    </div>
  );
}

function IssueHeader({ k, lookup, site }: { k: string; lookup: Lookup; site: string | null }) {
  const { select } = useJiraView();
  const s = lookup(k);
  const d = describe(k, lookup);
  const issue = s !== undefined && s !== "failed" && s.state === "found" ? s.issue : null;
  const href = issue?.url ?? (site ? `https://${site}/browse/${k}` : null);
  return (
    <div className="flex flex-col gap-1">
      {issue?.parent_key ? (
        <button
          type="button"
          onClick={() => select(issue.parent_key)}
          className="self-start text-xs text-[#8b949e] hover:text-[#58a6ff]"
        >
          <span className="font-mono">{issue.parent_key}</span> {describe(issue.parent_key, lookup).title}
        </button>
      ) : null}
      <h2 className="text-lg font-semibold leading-snug text-[#e6edf3]">
        <span className="font-mono text-[#8b949e]">{k}</span> {d.title}
      </h2>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-[#8b949e]">
        {issue ? <span>{issue.issue_type.name || "Issue"}</span> : null}
        {d.status ? <span className={CATEGORY_TEXT[d.category]}>{d.status}</span> : null}
        {issue ? <span>{issue.assignee ?? "Unassigned"}</span> : null}
        {href ? (
          <ExternalLink href={href} className="text-[#58a6ff] hover:underline">
            Open in Jira
          </ExternalLink>
        ) : null}
      </div>
    </div>
  );
}

/// For an epic: its children among the issues this page knows about.
/// Qualified on purpose -- Headstate sees the issues linked to your work or
/// assigned to you, not every child the epic has.
function EpicRollUp({ k, known, lookup }: { k: string; known: string[]; lookup: Lookup }) {
  const s = lookup(k);
  const level = s !== undefined && s !== "failed" && s.state === "found" ? s.issue.issue_type.hierarchy_level : null;
  if (level === null || level < 1) return null;
  const children = known.filter((c) => c !== k && epicOf(c, lookup) === k);
  if (children.length === 0) return null;
  const c = rollUp(children, lookup);
  return (
    <p className="text-xs text-[#8b949e]">
      Of the {children.length} issue{children.length === 1 ? "" : "s"} under this epic that are linked to your work or
      assigned to you: {c.done} done, {c.in_progress} in progress, {c.new} to do
      {c.unknown ? `, ${c.unknown} not known` : ""}.
    </p>
  );
}

function LinkedWork({ k, links, lookup }: { k: string; links: Links | undefined; lookup: Lookup }) {
  const s = lookup(k);
  const issue = s !== undefined && s !== "failed" && s.state === "found" ? s.issue : null;
  const prs = links?.prs ?? [];
  const worktrees = links?.worktrees ?? [];
  if (prs.length === 0 && worktrees.length === 0) {
    return <p className="text-sm text-[#8b949e]">None of your open pull requests or worktrees names {k}.</p>;
  }
  return (
    <div className="flex flex-col gap-3">
      {prs.length > 0 ? (
        <section aria-label="Pull requests" className="flex flex-col gap-1">
          <h3 className="text-sm font-medium text-[#e6edf3]">Pull requests</h3>
          <ul className="flex flex-col gap-1">
            {prs.map((pr) => {
              const gitlab = pr.identity.source?.provider === "gitlab";
              const drift = issue ? driftFor(issue, { draft: pr.is_draft, source: pr.source }) : null;
              return (
                <li
                  key={`${pr.identity.repo}#${pr.identity.number}`}
                  className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded border border-[#30363d] px-2 py-1.5 text-sm"
                >
                  <span className="min-w-0 flex-1 truncate text-[#e6edf3]">{pr.title}</span>
                  <span className="text-xs text-[#8b949e]">
                    {pr.identity.repo}
                    {gitlab ? "!" : "#"}
                    {pr.identity.number}
                    {pr.is_draft ? " · draft" : ""} · from the {pr.source === "title" ? "title" : "branch"}
                  </span>
                  <button
                    type="button"
                    onClick={() => openInHeadstate(pr.identity)}
                    className="text-xs text-[#58a6ff] hover:underline"
                  >
                    Open in Headstate
                  </button>
                  <ExternalLink href={pr.url} className="text-xs text-[#58a6ff] hover:underline">
                    {gitlab ? "Open on GitLab" : "Open on GitHub"}
                  </ExternalLink>
                  {drift ? (
                    <p role="status" className="flex basis-full items-center gap-1.5 text-xs text-[#d29922]">
                      <TriangleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      {drift.message}
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}
      {worktrees.length > 0 ? (
        <section aria-label="Worktrees" className="flex flex-col gap-1">
          <h3 className="text-sm font-medium text-[#e6edf3]">Worktrees</h3>
          <ul className="flex flex-col gap-1">
            {worktrees.map((w) => (
              <li key={w.path} className="flex flex-wrap gap-x-2 text-xs text-[#8b949e]">
                <span className="text-[#e6edf3]">{w.repo}</span>
                <span className="font-mono">{w.branch}</span>
                <span className="truncate font-mono">{w.path}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
