import type { PrIdentity } from "./identity";
import type { SourceCoverage } from "../api/tauri";

export type GitLabAction = "approve" | "request_changes" | "enqueue_train" | "comment" | "reply" | "resolve" | "unresolve" | "merge" | "close" | "reopen" | "draft" | "ready" | "rebase" | "retry_ci" | "enable_auto_merge" | "disable_auto_merge";

export interface GitLabCapabilities {
  viewer?: string | null;
  identity: PrIdentity;
  head_oid: string | null;
  actions: { action: GitLabAction; allowed: boolean; reason: string | null }[];
  discussions: { id: string; can_reply: boolean; can_resolve: boolean; resolved: boolean }[];
  discussions_complete: boolean;
}

export interface GitLabActionRequest {
  expected_viewer?: string | null;
  identity: PrIdentity;
  action: GitLabAction;
  expected_head: string | null;
  body?: string;
  discussion_id?: string;
}

export interface GitLabReceipt {
  identity: PrIdentity;
  action: GitLabAction;
  outcome: "verified" | "unverified";
  message: string;
}

type ReadState<T> = { state: "available"; value: T } | { state: "unavailable"; issue: string };
type Paged<T> = { items: T[]; total: number | null; coverage: SourceCoverage };
interface Comment { id: number; author: string | null; body: string | null; created_at: string | null; system: boolean | null }
export interface GitLabDiscussion { id: string; individual_note: boolean | null; notes: { comment: Comment; kind: string | null; resolvable: boolean | null; resolved: boolean | null }[] }

export interface GitLabDetail {
  viewer?: string | null;
  core: {
    identity: PrIdentity; id: number; title: string; url: string; state: string; is_draft: boolean;
    body: string | null; author: string | null; head_ref: string; head_oid: string | null; base_ref: string;
    detailed_merge_status: string | null; blocking_discussions_resolved: boolean | null;
  };
  pipelines: {
    pipelines: ReadState<Paged<{ id: number; project_id: number | null; sha: string; status: string; url: string | null }>>;
    current_head_jobs: ReadState<Paged<{ id: number; name: string; status: string; allow_failure: boolean | null; url: string | null }>> | null;
  };
  approvals: ReadState<{ required: number | null; left: number | null; approved_by: string[]; user_has_approved: boolean | null; user_can_approve: boolean | null }>;
  approval_rules: ReadState<{ id: number | null; name: string | null; approved: boolean | null; approvals_required: number | null }[]>;
  comments: ReadState<Paged<Comment>>;
  discussions: ReadState<{ discussions: Paged<GitLabDiscussion>; unresolved_resolvable: number | null }>;
}
