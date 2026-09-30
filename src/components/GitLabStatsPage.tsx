import { GitLabActivityChart } from "./GitLabActivityChart";
import { ExternalLink } from "./ExternalLink";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useGitLabStats, useGitLabStatsTree, useGitLabBackfill, type GitLabScope, type GitLabStatsReport } from "../api/gitlabStats";

function qualified(n: number, complete: boolean) { return complete ? String(n) : `At least ${n}`; }

export function GitLabStatsResults({ report }: { report: GitLabStatsReport }) {
  const complete = report.coverage.complete;
  const assignmentsComplete = complete && report.reviewer_rows_measured === report.counts.created;
  return <div className="space-y-6">
    <GitLabActivityChart report={report} />
    <p className="text-sm text-muted-foreground">MRs created {report.start.slice(0, 10)} through {report.end.slice(0, 10)} (UTC). Merged and closed counts describe this created cohort. Snapshot {new Date(report.fetched_at).toLocaleString()}.</p>
    {!complete && <p role="status" className="text-sm text-amber-600">Partial GitLab data: {report.coverage.stop.replaceAll("_", " ")}. {report.coverage.received} MRs retrieved{report.coverage.total === null ? "" : ` of ${report.coverage.total}`}. Counts are lower bounds; averages are withheld.</p>}
    {report.coverage.rate_remaining !== null && <p className="text-sm text-muted-foreground">GitLab requests remaining: {report.coverage.rate_remaining}{report.coverage.rate_reset === null ? "" : `; reset ${new Date(report.coverage.rate_reset * 1000).toLocaleString()}`}</p>}
    <dl aria-label="MR counts" className="grid grid-cols-2 gap-3 sm:grid-cols-4">{Object.entries(report.counts).map(([name, count]) => <div key={name}><dt className="capitalize text-sm">{name}</dt><dd className="text-xl font-semibold">{qualified(count, complete)}</dd></div>)}</dl>
    <section><h2 className="font-semibold">Created per day</h2><div className="max-h-64 overflow-auto"><table className="w-full text-left text-sm"><thead><tr><th>Date (UTC)</th><th>Created</th></tr></thead><tbody>{report.series.map(day => <tr key={day.day}><td>{day.day}</td><td>{qualified(day.created, complete)}</td></tr>)}</tbody></table></div></section>
    <section><h2 className="font-semibold">Authors in retrieved MRs</h2><div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr><th>Author</th><th>Created</th><th>Merged</th><th>Mean time to merge</th></tr></thead><tbody>{report.authors.map(author => <tr key={author.username}><td>{author.username}</td><td>{qualified(author.created, complete)}</td><td>{qualified(author.merged, complete)}</td><td>{complete && author.mean_merge_hours !== null && author.timed_merges === author.merged ? `${author.mean_merge_hours.toFixed(1)} hours (${author.timed_merges} MRs)` : "Unavailable"}</td></tr>)}</tbody></table></div></section>
    <section><h2 className="font-semibold">Current reviewer assignments</h2><p className="text-sm text-muted-foreground">Reviewer lists measured for {report.reviewer_rows_measured} of {report.counts.created} retrieved MRs. Assignments do not measure review activity.</p><ul className="text-sm">{report.reviewers.map(reviewer => <li key={reviewer.username}>{reviewer.username}: {qualified(reviewer.assigned, assignmentsComplete)} assigned</li>)}</ul></section>
    <section><h2 className="font-semibold">Merged in the window</h2><p className="text-sm text-muted-foreground">Includes MRs created before this window. This is a separate cohort.</p>
      {report.merged_window ? <>
        <p>Snapshot {new Date(report.merged_window.fetched_at).toLocaleString()}.</p>
        <p>{qualified(report.merged_window.count, report.merged_window.coverage.complete)} merged MRs</p>
        {!report.merged_window.coverage.complete && <p role="status">Merged data is partial: {report.merged_window.coverage.stop.replaceAll("_", " ")}. Means are unavailable.</p>}
        <table className="w-full text-left text-sm"><thead><tr><th>Date (UTC)</th><th>Merged</th></tr></thead><tbody>{report.merged_window.series.map(day => <tr key={day.day}><td>{day.day}</td><td>{qualified(day.merged, report.merged_window!.coverage.complete)}</td></tr>)}</tbody></table>
        <h3>Authors of merged MRs</h3><ul>{report.merged_window.authors.map(author => <li key={author.username}>{author.username}: {qualified(author.merged, report.merged_window!.coverage.complete)} merged; mean creation to merge: {report.merged_window!.coverage.complete && author.mean_merge_hours !== null ? `${author.mean_merge_hours.toFixed(1)} hours` : "Unavailable"}</li>)}</ul>
      </> : <p>Merged-in-window data unavailable{report.merged_error ? `: ${report.merged_error}` : "."}</p>}
    </section>
    <section><h2 className="font-semibold">Comment participation in the created cohort</h2><p className="text-sm text-muted-foreground">Non-author, non-system comments through the snapshot end. Comments may include bots. At most 10 MRs and 100 notes per MR are read per load.</p>
      {report.activity ? <>
        <p>{report.activity.mrs_checked} of {report.activity.mrs_total} retrieved MRs have complete comment reads. {report.activity.comments === null ? "Unavailable" : qualified(report.activity.comments, report.activity.complete)} comments.</p>
        <p>Mean first response among MRs with a response: {report.activity.complete && report.activity.mean_first_response_hours !== null ? `${report.activity.mean_first_response_hours.toFixed(1)} hours (${report.activity.responded_mrs} MRs)` : "Unavailable"}</p>
        {report.activity.rate_remaining !== null && <p>Comment read requests remaining: {report.activity.rate_remaining}{report.activity.rate_reset === null ? "" : `; reset ${new Date(report.activity.rate_reset * 1000).toLocaleString()}`}</p>}
        {report.activity.failures.map((failure, index) => <p role="status" key={index}>{failure}</p>)}
        <ul>{report.activity.participants.map(person => <li key={person.username}>{person.username}: {qualified(person.comments, report.activity!.complete)} comments on {qualified(person.mrs, report.activity!.complete)} MRs</li>)}</ul>
      </> : <p>Comment activity and response times unavailable.</p>}
    </section>
    <section><h2 className="font-semibold">Current formal review outcomes</h2>
      <p className="text-sm text-muted-foreground">Current approval and reviewer states for up to 10 retrieved MRs. These are a snapshot, not a history of review actions. GitLab does not provide the time of the first submitted review through these endpoints.</p>
      {report.review_evidence ? <>
        <p>{report.review_evidence.approvals_checked} of {report.review_evidence.mrs_total} MRs checked for approvals; {report.review_evidence.current_approvals === null ? "Unavailable" : qualified(report.review_evidence.current_approvals, report.review_evidence.approvals_complete)} current approvals.</p>
        <p>{report.review_evidence.changes_checked} of {report.review_evidence.mrs_total} MRs checked for change requests; {report.review_evidence.current_change_requests === null ? "Unavailable" : qualified(report.review_evidence.current_change_requests, report.review_evidence.changes_complete)} current change requests.</p>
        <p>Mean time to first current approval among approved MRs: {report.review_evidence.mean_first_current_approval_hours === null ? "Unavailable" : `${report.review_evidence.mean_first_current_approval_hours.toFixed(1)} hours (${report.review_evidence.timed_approved_mrs} MRs)`}. First formal review: Unavailable.</p>
        {report.review_evidence.failures.map((failure, index) => <p role="status" key={index}>{failure}</p>)}
        <ul>{report.review_evidence.reviewers.map(person => <li key={person.username}>{person.username}: {report.review_evidence!.current_approvals === null ? "Unavailable" : qualified(person.approvals, report.review_evidence!.approvals_complete)} current approvals; {report.review_evidence!.current_change_requests === null ? "Unavailable" : qualified(person.change_requests, report.review_evidence!.changes_complete)} current change requests</li>)}</ul>
      </> : <p>Current formal review outcomes unavailable.</p>}
    </section>
    <section><h2 className="font-semibold">MR history in this window</h2><ul className="space-y-2 text-sm">{report.history.map(mr => <li key={JSON.stringify([mr.source.provider, mr.source.host, mr.project, mr.iid])}><ExternalLink className="font-medium underline" href={mr.url}>{mr.project}!{mr.iid}</ExternalLink> — {mr.title} <span className="text-muted-foreground">({mr.state}; {mr.source.host})</span></li>)}</ul></section>
  </div>;
}

export function GitLabStatsPage({ host = "gitlab.com" }: { host?: string }) {
  const client = useQueryClient();
  const [selection, setSelection] = useState<{ account: string; scope: GitLabScope }>();
  const [days, setDays] = useState(30);
  const [revision, setRevision] = useState(0);
  const tree = useGitLabStatsTree(host, revision);
  const account = JSON.stringify([host, tree.data?.viewer]);
  const scope: GitLabScope = selection?.account === account ? selection.scope : { kind: "mine" };
  return <main className="mx-auto w-full max-w-5xl space-y-5 overflow-auto p-4 sm:p-6">
    <h1 className="text-xl font-semibold">GitLab MR Stats · {host}</h1>
    <button type="button" disabled={tree.isFetching} onClick={() => { client.removeQueries({ queryKey: ["stats", "gitlab", host] }); setRevision(value => value + 1); }} className="rounded border px-2">Refresh</button>
    {tree.isFetching && <p role="status">Loading GitLab scopes…</p>}
    {tree.isError && <p role="alert">Could not load GitLab scopes: {String(tree.error)}</p>}
    {/* Refresh always rediscovers the account and retries partial discovery.
        Unmount statistics while discovery runs, so old scopes/results cannot
        launch requests or remain visible against a newly selected account. */}
    {tree.isSuccess && !tree.isFetching && <GitLabStatsScope key={JSON.stringify([account, revision])}
      host={host} tree={tree.data} forceRefresh={revision > 0} scope={scope} days={days} setDays={setDays}
      setScope={next => setSelection({ account, scope: next })} />}
  </main>;
}

function GitLabStatsScope({ host, tree, scope, days, setScope, setDays, forceRefresh }: {
  forceRefresh: boolean;
  host: string;
  tree: NonNullable<ReturnType<typeof useGitLabStatsTree>["data"]>;
  scope: GitLabScope;
  days: number;
  setScope: (scope: GitLabScope) => void;
  setDays: (days: number) => void;
}) {
  const stats = useGitLabStats(host, tree.viewer, scope, days, forceRefresh);
  const [person, setPerson] = useState("");
  const groups = [...new Set([...(tree.groups ?? []), ...tree.projects.map(p => p.namespace).filter(Boolean)])].sort();
  return <>
    <div className="flex flex-wrap gap-3">
      <label>Scope <select aria-label="GitLab statistics scope" value={JSON.stringify(scope)} onChange={e => setScope(JSON.parse(e.target.value) as GitLabScope)} className="rounded border bg-background p-1">
        <option value={JSON.stringify({ kind: "mine" })}>My authored MRs</option>
        {scope.kind === "person" && <option value={JSON.stringify(scope)}>Author: {scope.path}</option>}
        {groups.map(path => <option key={`group:${path}`} value={JSON.stringify({ kind: "group", path })}>Group: {path}</option>)}
        {tree.projects.map(p => <option key={p.path} value={JSON.stringify({ kind: "project", path: p.path })}>Project: {p.path}</option>)}
      </select></label>
      <label>Window <select aria-label="GitLab statistics window" value={days} onChange={e => setDays(Number(e.target.value))} className="rounded border bg-background p-1"><option value={7}>7 days</option><option value={30}>30 days</option><option value={90}>90 days</option></select></label>
    </div>
    <form className="flex flex-wrap gap-2" onSubmit={e => { e.preventDefault(); if (/^[a-zA-Z0-9_.-]+$/.test(person) && person !== "." && person !== "..") setScope({ kind: "person", path: person }); }}>
      <label>Author username <input aria-label="GitLab author username" value={person} onChange={e => setPerson(e.target.value)} className="rounded border bg-background p-1" required /></label><button type="submit" className="rounded border px-2">View author</button>
    </form>
    {tree.group_error && <p role="status">Group discovery unavailable: {tree.group_error}</p>}
    {tree.group_coverage && !tree.group_coverage.complete && <p role="status">Group discovery is partial ({tree.group_coverage.stop.replaceAll("_", " ")}); more groups may exist.</p>}
    {!tree.coverage.complete && <p role="status">Project discovery is partial ({tree.coverage.stop.replaceAll("_", " ")}); more scopes may exist.</p>}
    {stats.isPending && <p role="status">Loading GitLab statistics…</p>}
    {stats.isError && <p role="alert">Could not load GitLab statistics: {String(stats.error)}</p>}
    {stats.data && !stats.isError && <GitLabStatsResults report={stats.data} />}
    <GitLabHistory key={JSON.stringify([host, tree.viewer, scope, days])} host={host} viewer={tree.viewer} scope={scope} days={days} />
  </>;
}

function GitLabHistory({ host, viewer, scope, days }: { host: string; viewer: string; scope: GitLabScope; days: number }) {
  const history = useGitLabBackfill(host, viewer, scope, days);
  return <section className="space-y-2">
    <h2 className="font-semibold">Retained daily history</h2>
    <p className="text-sm text-muted-foreground">Each click retrieves one missing closed UTC day, up to 20 list pages and 10 comment requests within 45 seconds. Completed count receipts are retained and skipped on resume. Historical comment reads can be partial. No background requests run.</p>
    <button type="button" className="rounded border px-2" disabled={history.isPending} onClick={() => history.mutate()}>{history.isPending ? "Loading history…" : "Load next history day"}</button>
    {history.isError && <p role="alert">Could not load history: {String(history.error)}</p>}
    {history.data && <>
      <p>{history.data.complete_days} of {history.data.requested_days} closed days have complete created and merged counts; {history.data.attempted_days} days attempted.</p>
      {history.data.error && <p role="alert">{history.data.error}</p>}
      <table className="w-full text-left text-sm"><thead><tr><th>Day (UTC)</th><th>Created</th><th>Merged in day</th><th>Snapshots</th></tr></thead><tbody>{history.data.slices.map(slice => <tr key={slice.start}><td>{slice.start.slice(0,10)}</td><td>{qualified(slice.counts.created,slice.coverage.complete)}</td><td>{slice.merged_window ? qualified(slice.merged_window.count,slice.merged_window.coverage.complete) : "Unavailable"}</td><td>Created: {new Date(slice.fetched_at).toLocaleString()}{slice.merged_window && <>; merged: {new Date(slice.merged_window.fetched_at).toLocaleString()}</>}</td></tr>)}</tbody></table>
    </>}
  </section>;
}
