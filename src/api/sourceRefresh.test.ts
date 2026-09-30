import { describe, expect, it } from "vitest";
import type { PullRequest } from "../types/pr";
import { receiptAdvisory, SourceRefreshState, type SourceStatus } from "./sourceRefresh";

const rows = (number: number) => [{ number }] as PullRequest[];
const update = (revision: number, error: string | null = null, extra: Partial<SourceStatus> = {}): SourceStatus => ({
  source: { provider: "github", host: "github.com" }, list: "reviewing",
  session: "desktop-1", revision, receipt_revision: revision,
  phase: error ? "failed" : "ready", error, prs: rows(revision), ...extra,
});

describe("correlated source refreshes", () => {
  it("keeps newer event rows/status when an older successful reply arrives", () => {
    const state = new SourceRefreshState();
    const request = state.start("a");
    state.accept(update(4, "newer failure", { receipt_revision: 3, prs: rows(3) }));
    state.resolve(request, { request_id: "a", update: update(2) });
    expect(state.snapshot()).toMatchObject({ prs: rows(3), error: "newer failure" });
  });

  it("keeps newer recovery when a matched backend failure rejects late", () => {
    const state = new SourceRefreshState();
    const request = state.start("a");
    state.accept(update(2, "old provider failure", { completed_request: "a" }));
    state.accept(update(4));
    state.reject(request, "old provider failure");
    expect(state.snapshot()).toMatchObject({ prs: rows(4), error: null });
  });

  it.each(["before", "after"])("keeps transport failure when unrelated background success arrives %s rejection", (order) => {
    const state = new SourceRefreshState();
    state.accept(update(1));
    const request = state.start("phone-b");
    if (order === "before") state.accept(update(2, null, { completed_request: "background-a" }));
    state.reject(request, "desktop unreachable");
    if (order === "after") state.accept(update(2, null, { completed_request: "background-a" }));
    expect(state.snapshot()).toMatchObject({ prs: rows(2), error: "desktop unreachable" });
  });

  it("recovers from missed events using a newer correlated reply", () => {
    const state = new SourceRefreshState();
    state.accept(update(1, "previous provider failure"));
    const request = state.start("b");
    state.resolve(request, { request_id: "b", update: update(3) });
    expect(state.snapshot()).toMatchObject({ prs: rows(3), error: null });
  });

  it("clears an older transport failure when a newer requested attempt completes", () => {
    const state = new SourceRefreshState();
    state.reject(state.start("a"), "offline");
    const request = state.start("b");
    state.accept(update(3, null, { completed_request: "b" }));
    state.reject(request, "reply connection lost");
    expect(state.snapshot().error).toBeNull();
  });

  it("does not let an older local request overwrite a newer transport failure", () => {
    const state = new SourceRefreshState();
    const a = state.start("a");
    state.reject(state.start("b"), "new offline failure");
    state.resolve(a, { request_id: "a", update: update(2) });
    expect(state.snapshot().error).toBe("new offline failure");
  });

  it("orders receipt revisions separately from status revisions, including same-attempt rechecks", () => {
    const state = new SourceRefreshState();
    state.accept(update(4, "latest failed", { receipt_revision: 3, prs: rows(3) }));
    state.accept(update(5, "latest failed"));
    state.accept(update(4));
    expect(state.snapshot()).toMatchObject({ prs: rows(5), error: "latest failed" });
  });

  it("accepts old-desktop array replies without overwriting later legacy rows", () => {
    const state = new SourceRefreshState();
    state.resolve(state.start("a"), rows(1));
    const request = state.start("b");
    state.legacyRows(rows(3));
    state.resolve(request, rows(2));
    expect(state.snapshot()).toMatchObject({ prs: rows(3), error: null, modern: false });
  });

  it("preserves absence when a provider failure has no receipt", () => {
    const state = new SourceRefreshState();
    const request = state.start("a");
    expect(state.resolve(request, { request_id: "a", update: update(2, "provider failure", { prs: null, receipt_revision: null }) })).toBeUndefined();
    expect(state.snapshot()).toMatchObject({ prs: undefined, error: "provider failure" });
  });

  it("keeps a confirmed row patch when the same receipt is replayed", () => {
    const state = new SourceRefreshState();
    state.accept(update(2));
    state.patchRows((prs) => prs.map((pr) => ({ ...pr, in_merge_queue: true })));
    state.accept(update(2));
    expect(state.snapshot().prs?.[0].in_merge_queue).toBe(true);
  });

  it("keeps receipt coverage through newer attempts and only clears warnings for measured completeness", () => {
    const state = new SourceRefreshState();
    expect(receiptAdvisory(state.snapshot(), "total")).toBeUndefined();
    state.accept(update(2, null, { coverage: { partial: { total: 5 } } }));
    expect(receiptAdvisory(state.snapshot(), "total")).toBe(5);
    expect(receiptAdvisory(state.snapshot(), "missing")).toBe(4);
    state.accept(update(3, "new failure", { receipt_revision: 2, coverage: "complete" }));
    expect(receiptAdvisory(state.snapshot(), "missing")).toBe(4);
    state.accept(update(4, null, { coverage: { partial: { total: 1 } } }));
    // A non-short numeric count is not proof that a partial response is whole.
    expect(receiptAdvisory(state.snapshot(), "total")).toBeNull();
    expect(receiptAdvisory(state.snapshot(), "missing")).toBeNull();
    state.accept(update(5, null, { coverage: "complete" }));
    expect(receiptAdvisory(state.snapshot(), "total")).toBe(0);
    expect(receiptAdvisory(state.snapshot(), "missing")).toBe(0);
  });

  it("does not roll back to an old process reply after observing a new process", () => {
    const state = new SourceRefreshState();
    const request = state.start("a");
    state.accept(update(1, null, { session: "desktop-new" }));
    state.resolve(request, { request_id: "a", update: update(99) });
    expect(state.snapshot().prs).toEqual(rows(1));
    state.accept(update(2, null, { session: "desktop-new" }));
    expect(state.snapshot().prs).toEqual(rows(2));
  });
});

 it("retains rows and suppresses retrying errors until failure, then clears on recovery", () => {
    const state = new SourceRefreshState();
    state.accept(update(1));
    state.accept(update(2, "temporary failure", { phase: "retrying", prs: null }));
    expect(state.snapshot()).toMatchObject({ prs: rows(1), error: null });
    state.accept(update(3, "persistent failure", { phase: "failed", prs: null }));
    expect(state.snapshot().error).toBe("persistent failure");
    state.accept(update(4));
    expect(state.snapshot()).toMatchObject({ prs: rows(4), error: null });
  });
