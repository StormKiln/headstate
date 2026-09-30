import { useGitLabViewer } from "../api/authAvailability";
import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { PullRequest } from "../types/pr";
import type { MergeRequest } from "../types/gitlab";
import type { PrIdentity } from "../types/identity";
import type { SourceCoverage } from "../api/tauri";
import type { SourceSelection } from "../store/sourceSelection";
import { useSourceSelection } from "../store/sourceSelection";
import { prIdentity, prKey } from "../lib/prIdentity";
import { current } from "../lib/ariaCurrent";
import { PrRow } from "./PrRow";
import { ViewSwitcher } from "./ViewSwitcher";
import { ExternalLink } from "./ExternalLink";
import { relativeSeconds } from "../lib/time";
import { useFilters, useActiveFilters, type View } from "../store/filters";
import { applyFilters, deriveStacked, type Filters } from "../lib/derive";
import { filterGitLab, gitlabParents } from "../lib/gitlabFilters";
import { FilterBar } from "./FilterBar";
import { BulkBar } from "./BulkBar";
import { useRowCursor } from "../lib/useRowCursor";
import { GitLabDetail } from "./GitLabDetail";
import { GitLabBulkActions } from "./GitLabBulkActions";
import { getGitLabDetail } from "../api/tauri";

type Row = { kind: "github"; value: PullRequest } | { kind: "gitlab"; value: MergeRequest };

export function sourceRepoKey(row: PrIdentity): string {
  const source = row.source ?? { provider: "github", host: "github.com" };
  return JSON.stringify([source.provider, source.host, row.repo]);
}

export function combinedRows(github: PullRequest[], gitlab: MergeRequest[], selection: SourceSelection, repoKey: string | null, query: string, filters: Filters = {}): Row[] {
  const all: Row[] = [
    ...(selection === "gitlab" ? [] : applyFilters(github, filters).map((value): Row => ({ kind: "github", value }))),
    ...(selection === "github" ? [] : filterGitLab(gitlab, filters).map((value): Row => ({ kind: "gitlab", value }))),
  ];
  const needle = query.toLowerCase().trim();
  return all.filter(({ value }) =>
    (repoKey === null || sourceRepoKey(value) === repoKey) &&
    (!needle || value.title.toLowerCase().includes(needle) || value.repo.toLowerCase().includes(needle) || String(value.number) === needle.replace(/^#|^!/, "")),
  ).sort((a, b) => {
    const field = filters.sort === "recently-updated" || filters.sort === "least-recently-updated" ? "updated_at" : "created_at";
    const direction = filters.sort === "oldest" || filters.sort === "least-recently-updated" ? 1 : -1;
    return direction * (Date.parse(a.value[field]) - Date.parse(b.value[field])) || prKey(a.value).localeCompare(prKey(b.value));
  });
}

function coverageMessage(provider: string, coverage: SourceCoverage | null, count: number): string | null {
  if (coverage === null || coverage === "unknown") return `${provider} could not confirm whether this list is complete.`;
  if (typeof coverage === "object") {
    const total = coverage.partial.total;
    return total !== null && total > count
      ? `${provider}: showing ${count} of ${total}; the rest did not load.`
      : `${provider}: this list is partial; more rows may be open.`;
  }
  return null;
}

export function SourceRepoSidebar({ github, gitlab, selection, viewCounts }: { github: PullRequest[]; gitlab: MergeRequest[]; selection: SourceSelection; viewCounts?: Partial<Record<View, number>> }) {
  const repoKey = useSourceSelection((s) => s.repoKey);
  const storeRepoKey = useSourceSelection((s) => s.setRepoKey);
  const setRepoKey = (key: string | null) => {
    useFilters.getState().setFilter("repo", undefined);
    storeRepoKey(key);
  };
  const rows: Row[] = [
    ...(selection === "gitlab" ? [] : github.map((value): Row => ({ kind: "github", value }))),
    ...gitlab.map((value): Row => ({ kind: "gitlab", value })),
  ];
  const repos = new Map<string, { label: string; count: number }>();
  for (const { value } of rows) {
    const key = sourceRepoKey(value);
    const label = `${value.source?.provider === "gitlab" ? "GitLab" : "GitHub"} · ${value.source?.host ?? "github.com"} · ${value.repo}`;
    const prior = repos.get(key);
    repos.set(key, { label, count: (prior?.count ?? 0) + 1 });
  }
  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <button type="button" onClick={() => setRepoKey(null)} aria-current={current(repoKey === null)} className="flex w-full justify-between rounded px-3 py-2 text-sm hover:bg-[#161b22]">
          <span>All repositories</span><span>{rows.length} shown</span>
        </button>
        {[...repos].sort((a, b) => b[1].count - a[1].count).map(([key, repo]) => (
          <button type="button" key={key} onClick={() => setRepoKey(key)} aria-current={current(repoKey === key)} className="flex w-full justify-between gap-2 rounded px-3 py-2 text-left text-sm hover:bg-[#161b22]">
            <span className="truncate">{repo.label}</span><span>{repo.count} shown</span>
          </button>
        ))}
      </div>
    </nav>
  );
}

function GitLabRow({ mr, parent, onOpen, opened, cursored }: { mr: MergeRequest; parent?: number; onOpen: () => void; opened: boolean; cursored: boolean }) {
  const density = useFilters(s => s.density);
  return (
    <div role="button" tabIndex={0} aria-current={current(opened)} onClick={onOpen} onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onOpen(); } }} className={`cursor-pointer border-b border-[#30363d] px-4 ${density === "dense" ? "py-1.5" : "py-3"} hover:bg-[#161b22] ${cursored ? "ring-2 ring-inset ring-[#1f6feb]" : ""}`}>
      <div className="text-xs text-[#8b949e]">GitLab · {mr.source.host} · {mr.repo}</div>
      <div className="mt-1 font-semibold">{mr.title} <span className="text-xs font-normal text-[#8b949e]">!{mr.number}</span></div>
      <div className="mt-1 text-xs text-[#8b949e]">
        {mr.is_draft ? "Draft · " : ""}{mr.author} · {mr.head_ref} → {mr.base_ref}
        {mr.ci === null ? " · CI unknown" : ` · CI ${mr.ci}`}
        {mr.review === null ? " · Review unknown" : ` · Review ${mr.review.replaceAll("_", " ")}`}
      </div>
      <div className="mt-1 text-xs text-[#8b949e]">
        {parent === undefined ? null : `Targets branch of !${parent} · `}
        {mr.in_merge_queue === true ? "In merge train · " : ""}
        {mr.unresolved_threads != null ? `${mr.unresolved_threads}${mr.unresolved_threads_floor !== false ? "+" : ""} unresolved · ` : ""}
        {mr.reviewers?.length ? `Reviewers: ${mr.reviewers.join(", ")}` : ""}
      </div>
      {mr.labels?.length ? <div className="flex flex-wrap gap-1">{mr.labels.map(label => <span className="rounded border border-[#30363d] px-1 text-xs" key={label.name}>{label.name}</span>)}</div> : null}
      <ExternalLink href={mr.url} className="mt-1 inline-block text-xs text-[#4493f8] hover:underline">Open on GitLab</ExternalLink>
    </div>
  );
}

export function GitLabSummary({ identity, mr, onBack }: { identity: PrIdentity; mr?: MergeRequest; onBack: () => void }) {
  // Closing removes the MR from the open queue. Share the detail query with
  // GitLabDetail so its title and link survive that queue change and follow
  // the authoritative readback after reopen.
  const viewer = useGitLabViewer();
  const detail = useQuery({ queryKey: ["gitlab-detail", prKey(identity), viewer], queryFn: async () => { const result = await getGitLabDetail(identity); if (viewer !== undefined && result.viewer !== viewer) throw new Error("GitLab account changed. Refresh account status."); return result; }, enabled: viewer !== null, staleTime: 60_000, retry: false });
  const core = detail.data?.core;
  const title = core?.title ?? mr?.title;
  const url = core?.url ?? mr?.url;
  return (
    <div className="rounded-md border border-[#30363d] bg-[#161b22] p-4 [&_button]:min-h-11 sm:[&_button]:min-h-0">
      <button type="button" onClick={onBack} className="mb-3 text-sm text-[#4493f8]">← Back to list</button>
      <div className="text-xs text-[#8b949e]">GitLab · {identity.source?.host} · {identity.repo} !{identity.number}</div>
      {title ? <>
        <h2 className="mt-1 text-lg font-semibold">{title}</h2>
        <p className="mt-2 text-sm text-[#8b949e]">{core?.author ?? mr?.author ?? "Unknown author"} · {core?.head_ref ?? mr?.head_ref} → {core?.base_ref ?? mr?.base_ref}{(core?.is_draft ?? mr?.is_draft) ? " · Draft" : ""}</p>
      </> : null}
      {/* The open queue may omit a selected MR after a close or partial poll.
          Identity keeps detail, drafts and action receipts mounted until Back. */}
      <GitLabDetail key={`${prKey(identity)}:${viewer}`} identity={identity} />
      {url ? <ExternalLink href={url} className="mt-3 inline-block text-sm text-[#4493f8] hover:underline">Open on GitLab</ExternalLink> : null}
    </div>
  );
}

export function SourceQueue({
  selection, github, gitlab, githubLoading, gitlabLoading, githubError, gitlabError,
  githubCoverage, gitlabCoverage, onOpen, onRefreshGitHub, onRefreshGitLab,
  githubStaleSecs, gitlabStaleSecs,
  canWriteGitHub,
}: {
  selection: SourceSelection;
  github: PullRequest[];
  gitlab: MergeRequest[] | undefined;
  githubLoading: boolean;
  gitlabLoading: boolean;
  githubError: string | null;
  gitlabError: string | null;
  githubCoverage: SourceCoverage | null;
  gitlabCoverage: SourceCoverage | null;
  githubStaleSecs: number | null;
  gitlabStaleSecs: number | null | "unknown";
  canWriteGitHub: boolean;
  onOpen: (identity: PrIdentity) => void;
  onRefreshGitHub: () => void;
  onRefreshGitLab: () => void;
}) {
  const repoKey = useSourceSelection((s) => s.repoKey);
  const query = useSourceSelection((s) => s.query);
  const filters = useActiveFilters();
  const checked = useFilters((s) => s.checked);
  const setChecked = useFilters((s) => s.setChecked);
  const [gitlabSelected, setGitlabSelected] = useState<Set<string>>(() => new Set());
  const [bulkBusy, setBulkBusy] = useState(false);
  const selectedPr = useFilters((s) => s.selectedPr);
  const cursor = useFilters((s) => s.cursor);
  const setCursor = useFilters((s) => s.setCursor);
  const rows = useMemo(() => combinedRows(github, gitlab ?? [], selection, repoKey, query, filters), [github, gitlab, selection, repoKey, query, filters]);
  const githubRows = rows.flatMap(row => row.kind === "github" ? [row.value] : []);
  const stacked = deriveStacked(githubRows);
  const gitlabStacked = gitlabParents(rows.flatMap(row => row.kind === "gitlab" ? [row.value] : []));
  const visibleKeys = githubRows.map(prKey);
  const selectRange = (from: string, to: string) => {
    const a = visibleKeys.indexOf(from), b = visibleKeys.indexOf(to);
    if (a >= 0 && b >= 0) setChecked([...new Set([...checked, ...visibleKeys.slice(Math.min(a, b), Math.max(a, b) + 1)])]);
  };
  useRowCursor({
    toggle: (index) => {
      const row = rows[index];
      if (!row) return;
      const key = prKey(row.value);
      if (row.kind === "github" && canWriteGitHub) setChecked(checked.includes(key) ? checked.filter(k => k !== key) : [...checked, key]);
      if (row.kind === "gitlab" && !bulkBusy) setGitlabSelected(previous => { const next = new Set(previous); if (next.has(key)) next.delete(key); else if (next.size < 10) next.add(key); return next; });
    },
    rows: () => rows.length,
    open: (index) => {
      const row = rows[index];
      if (row) onOpen(prIdentity(row.value));
    },
  });
  useEffect(() => {
    if (cursor !== null && cursor >= rows.length) setCursor(rows.length > 0 ? rows.length - 1 : null);
  }, [cursor, rows.length, setCursor]);
  const sources = [
    ...(selection === "gitlab" ? [] : [{ name: "GitHub", rows: (githubLoading || githubError) && github.length === 0 ? undefined : github.length, loading: githubLoading, error: githubError, coverage: githubCoverage, staleSecs: githubStaleSecs, retry: onRefreshGitHub }]),
    { name: "GitLab", rows: gitlab?.length, loading: gitlabLoading, error: gitlabError, coverage: gitlabCoverage, staleSecs: gitlabStaleSecs, retry: onRefreshGitLab },
  ];
  return <div className="space-y-3">
    <p className="text-xs text-[#8b949e]">GitLab queues use the host configured in Settings on the desktop.</p>
    {sources.map((source) => <div key={source.name}>
      {source.error ? <div role="alert" className="rounded-md border border-[#d29922]/40 bg-[#d29922]/10 px-4 py-2 text-sm text-[#d29922]">
        {source.name}: {source.error}{source.rows !== undefined ? " Saved rows remain visible." : ""}
        <button type="button" onClick={source.retry} className="ml-2 underline">Retry</button>
      </div> : source.loading && source.rows === undefined ? <p role="status" className="text-sm text-[#8b949e]">Loading {source.name}…</p> : null}
      {source.rows !== undefined && source.coverage !== "complete" ? <p role="status" className="mt-1 text-xs text-[#d29922]">{coverageMessage(source.name, source.coverage, source.rows)}</p> : null}
      {source.rows !== undefined && source.staleSecs !== null ? <p role="status" className="mt-1 text-xs text-[#d29922]">{source.staleSecs === "unknown" ? `${source.name} saved list age could not be confirmed.` : `${source.name} is showing a saved list from ${relativeSeconds(source.staleSecs)}.`}</p> : null}
    </div>)}
    <FilterBar prs={[...github, ...(gitlab ?? []).map(row => ({ labels: row.labels ?? [] }))]} />
    {selection === "both" && canWriteGitHub ? <BulkBar prs={github} /> : null}
    <GitLabBulkActions rows={(gitlab ?? []).filter(row => gitlabSelected.has(prKey(row)))} onBusy={setBulkBusy} onSettled={() => { setGitlabSelected(new Set()); }} />
    <div className="rounded-md border border-[#30363d]">
      <div className="border-b border-[#30363d] bg-[#161b22] px-4 py-3 text-sm font-semibold">{canWriteGitHub && githubRows.length > 0 ? <label className="mr-3"><input aria-label="Select all GitHub requests" type="checkbox" checked={visibleKeys.every(key => checked.includes(key))} onChange={e => setChecked(e.target.checked ? [...new Set([...checked, ...visibleKeys])] : checked.filter(key => !visibleKeys.includes(key)))} /> GitHub</label> : null}{rows.length} shown</div>
      {rows.length === 0 ? <p className="px-4 py-10 text-center text-sm text-[#8b949e]">
        {sources.some((s) => s.rows === undefined)
          ? sources.some((s) => s.error) ? "Some sources could not be checked. No requests are available to show." : "Waiting for a source to answer…"
          : query || repoKey ? "No requests match this search or repository." : "No open requests were returned by the selected sources."}
      </p> : rows.map((row, index) => row.kind === "github"
        ? <PrRow key={prKey(row.value)} pr={row.value} onOpen={() => onOpen(prIdentity(row.value))} opened={selectedPr !== null && prKey(selectedPr) === prKey(row.value)} cursored={cursor === index} canWrite={canWriteGitHub} selectable={canWriteGitHub} onRange={selectRange} stackedOn={stacked.get(row.value.id)} showSource />
        : <div key={prKey(row.value)} className="flex items-start">
          <input type="checkbox" className="ml-3 mt-4" aria-label={`Select GitLab ${row.value.source.host} ${row.value.repo} !${row.value.number}`} checked={gitlabSelected.has(prKey(row.value))} disabled={bulkBusy || (!gitlabSelected.has(prKey(row.value)) && gitlabSelected.size >= 10)} onChange={(event) => setGitlabSelected((previous) => { const next = new Set(previous); if (event.target.checked) next.add(prKey(row.value)); else next.delete(prKey(row.value)); return next; })} />
          <div className="min-w-0 flex-1"><GitLabRow mr={row.value} parent={gitlabStacked.get(prKey(row.value))} onOpen={() => onOpen(prIdentity(row.value))} opened={selectedPr !== null && prKey(selectedPr) === prKey(row.value)} cursored={cursor === index} /></div>
        </div>)}
    </div>
  </div>;
}
