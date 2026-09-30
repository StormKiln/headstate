import { useWritesPaused } from "../lib/useWritesPaused";
import { useGitLabViewer } from "../api/authAvailability";
import { useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { getGitLabActionCapabilities, gitLabAction } from "../api/tauri";
import type { MergeRequest } from "../types/gitlab";
import type { GitLabAction, GitLabActionRequest } from "../types/gitlabActions";
import { prIdentity, prKey } from "../lib/prIdentity";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

const BULK: { action: GitLabAction; label: string }[] = [
  { action: "enqueue_train", label: "Add MRs to merge train" },
  { action: "approve", label: "Approve MRs" }, { action: "close", label: "Close MRs" },
  { action: "draft", label: "Mark draft" }, { action: "ready", label: "Mark ready" },
];

export function GitLabBulkActions({ rows, onSettled, onBusy }: {
  rows: MergeRequest[]; onSettled: () => void; onBusy: (busy: boolean) => void;
}) {
  const viewer = useGitLabViewer();
  const paused = useWritesPaused();
  const receipts = useQueries({ queries: rows.map((row) => ({
    queryKey: ["gitlab-actions", prKey(row), viewer], queryFn: () => getGitLabActionCapabilities(prIdentity(row)), enabled: viewer !== null, staleTime: 30_000, retry: false,
  })) });
  const [pending, setPending] = useState<{ label: string; requests: GitLabActionRequest[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<string[]>([]);
  const allChecked = rows.length > 0 && rows.length <= 10 && receipts.every((receipt, i) => !receipt.isFetching && !receipt.error && receipt.data && (viewer === undefined || receipt.data.viewer === viewer) && prKey(receipt.data.identity) === prKey(rows[i]) && receipt.data.head_oid === rows[i].head_oid);
  const allowed = BULK.filter(({ action }) => allChecked && receipts.every((receipt) => receipt.data?.actions.some((c) => c.action === action && c.allowed)));
  const run = async () => {
    if (!pending || busy || paused) return;
    const requests = pending.requests;
    setPending(null); setBusy(true); onBusy(true); setResults([]);
    const outcomes: string[] = [];
    // A bounded sequence retains each result; one refusal does not erase
    // successful writes to other identities. No action is automatically retried.
    for (const request of requests) {
      const label = `${request.identity.source?.host} · ${request.identity.repo} !${request.identity.number}`;
      try {
        const receipt = await gitLabAction(request);
        outcomes.push(`${label}: ${receipt.outcome === "verified" ? "verified" : "could not verify; refresh before acting again"}`);
      } catch (error) {
        outcomes.push(`${label}: ${String(error)} Refresh before acting again.`);
      }
      setResults([...outcomes]);
    }
    setBusy(false); onBusy(false);
    // Unverified writes may already have landed. Clear all attempted rows so
    // a repeated bulk click cannot silently resubmit any of them.
    onSettled();
  };
  if (!rows.length && !results.length) return null;
  return <section className="space-y-2 rounded border border-[#30363d] p-3 text-sm" aria-label="GitLab bulk actions">
    {paused ? <p role="status">Writes paused: {paused}.</p> : null}
    {rows.length ? <p>{rows.length} GitLab MRs selected (maximum 10). Actions require permission on every selected MR.</p> : null}
    {rows.length > 0 && !allChecked ? <p role="status">Permissions and selected MR heads are not confirmed together. Refresh the GitLab list if this persists.</p> : null}
    {allChecked && !allowed.length ? <p>No supported bulk action is available for every selected MR.</p> : null}
    <div className="flex flex-wrap gap-2">{allowed.map(({ action, label }) => <button type="button" key={action} disabled={busy || !!paused} className="rounded border border-[#30363d] px-3 py-1.5 disabled:opacity-40" onClick={() => setPending({ label, requests: rows.map((row, i) => ({ identity: prIdentity(row), action, expected_viewer: receipts[i].data?.viewer, expected_head: row.head_oid })) })}>{label}</button>)}</div>
    {results.length ? <div role="status">{results.map((result, i) => <p key={i}>{result}</p>)}</div> : null}
    <Dialog open={pending !== null} onOpenChange={(open) => { if (!open) setPending(null); }}><DialogContent>
      <DialogTitle>{pending?.label ?? "GitLab bulk action"}</DialogTitle>
      <p>Apply to these {pending?.requests.length} GitLab merge requests?</p>
      {pending?.requests.map(({ identity }) => <p key={prKey(identity)} className="break-words text-sm">{identity.source?.host} · {identity.repo} !{identity.number}</p>)}
      <div className="flex gap-2"><button type="button" disabled={busy || !!paused} onClick={() => void run()} className="rounded border border-[#30363d] px-3 py-1.5">Confirm</button><button type="button" onClick={() => setPending(null)}>Cancel</button></div>
    </DialogContent></Dialog>
  </section>;
}
