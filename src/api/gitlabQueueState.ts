import type { MergeRequest } from "../types/gitlab";
import type { SourceCoverage, SourceList, SourcePollUpdate, SourceRefreshReply, SourceSnapshot } from "./tauri";

export type GitLabQueueSnapshot = {
  rows: MergeRequest[] | undefined;
  coverage: SourceCoverage | null;
  staleSecs: number | null | "unknown";
  loading: boolean;
  refreshing: boolean;
  error: string | null;
};

type Request = { id: string; order: number; session: string | undefined; completed: boolean };

/// The cache, desktop events, and IPC replies are independent connections.
/// Only desktop receipt revisions can replace rows, and only newer status
/// revisions can change the provider error. A late reply therefore cannot
/// erase a failure (or a later receipt) already observed through events.
export class GitLabQueueState {
  constructor(private readonly expectedHost?: string, private readonly expectedList?: SourceList, private readonly viewer?: string | null) {}
  private value: GitLabQueueSnapshot = {
    rows: undefined, coverage: null, staleSecs: null,
    loading: true, refreshing: false, error: null,
  };
  private session: string | undefined;
  private retired = new Set<string>();
  private revision = -1;
  private receiptRevision = -1;
  private receivedAt: string | null | undefined;
  private providerError: string | null = null;
  private transportError: { id: string; message: string } | null = null;
  private order = 0;
  private requests = new Map<string, Request>();

  snapshot = () => this.value;

  private publish(patch: Partial<GitLabQueueSnapshot> = {}) {
    this.value = {
      ...this.value,
      ...patch,
      refreshing: this.requests.size > 0,
      error: this.transportError?.message ?? this.providerError,
    };
  }

  tick() {
    if (this.receivedAt !== undefined) {
      const staleSecs = receiptAge(this.receivedAt);
      if (staleSecs !== this.value.staleSecs) this.publish({ staleSecs });
    }
  }

  seed(data: SourceSnapshot["data"]) {
    // Cache is a fallback only. It cannot overwrite a live event that arrived
    // during the disk read, including a measured empty live list.
    if (this.value.rows !== undefined) return;
    if (data.state === "git_lab_available") {
      if (this.viewer !== undefined && (this.viewer === null || data.mrs.some(row => row.viewer !== this.viewer))) return;
      this.receivedAt = data.fetched_at;
      this.publish({ rows: data.mrs, coverage: data.coverage, staleSecs: data.stale_secs, loading: false });
    } else if (data.state === "unreadable") {
      this.providerError = "The saved GitLab list could not be read.";
      this.publish({ loading: false });
    }
  }

  start(id: string): Request {
    const request = { id, order: ++this.order, session: this.session, completed: false };
    this.requests.set(id, request);
    this.publish();
    return request;
  }

  accept(update: SourcePollUpdate) {
    if (this.viewer !== undefined && (this.viewer === null || update.mrs?.some(row => row.viewer !== this.viewer))) return;
    if (this.expectedHost !== undefined && (update.source.provider !== "gitlab" || update.source.host !== this.expectedHost || update.list !== this.expectedList)) return;
    if (this.retired.has(update.session)) return;
    if (this.session !== update.session) {
      if (this.session !== undefined) this.retired.add(this.session);
      this.session = update.session;
      this.revision = -1;
      this.receiptRevision = -1;
    }
    const completed = update.completed_request ? this.requests.get(update.completed_request) : undefined;
    if (completed) {
      completed.completed = true;
      this.requests.delete(completed.id);
      if (this.transportError?.id === completed.id) this.transportError = null;
    }
    if (update.revision > this.revision) {
      this.revision = update.revision;
      this.providerError = update.error;
    }
    if (update.mrs !== null && update.receipt_revision !== null && update.receipt_revision > this.receiptRevision) {
      this.receiptRevision = update.receipt_revision;
      this.receivedAt = update.last_received_at;
      this.publish({ rows: update.mrs, coverage: update.coverage, staleSecs: receiptAge(this.receivedAt), loading: false });
    } else {
      // A failed/fetching event often repeats the last saved `mrs`. Its
      // presence is not evidence that those rows were fetched again. Recheck
      // the age of the accepted receipt without adopting this event's time.
      const staleSecs = this.receivedAt === undefined ? this.value.staleSecs : receiptAge(this.receivedAt);
      this.publish(update.phase === "fetching" ? { staleSecs } : { loading: false, staleSecs });
    }
  }

  resolve(request: Request, reply: SourceRefreshReply) {
    if ("update" in reply && (this.session === undefined || reply.update.session === this.session || request.session === this.session)) {
      this.accept(reply.update);
    }
    // Legacy desktops return rows without a revision. They cannot safely
    // supersede any versioned receipt or terminal status already observed.
    if (this.viewer === undefined && "mrs" in reply && this.session === undefined && this.value.rows === undefined && reply.mrs !== null) {
      this.publish({ rows: reply.mrs, coverage: reply.coverage, staleSecs: null, loading: false });
    }
    this.requests.delete(request.id);
    if (request.order === this.order) this.transportError = null;
    this.publish({ loading: false });
  }

  reject(request: Request, error: unknown) {
    if (!request.completed && request.order === this.order) {
      this.transportError = {
        id: request.id,
        message: typeof error === "string" ? error : error instanceof Error ? error.message : "Refresh failed",
      };
    }
    this.requests.delete(request.id);
    this.publish({ loading: false });
  }
}

/// SQLite marks a snapshot stale after one hour. An event carrying rows
/// saved two hours ago must not turn that marker into "fresh" simply because
/// the event arrived now.
export function receiptAge(receivedAt: string | null): number | null | "unknown" {
  if (receivedAt === null) return "unknown";
  // SQLite cache timestamps are UTC without a zone marker; live poll
  // timestamps are RFC 3339. Treat both as UTC across desktop timezones.
  const normalized = /^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$/.test(receivedAt)
    ? `${receivedAt.replace(" ", "T")}Z` : receivedAt;
  const milliseconds = Date.parse(normalized);
  if (!Number.isFinite(milliseconds)) return "unknown";
  const seconds = Math.max(0, Math.floor((Date.now() - milliseconds) / 1000));
  return seconds > 3600 ? seconds : null;
}
