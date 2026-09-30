import type { PullRequest } from "../types/pr";

type SourceCoverage = "complete" | "unknown" | { partial: { total: number | null } };

export type SourceStatus = {
  source: { provider: string; host: string };
  list: "authored" | "reviewing";
  phase: "not_requested" | "fetching" | "ready" | "partial" | "unknown" | "retrying" | "failed" | "not_asked";
  error: string | null;
  // Absent on older desktops. Their legacy array replies remain supported.
  session?: string;
  revision?: number;
  receipt_revision?: number | null;
  completed_request?: string | null;
  prs?: PullRequest[] | null;
  coverage?: SourceCoverage | null;
};
export type RefreshReply = PullRequest[] | { request_id: string; update: SourceStatus };
type Request = { id: string; order: number; rows: number; status: number; completed: boolean; session: string | undefined };
type Snapshot = { prs: PullRequest[] | undefined; error: string | null; modern: boolean; coverage?: SourceCoverage | null };

/// A qualifier for the accepted receipt, never for the most recent attempt.
/// Missing counts and partial coverage without a positive measured gap use the
/// generic warning; only a complete receipt can clear it with zero.
export function receiptAdvisory(snapshot: Snapshot, kind: "total" | "missing"): number | null | undefined {
  const { coverage, prs } = snapshot;
  if (coverage === undefined || prs === undefined) return undefined;
  if (coverage === "complete") return 0;
  const total = typeof coverage === "object" && coverage !== null ? coverage.partial.total : null;
  if (total === null || total <= prs.length) return null;
  return kind === "total" ? total : total - prs.length;
}

/// Reconcile the command and event connections independently. Provider outcomes
/// are ordered by desktop revision; a transport failure belongs to a particular
/// local request and cannot be cleared by an unrelated background publication.
export class SourceRefreshState {
  private value: Snapshot = { prs: undefined, error: null, modern: false };
  private backendError: string | null = null;
  private legacyStatusError = false;
  private transportError: { id: string; message: string } | null = null;
  private session: string | undefined;
  private retiredSessions = new Set<string>();
  private revision = -1;
  private receiptRevision = -1;
  private coverage: SourceCoverage | null | undefined;
  private rowEpoch = 0;
  private statusEpoch = 0;
  private order = 0;
  private requests = new Map<string, Request>();
  private listeners = new Set<() => void>();

  readonly snapshot = () => this.value;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private publish(prs = this.value.prs) {
    this.value = { prs, error: this.transportError?.message ?? this.backendError, modern: this.session !== undefined, coverage: this.coverage };
    for (const listener of this.listeners) listener();
  }
  start(id: string): Request {
    const request = { id, order: ++this.order, rows: this.rowEpoch, status: this.statusEpoch, completed: false, session: this.session };
    this.requests.set(id, request);
    return request;
  }
  accept(update: SourceStatus) {
    const modern = update.session !== undefined && update.revision !== undefined;
    if (!modern) {
      if (this.session !== undefined) return;
      if (update.phase !== "fetching") this.statusEpoch++;
      this.backendError = update.phase === "retrying" ? null : update.error;
      this.legacyStatusError = update.error !== null;
      this.publish();
      return;
    }
    if (this.retiredSessions.has(update.session!)) return;
    if (this.session !== update.session) {
      if (this.session !== undefined) this.retiredSessions.add(this.session);
      this.session = update.session;
      this.revision = -1;
      this.receiptRevision = -1;
    }
    if (update.completed_request) {
      const request = this.requests.get(update.completed_request);
      if (request) request.completed = true;
      if (request?.order === this.order || this.transportError?.id === update.completed_request) this.transportError = null;
    }
    if (update.revision! > this.revision) {
      this.revision = update.revision!;
      this.backendError = update.phase === "retrying" ? null : update.error;
      this.statusEpoch++;
    }
    let rows = this.value.prs;
    if (update.prs != null && update.receipt_revision != null && update.receipt_revision > this.receiptRevision) {
      this.receiptRevision = update.receipt_revision;
      this.coverage = update.coverage ?? null;
      this.rowEpoch++;
      rows = update.prs;
    }
    this.publish(rows);
  }
  legacyRows(prs: PullRequest[]) {
    if (this.session !== undefined) return;
    this.rowEpoch++;
    if (!this.legacyStatusError) this.backendError = null;
    this.publish(prs);
  }
  legacyError(error: string | null) {
    if (this.session !== undefined) return;
    this.backendError = error;
    this.legacyStatusError = false;
    this.publish();
  }
  resolve(request: Request, reply: RefreshReply) {
    if (Array.isArray(reply)) {
      // Version skew: older desktops ignore requestId and return arrays.
      if (request.rows === this.rowEpoch) this.legacyRows(reply);
      if (request.status === this.statusEpoch) this.backendError = null;
    } else if (reply.update.session === this.session || this.session === request.session) {
      // A new-session event may have overtaken a reply from an old desktop
      // process even if we never observed that old session before this call.
      this.accept(reply.update);
    }
    if (request.order === this.order) this.transportError = null;
    this.requests.delete(request.id);
    this.publish();
    return this.value.prs;
  }
  /// Confirmed mutations change the authoritative rows without claiming a new
  /// provider receipt. Replaying the same receipt must not erase that fact.
  patchRows(patch: (rows: PullRequest[]) => PullRequest[]) {
    if (this.value.prs === undefined) return;
    const rows = patch(this.value.prs);
    if (rows === this.value.prs) return;
    this.rowEpoch++;
    this.publish(rows);
  }
  reject(request: Request, error: unknown) {
    // A matching terminal event proves the desktop already reported this
    // request's provider outcome. A later command rejection adds no outcome.
    const legacyOutcome = this.session === undefined && request.status !== this.statusEpoch;
    if (!request.completed && !legacyOutcome && request.order === this.order) {
      this.transportError = { id: request.id, message: typeof error === "string" ? error : error instanceof Error ? error.message : "Refresh failed" };
    }
    this.requests.delete(request.id);
    this.publish();
  }
  dismiss() {
    this.transportError = null;
    this.backendError = null;
    this.publish();
  }
}
