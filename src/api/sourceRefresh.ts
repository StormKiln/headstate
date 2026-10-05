import type { SourceSnapshot } from "./tauri";
import type { PullRequest } from "../types/pr";

export type SourceCoverage = "complete" | "unknown" | { partial: { total: number | null } };

export type SourceStatus = {
  source: { provider: string; host: string };
  list: "authored" | "reviewing";
  owner?: string | null;
  last_received_at?: string | null;
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
type Snapshot = { prs: PullRequest[] | undefined; error: string | null; modern: boolean; session?: string; coverage?: SourceCoverage | null; staleSecs?: number | null; fetchedAt?: string; savedOwner?: string };

/// A qualifier for the accepted receipt, never for the most recent attempt.
/// Missing counts and partial coverage without a positive measured gap use the
/// generic warning; only a complete receipt can clear it with zero.
export function receiptAdvisory(snapshot: Snapshot, kind: "total" | "missing"): number | null | undefined {
  const { coverage, prs } = snapshot;
  if (coverage === undefined || prs === undefined) return undefined;
  if (coverage === "complete") return 0;
  const total = typeof coverage === "object" && coverage !== null ? coverage.partial.total : null;
  const observed = prs.filter((pr) => pr.observation?.state !== "retained").length;
  if (total === null || total <= observed) return null;
  return kind === "total" ? total : total - observed;
}

/// Reconcile the command and event connections independently. Provider outcomes
/// are ordered by desktop revision; a transport failure belongs to a particular
/// local request and cannot be cleared by an unrelated background publication.
export class SourceRefreshState {
  private retained: { staleSecs: number | null; fetchedAt: string; savedOwner?: string } | undefined;
  private providerReceipt = false;
  private providerAt: string | undefined;
  private retired = false;
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
  private onProviderRows?: (rows: PullRequest[], session: string | undefined) => void;

  constructor(onProviderRows?: (rows: PullRequest[], session: string | undefined) => void) {
    this.onProviderRows = onProviderRows;
  }

  readonly snapshot = () => this.value;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private publish(prs = this.value.prs, fromProvider = false) {
    const providerAge = this.providerAt ? Math.max(0, Math.floor((Date.now() - Date.parse(this.providerAt)) / 1000)) : 0;
    this.value = { staleSecs: providerAge > 3600 ? providerAge : null, prs, error: this.transportError?.message ?? this.backendError, modern: this.session !== undefined, session: this.session, coverage: this.coverage, ...this.retained };
    for (const listener of this.listeners) listener();
    // Display patches and status-only publications are not provider evidence.
    // Deliver the accepted rows themselves, never a later patched snapshot.
    if (fromProvider && prs !== undefined) this.onProviderRows?.(prs, this.session);
  }
  start(id: string): Request {
    const request = { id, order: ++this.order, rows: this.rowEpoch, status: this.statusEpoch, completed: false, session: this.session };
    this.requests.set(id, request);
    return request;
  }
  allowsOwnership(update: SourceStatus) {
    if (this.retired || (update.session && this.retiredSessions.has(update.session))) return false;
    return update.session !== this.session || update.revision === undefined || update.revision > this.revision;
  }
  accept(update: SourceStatus) {
    if (this.retired) return;
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
    let fromProvider = false;
    if (update.prs != null && update.receipt_revision != null && update.receipt_revision > this.receiptRevision) {
      this.retained = undefined;
      this.providerReceipt = true;
      this.providerAt = update.last_received_at ?? undefined;
      this.receiptRevision = update.receipt_revision;
      this.coverage = update.coverage ?? null;
      this.rowEpoch++;
      rows = update.prs;
      fromProvider = true;
    }
    this.publish(rows, fromProvider);
  }
  legacyRows(prs: PullRequest[]) {
    if (this.retired) return;
    if (this.session !== undefined) return;
    this.retained = undefined;
    this.providerReceipt = true;
    this.providerAt = undefined;
    this.rowEpoch++;
    if (!this.legacyStatusError) this.backendError = null;
    this.publish(prs, true);
  }
  legacyError(error: string | null) {
    if (this.session !== undefined) return;
    this.backendError = error;
    this.legacyStatusError = false;
    this.publish();
  }
  resolve(request: Request, reply: RefreshReply) {
    if (this.retired) return undefined;
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
    if (this.retired) return;
    // A matching terminal event proves the desktop already reported this
    // request's provider outcome. A later command rejection adds no outcome.
    const legacyOutcome = this.session === undefined && request.status !== this.statusEpoch;
    if (!request.completed && !legacyOutcome && request.order === this.order) {
      this.transportError = { id: request.id, message: typeof error === "string" ? error : error instanceof Error ? error.message : "Refresh failed" };
    }
    this.requests.delete(request.id);
    this.publish();
  }
  seed(receipt: SourceSnapshot) {
    if (this.retired || this.providerReceipt) return;
    const ownership = receipt.ownership;
    if (!ownership || !["live_verified", "credential_bound", "saved_desktop"].includes(ownership.state)) {
      this.backendError = receipt.data.state === "withheld" ? receipt.data.reason : "The saved snapshot's account could not be verified. Refresh to verify it.";
      this.publish();
      return;
    }
    if (receipt.data.state !== "available") {
      if (receipt.data.state === "withheld") this.backendError = receipt.data.reason;
      this.publish();
      return;
    }
    this.coverage = receipt.data.coverage;
    this.retained = { staleSecs: receipt.data.stale_secs, fetchedAt: receipt.data.fetched_at,
      savedOwner: ownership.state === "saved_desktop" ? ownership.owner : undefined };
    // Disk replay is readable inventory, never a fresh detail/action observation.
    const rows = receipt.data.prs.map(pr => ({ ...pr, observation: {
      ...(pr.observation ?? { last_observed_at: null, unknown_fields: [], retained_fields: [] }),
      state: "retained" as const,
    } }));
    this.publish(rows);
  }
  retire() {
    this.retired = true;
    this.requests.clear();
    this.coverage = undefined;
    this.transportError = null;
    this.backendError = null;
    this.value = { prs: undefined, error: null, modern: false };
    for (const listener of this.listeners) listener();
  }
  dismiss() {
    this.transportError = null;
    this.backendError = null;
    this.publish();
  }
}
