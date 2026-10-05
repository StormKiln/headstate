import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import { CourtStrip } from "../components/CourtStrip";
import * as hooks from "./sourceRefreshHooks";
import { listen } from "./transport";
import { refreshSource } from "./tauri";
vi.mock("./transport", () => ({ listen: vi.fn(async () => () => {}), call: vi.fn(async () => undefined) }));
vi.mock("./tauri", () => ({ refreshSource: vi.fn(), getCached: vi.fn(), getSourceSnapshot: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
it.each(["authored", "reviewing"] as const)("retires %s receipts across unmount and pairing reset and fences pending calls", async list => {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const first = renderHook(() => hooks.useSourceRefresh(list), { wrapper });
  await act(async () => {});
  const callback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  act(() => callback({ payload: { source: { provider: "github", host: "github.com" }, list, session: "desktop-a", revision: 1, receipt_revision: 1, phase: "ready", error: null, prs: PR_FIXTURES, coverage: "complete" } } as never));
  expect(first.result.current.prs).toEqual(PR_FIXTURES);
  let finish!: (value: never) => void;
  vi.mocked(refreshSource).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const pending = hooks.refreshWithState(qc, list).catch(() => undefined);
  first.unmount();
  await act(async () => { hooks.retireSourceOwnership(qc); await qc.resetQueries(); });
  const next = renderHook(() => hooks.useSourceRefresh(list), { wrapper });
  expect(next.result.current.prs).toBeUndefined();
  expect(next.result.current.coverage).toBeUndefined();
  await act(async () => { finish(PR_FIXTURES as never); await pending; });
  expect(qc.getQueryData([list === "authored" ? "prs" : "reviewing"])).toBeUndefined();
  act(() => callback({ payload: { source: { provider: "github", host: "github.com" }, list, session: "desktop-a", revision: 2, receipt_revision: 2, phase: "ready", error: null, prs: PR_FIXTURES } } as never));
  expect(next.result.current.prs).toBeUndefined();
});
it("retains yesterday's 150 rows and measured empty without live/detail authority", async () => {
  const { getSourceSnapshot } = await import("./tauri");
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  for (const rows of [Array.from({ length: 150 }, (_, i) => ({ ...PR_FIXTURES[0], number: i + 1 })), []]) {
    const at = "2026-01-01T00:00:00Z";
    vi.mocked(getSourceSnapshot).mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list: "authored", ownership: { state: "credential_bound", owner: "alice" }, data: { state: "available", prs: rows, fetched_at: at, stale_secs: 86400, coverage: "complete" } } as never);
    await act(async () => { await hooks.readAuthored(qc); });
    expect(hook.result.current.prs).toHaveLength(rows.length);
    expect(hook.result.current.staleSecs).toBe(86400);
    expect(hook.result.current.fetchedAt).toBe(at);
    expect(hook.result.current.prs?.every(row => row.observation?.state === "retained")).toBe(true);
    expect(refreshSource).not.toHaveBeenCalled();
  }
});
it("a late disk receipt never replaces an accepted live receipt", async () => {
  const { getSourceSnapshot } = await import("./tauri");
  const qc = new QueryClient();
  let finish!: (value: never) => void;
  vi.mocked(getSourceSnapshot).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const reading = hooks.readAuthored(qc);
  vi.mocked(refreshSource).mockResolvedValueOnce(PR_FIXTURES.slice(1));
  await hooks.refreshWithState(qc, "authored");
  finish({ source: { provider: "github", host: "github.com" }, list: "authored", ownership: { state: "credential_bound", owner: "alice" }, data: { state: "available", prs: PR_FIXTURES, fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400, coverage: "complete" } } as never);
  expect(await reading).toEqual(PR_FIXTURES.slice(1));
});
it("a verified account change retires both lists and old detail authority", async () => {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => ({ authored: hooks.useSourceRefresh("authored"), reviewing: hooks.useSourceRefresh("reviewing") }), { wrapper });
  await act(async () => {});
  const callbacks = vi.mocked(listen).mock.calls.filter(c => c[0] === "source-poll-status").map(c => c[1]);
  const frame = (list: string, owner: string, revision: number, prs = PR_FIXTURES) => ({ source: { provider: "github", host: "github.com" }, list, owner, session: "desktop", revision, receipt_revision: revision, prs, phase: "ready", error: null, coverage: "complete" });
  act(() => { for (const fn of callbacks) { fn({ payload: frame("authored", "alice", 1) } as never); fn({ payload: frame("reviewing", "alice", 1) } as never); } });
  expect(hook.result.current.reviewing.prs).toEqual(PR_FIXTURES);
  act(() => { for (const fn of callbacks) fn({ payload: frame("authored", "bob", 2, []) } as never); });
  expect(hook.result.current.authored.prs).toEqual([]);
  expect(hook.result.current.reviewing.prs).toBeUndefined();
  expect(qc.getQueryData(["reviewing"])).toBeUndefined();
});
it("delayed provider identity reopens the retained cache without another live refresh", async () => {
  const { getSourceSnapshot } = await import("./tauri");
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  await act(async () => {});
  vi.mocked(getSourceSnapshot).mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list: "authored", ownership: { state: "live_verified", owner: "alice" }, data: { state: "available", prs: PR_FIXTURES, fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400, coverage: "complete" } } as never);
  const callback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  await act(async () => callback({ payload: { source: { provider: "github", host: "github.com" }, list: "authored", owner: "alice", session: "desktop", revision: 0, receipt_revision: null, prs: null, phase: "not_requested", error: null } } as never));
  expect(hook.result.current.prs).toHaveLength(PR_FIXTURES.length);
  expect(hook.result.current.staleSecs).toBe(86400);
  expect(refreshSource).not.toHaveBeenCalled();
});
it("a new backend session without owner evidence cannot retain old live rows as current", async () => {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  await act(async () => {});
  const callback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  const source = { provider: "github", host: "github.com" };
  act(() => callback({ payload: { source, list: "authored", session: "old-process", owner: "alice", revision: 1, receipt_revision: 1, prs: PR_FIXTURES, phase: "ready", error: null } } as never));
  expect(hook.result.current.prs).toHaveLength(PR_FIXTURES.length);
  act(() => callback({ payload: { source, list: "authored", session: "new-process", revision: 0, receipt_revision: null, prs: null, phase: "not_requested", error: null } } as never));
  expect(hook.result.current.prs).toBeUndefined();
  expect(qc.getQueryData(["prs"])).toBeUndefined();
});
it("rejects a retired owner's late session before it can retire the new account", async () => {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  await act(async () => {});
  const frame = (owner: string, session: string, prs = PR_FIXTURES) => ({ source: { provider: "github", host: "github.com" }, list: "authored", owner, session, revision: 2, receipt_revision: 2, prs, phase: "ready", error: null });
  const publish = (payload: unknown) => { for (const call of vi.mocked(listen).mock.calls.filter(c => c[0] === "source-poll-status")) call[1]({ payload } as never); };
  act(() => publish(frame("alice", "alice-process")));
  act(() => publish(frame("bob", "bob-process", [])));
  expect(hook.result.current.prs).toEqual([]);
  await act(async () => {});
  act(() => publish(frame("alice", "alice-process")));
  expect(hook.result.current.prs).toEqual([]);
});
it("an older desktop's ownerless cached envelope falls back to a fresh provider request", async () => {
  const { getSourceSnapshot } = await import("./tauri");
  const qc = new QueryClient();
  vi.mocked(getSourceSnapshot).mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list: "authored", data: { state: "available", prs: PR_FIXTURES, fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400, coverage: "complete" } });
  vi.mocked(refreshSource).mockResolvedValueOnce(PR_FIXTURES.slice(1));
  expect(await hooks.readAuthored(qc)).toEqual(PR_FIXTURES.slice(1));
});
it("same-owner new process status preserves the qualified retained receipt", async () => {
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  await act(async () => {});
  const callback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  const source = { provider: "github", host: "github.com" };
  act(() => callback({ payload: { source, list: "authored", session: "old-process", owner: "alice", revision: 1, receipt_revision: 1, prs: PR_FIXTURES, phase: "ready", error: null } } as never));
  act(() => callback({ payload: { source, list: "authored", session: "new-process", owner: "alice", revision: 0, receipt_revision: null, prs: null, phase: "not_requested", error: null } } as never));
  expect(hook.result.current.prs).toEqual(PR_FIXTURES);
});
it.each((["authored", "reviewing"] as const).flatMap(list => [undefined, "alice"].map(owner => ({ list, owner }))))("retained $list receipt qualifies new process owner $owner", async ({ list, owner }) => {
  const { getSourceSnapshot } = await import("./tauri");
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh(list), { wrapper });
  await act(async () => {});
  vi.mocked(getSourceSnapshot).mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list, session: "alice-process", ownership: { state: "live_verified", owner: "alice" }, data: { state: "available", prs: PR_FIXTURES, fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400, coverage: "complete" } } as never);
  await act(async () => { await hooks.readRetained(qc, list); });
  expect(hook.result.current.prs).toHaveLength(PR_FIXTURES.length);
  const callback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  act(() => callback({ payload: { source: { provider: "github", host: "github.com" }, list, owner, session: "new-process", revision: 0, receipt_revision: null, prs: null, phase: "not_requested", error: null } } as never));
  if (owner) {
    expect(hook.result.current.prs).toHaveLength(PR_FIXTURES.length);
    expect(hook.result.current.fetchedAt).toBe("2026-01-01T00:00:00Z");
    expect(hook.result.current.prs?.every(row => row.observation?.state === "retained")).toBe(true);
  } else {
    expect(hook.result.current.prs).toBeUndefined();
    expect(hook.result.current.coverage).toBeUndefined();
  }
});
it("same-running-desktop re-pair recovers rows and detail facts while old reads stay retired", async () => {
  const { beginDetailRead, detailReadIsCurrent, detailNeedsRevalidation } = await import("./detailRevalidation");
  const qc = new QueryClient();
  const row = PR_FIXTURES[0];
  qc.setQueryData(["pr-detail", row.repo, row.number], { ...row, head_oid: "before" });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const hook = renderHook(() => hooks.useSourceRefresh("authored"), { wrapper });
  await act(async () => {});
  const frame = (revision: number) => ({ source: { provider: "github", host: "github.com" }, list: "authored", owner: "alice", session: "same-running-desktop", revision, receipt_revision: revision, prs: [{ ...row, head_oid: `head-${revision}` }], phase: "ready", error: null });
  const oldCallback = vi.mocked(listen).mock.calls.find(c => c[0] === "source-poll-status")![1];
  act(() => oldCallback({ payload: frame(1) } as never));
  const oldRead = beginDetailRead(qc, row.repo, row.number);
  await act(async () => { hooks.retireSourceOwnership(qc); await qc.resetQueries(); });
  expect(hook.result.current.prs).toBeUndefined();
  expect(detailReadIsCurrent(qc, oldRead)).toBe(false);
  const { getSourceSnapshot } = await import("./tauri");
  vi.mocked(getSourceSnapshot).mockResolvedValueOnce({ source: { provider: "github", host: "github.com" }, list: "authored", session: "same-running-desktop", ownership: { state: "live_verified", owner: "alice" }, data: { state: "available", prs: [row], fetched_at: "2026-01-01T00:00:00Z", stale_secs: 86400, coverage: "complete" } } as never);
  await act(async () => { await hooks.readRetained(qc, "authored"); });
  expect(hook.result.current.prs).toHaveLength(1);
  expect(hook.result.current.fetchedAt).toBe("2026-01-01T00:00:00Z");
  expect(detailNeedsRevalidation(qc, row.repo, row.number)).toBe(false);
  qc.setQueryData(["pr-detail", row.repo, row.number], { ...row, head_oid: "before" });
  const callback = vi.mocked(listen).mock.calls.filter(c => c[0] === "source-poll-status").at(-1)![1];
  act(() => callback({ payload: frame(2) } as never));
  expect(hook.result.current.prs?.[0].head_oid).toBe("head-2");
  expect(detailNeedsRevalidation(qc, row.repo, row.number)).toBe(true);
  act(() => oldCallback({ payload: frame(3) } as never));
  expect(hook.result.current.prs?.[0].head_oid).toBe("head-2");
});

it("retires the mounted Court all-clear immediately and restores it only after a new complete pair", async () => {
  const qc = new QueryClient();
  function Summary() {
    const authored = hooks.useSourceRefresh("authored"); const reviewing = hooks.useSourceRefresh("reviewing");
    const availability = (snapshot: typeof authored) => ({ status: snapshot.error ? "failed" as const : snapshot.prs === undefined ? "pending" as const : "available" as const, coverage: snapshot.coverage ?? null, retained: snapshot.fetchedAt !== undefined || !!snapshot.staleSecs });
    return <CourtStrip authored={authored.prs ?? []} reviewing={reviewing.prs ?? []} availability={{ authored: availability(authored), reviewing: availability(reviewing) }} onSelect={() => {}} />;
  }
  render(<QueryClientProvider client={qc}><Summary /></QueryClientProvider>);
  await act(async () => {});
  const callbacks = () => vi.mocked(listen).mock.calls.filter(c => c[0] === "source-poll-status").map(c => c[1]);
  const old = callbacks();
  const frame = (list: string, revision: number) => ({ payload: { source: { provider: "github", host: "github.com" }, list, session: "desktop", owner: "synthetic-viewer", revision, receipt_revision: revision, phase: "ready", error: null, prs: [], coverage: "complete" } });
  act(() => { for (const fn of old) for (const list of ["authored", "reviewing"]) fn(frame(list, 1) as never); });
  expect(screen.getByText("Nothing needs your attention")).toBeTruthy();
  act(() => hooks.retireSourceOwnership(qc));
  expect(screen.queryByText("Nothing needs your attention")).toBeNull();
  act(() => { for (const fn of old) for (const list of ["authored", "reviewing"]) fn(frame(list, 2) as never); });
  expect(screen.queryByText("Nothing needs your attention")).toBeNull();
  act(() => { for (const fn of callbacks()) fn(frame("authored", 3) as never); });
  expect(screen.queryByText("Nothing needs your attention")).toBeNull();
  act(() => { for (const fn of callbacks()) fn(frame("reviewing", 3) as never); });
  expect(screen.getByText("Nothing needs your attention")).toBeTruthy();
  expect(refreshSource).not.toHaveBeenCalled(); qc.clear();
});
