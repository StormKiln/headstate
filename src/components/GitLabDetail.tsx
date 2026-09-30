import { useWritesPaused } from "../lib/useWritesPaused";
import { useGitLabViewer } from "../api/authAvailability";
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getGitLabActionCapabilities, getGitLabDetail, gitLabAction } from "../api/tauri";
import type { PrIdentity } from "../types/identity";
import type { GitLabAction, GitLabActionRequest, GitLabCapabilities, GitLabDiscussion, GitLabReceipt } from "../types/gitlabActions";
import { prKey } from "../lib/prIdentity";
import { GitLabClaudify } from "./GitLabClaudify";
import { Markdown } from "./Markdown";
import { ExternalLink } from "./ExternalLink";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

const LABEL: Record<GitLabAction, string> = {
  request_changes: "Request changes", enqueue_train: "Add to merge train",
  approve: "Approve", comment: "Post comment", reply: "Reply", resolve: "Resolve discussion", unresolve: "Reopen discussion",
  merge: "Merge MR", close: "Close MR", reopen: "Reopen MR", draft: "Mark draft", ready: "Mark ready",
  rebase: "Rebase source branch", retry_ci: "Retry failed CI", enable_auto_merge: "Enable auto-merge", disable_auto_merge: "Cancel auto-merge",
};
const BUTTON = "rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d] disabled:opacity-40";
const PRIMARY: GitLabAction[] = ["approve", "request_changes", "enqueue_train", "merge", "close", "reopen", "draft", "ready", "rebase", "retry_ci", "enable_auto_merge", "disable_auto_merge"];

function permitted(capabilities: GitLabCapabilities | undefined, action: GitLabAction): boolean {
  return capabilities?.actions.some((c) => c.action === action && c.allowed) === true;
}

function Discussion({ discussion, capabilities, busy, act }: {
  discussion: GitLabDiscussion; capabilities: GitLabCapabilities | undefined; busy: boolean;
  act: (action: GitLabAction, body?: string, discussion?: string) => void;
}) {
  const [reply, setReply] = useState("");
  const permission = capabilities?.discussions.find((d) => d.id === discussion.id);
  return <section className="space-y-2 rounded border border-[#30363d] p-3">
    {discussion.notes.map(({ comment, resolvable, resolved }) => <div key={comment.id}>
      <p className="text-xs text-[#8b949e]">{comment.author ?? "Unknown author"}{resolvable ? resolved === null ? " · Resolution unknown" : resolved ? " · Resolved" : " · Unresolved" : ""}</p>
      <Markdown>{comment.body ?? "Comment text unavailable"}</Markdown>
    </div>)}
    {permission?.can_resolve ? <button className={BUTTON} type="button" disabled={busy} onClick={() => act(permission.resolved ? "unresolve" : "resolve", undefined, discussion.id)}>{permission.resolved ? "Reopen discussion" : "Resolve discussion"}</button> : null}
    {permission?.can_reply && permitted(capabilities, "reply") ? <div>
      <textarea aria-label="Discussion reply" value={reply} onChange={(e) => setReply(e.target.value)} maxLength={32768} className="w-full rounded border border-[#30363d] bg-[#0d1117] p-2 text-sm" />
      <button className={BUTTON} type="button" disabled={busy || !reply.trim()} onClick={() => act("reply", reply, discussion.id)}>Reply</button>
    </div> : null}
  </section>;
}

export function GitLabDetail({ identity }: { identity: PrIdentity }) {
  const viewer = useGitLabViewer();
  const paused = useWritesPaused();
  const key = prKey(identity);
  const detail = useQuery({ queryKey: ["gitlab-detail", key, viewer], queryFn: async () => { const result = await getGitLabDetail(identity); if (viewer !== undefined && result.viewer !== viewer) throw new Error("GitLab account changed. Refresh account status."); return result; }, enabled: viewer !== null, staleTime: 60_000, retry: false });
  const caps = useQuery({ queryKey: ["gitlab-actions", key, viewer], queryFn: async () => { const result = await getGitLabActionCapabilities(identity); if (viewer !== undefined && result.viewer !== viewer) throw new Error("GitLab account changed. Refresh account status."); return result; }, enabled: viewer !== null, staleTime: 30_000, retry: false });
  const [pending, setPending] = useState<GitLabActionRequest | null>(null);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [receipt, setReceipt] = useState<GitLabReceipt | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [comment, setComment] = useState("");
  const refresh = () => { void detail.refetch(); void caps.refetch(); };
  // Permission and detail reads must describe the same commit the user sees.
  const capabilities = !caps.error && !detail.error && caps.data && detail.data && prKey(caps.data.identity) === key && caps.data.head_oid === detail.data.core.head_oid ? caps.data : undefined;
  const act = (action: GitLabAction, body?: string, discussion_id?: string) => {
    if (!capabilities || busy || uncertain || paused) return;
    setPending({ identity, action, expected_viewer: capabilities.viewer, expected_head: detail.data?.core.head_oid ?? null, body, discussion_id });
  };
  const run = async () => {
    if (!pending || busy || uncertain || paused) return;
    const request = pending;
    setBusy(true); setPending(null); setError(null); setReceipt(null);
    try {
      const result = await gitLabAction(request);
      setReceipt(result);
      setUncertain(result.outcome === "unverified");
      if (result.outcome === "verified" && request.action === "comment") setComment("");
    } catch (e) {
      setError(`${String(e)} Refresh the MR before trying again.`);
      setUncertain(true);
    } finally {
      setBusy(false);
    }
  };
  if (!detail.data) return <div className="mt-3 text-sm" role={detail.error ? "alert" : "status"}>
    {detail.error ? String(detail.error) : "Checking GitLab MR details…"}
    {detail.error ? <button type="button" onClick={refresh} className={`${BUTTON} ml-2`}>Retry detail</button> : null}
  </div>;
  const d = detail.data;
  const discussions = d.discussions.state === "available" ? d.discussions.value.discussions.items : [];
  const discussionNotes = new Set(discussions.flatMap((thread) => thread.notes.map((n) => n.comment.id)));
  return <div className="mt-4 space-y-4">
    <div className="text-sm text-[#8b949e]">{d.core.state}{d.core.is_draft ? " · Draft" : ""} · Head {d.core.head_oid?.slice(0, 12) ?? "unknown"} · Merge status: {d.core.detailed_merge_status ?? "unknown"}
      <button type="button" className={`${BUTTON} ml-2`} disabled={busy || detail.isFetching || caps.isFetching} onClick={refresh}>Refresh details</button>
    </div>
    <GitLabClaudify detail={d} />
    {d.core.body ? <Markdown>{d.core.body}</Markdown> : null}
    {detail.error ? <p role="alert">Detail refresh failed; showing the previous detail. {String(detail.error)}</p> : null}
    {caps.error ? <p role="alert">Action permissions could not be checked. {String(caps.error)}</p> : !capabilities ? <p role="status">Action permissions and MR head have not yet been confirmed together.</p> : null}
    {receipt ? <p role={receipt.outcome === "verified" ? "status" : "alert"}>{LABEL[receipt.action]}: {receipt.message}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    {paused ? <p role="status">Writes paused: {paused}.</p> : null}
    {uncertain ? <div role="status">This write may already have completed. Inspect the MR on GitLab before allowing another write.
      <button type="button" className={`${BUTTON} ml-2`} onClick={() => { setUncertain(false); refresh(); }}>I checked GitLab; allow another action</button>
    </div> : null}
    <div className="flex flex-wrap gap-2">{PRIMARY.filter((a) => permitted(capabilities, a)).map((a) => <button key={a} type="button" className={BUTTON} disabled={busy || uncertain || !!paused || caps.isFetching || detail.isFetching} onClick={() => act(a)}>{LABEL[a]}</button>)}</div>
    <details className="text-xs text-[#8b949e]"><summary>Action availability</summary>
      <p>Rebase and request-changes reviews require a fresh head check, but GitLab does not provide an atomic head guard for these actions.</p>
      {caps.data?.actions.filter((c) => !c.allowed).map((c) => <p key={c.action}>{LABEL[c.action]}: {c.reason}</p>)}
      {caps.data && !caps.data.discussions_complete ? <p>Permissions were checked for the first 100 discussions. Later discussions can be acted on in GitLab.</p> : null}
    </details>
    {permitted(capabilities, "comment") ? <div>
      <label className="text-sm">Comment<textarea aria-label="MR comment" value={comment} onChange={(e) => setComment(e.target.value)} maxLength={32768} className="mt-1 w-full rounded border border-[#30363d] bg-[#0d1117] p-2" /></label>
      <button type="button" className={BUTTON} disabled={busy || !comment.trim()} onClick={() => act("comment", comment)}>Post comment</button>
    </div> : null}
    <section className="space-y-1 text-sm"><h3 className="font-semibold">Approvals</h3>
      {d.approvals.state === "unavailable" ? <p>Approvals unavailable ({d.approvals.issue}).</p> : <>
        <p>{d.approvals.value.approved_by.length ? `Approved by ${d.approvals.value.approved_by.join(", ")}` : "No approving reviewer returned."}</p>
        <p>Required: {d.approvals.value.required ?? "unknown"} · Remaining: {d.approvals.value.left ?? "unknown"}</p>
      </>}
      {d.approval_rules.state === "unavailable" ? <p>Approval rules unavailable ({d.approval_rules.issue}).</p> : null}
    </section>
    <section className="space-y-1 text-sm"><h3 className="font-semibold">CI</h3>
      {d.pipelines.pipelines.state === "unavailable" ? <p>Pipelines unavailable ({d.pipelines.pipelines.issue}).</p> : <>
        {d.pipelines.pipelines.value.coverage !== "complete" ? <p>Pipeline list is incomplete.</p> : null}
        {d.pipelines.pipelines.value.items.map((p) => <p key={p.id}>Pipeline {p.id}: {p.status}{p.sha === d.core.head_oid ? " · Current head" : " · Earlier head"}{p.url ? <> · <ExternalLink href={p.url}>Open pipeline</ExternalLink></> : null}</p>)}
        {!d.pipelines.pipelines.value.items.length ? <p>No pipelines returned. CI success has not been established.</p> : null}
      </>}
      {!d.pipelines.current_head_jobs ? <p>Current-head jobs were not checked.</p> : d.pipelines.current_head_jobs.state === "unavailable" ? <p>Current-head jobs unavailable ({d.pipelines.current_head_jobs.issue}).</p> : <>
        {d.pipelines.current_head_jobs.value.coverage !== "complete" ? <p>Job list is incomplete.</p> : null}
        {d.pipelines.current_head_jobs.value.items.map((job) => <p key={job.id}>{job.name}: {job.status}{job.allow_failure ? " · Allowed to fail" : ""}</p>)}
      </>}
    </section>
    <section className="space-y-3"><h3 className="font-semibold">Discussions</h3>
      {d.discussions.state === "unavailable" ? <p>Discussions unavailable ({d.discussions.issue}).</p> : <>
        <p className="text-sm">Unresolved threads: {d.discussions.value.unresolved_resolvable ?? "unknown"}{d.discussions.value.discussions.coverage !== "complete" ? " · Discussion list is incomplete" : ""}</p>
        {discussions.map((discussion) => <Discussion key={discussion.id} discussion={discussion} capabilities={capabilities} busy={busy || uncertain || !!paused} act={act} />)}
      </>}
      {d.comments.state === "unavailable" ? <p>Comments unavailable ({d.comments.issue}).</p> : <>
        {d.comments.value.coverage !== "complete" ? <p>Comment list is incomplete.</p> : null}
        {d.comments.value.items.filter((c) => !discussionNotes.has(c.id)).map((c) => <p className="whitespace-pre-wrap break-words text-sm" key={c.id}>{c.author ?? "Unknown author"}: {c.body ?? "Comment text unavailable"}</p>)}
      </>}
    </section>
    <Dialog open={pending !== null} onOpenChange={(open) => { if (!open) setPending(null); }}><DialogContent>
      <DialogTitle>{pending ? LABEL[pending.action] : "GitLab action"}</DialogTitle>
      <p className="text-sm">GitLab · {identity.source?.host} · {identity.repo} !{identity.number}</p>
      {pending?.action === "merge" || pending?.action === "enable_auto_merge" ? <p className="text-sm">This changes the target branch{pending.action === "enable_auto_merge" ? " when GitLab's checks pass" : " now"}. Head: {pending.expected_head?.slice(0, 12)}.</p> : null}
      {pending?.action === "rebase" ? <p className="text-sm">This rewrites the source branch by rebasing it onto the target. A concurrent push cannot be guarded atomically by GitLab.</p> : null}
      {pending?.body ? <p className="max-h-52 overflow-auto whitespace-pre-wrap break-words text-sm">{pending.body}</p> : null}
      <div className="flex gap-2"><button type="button" className={BUTTON} onClick={() => void run()} disabled={busy || !!paused}>Confirm</button><button type="button" className={BUTTON} onClick={() => setPending(null)}>Cancel</button></div>
    </DialogContent></Dialog>
  </div>;
}
