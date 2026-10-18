import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useClaudeEventProfile, useClaudeSessionEvents, useClaudeTranscriptSearch } from "./hooks";
import { SessionTrouble } from "../components/ClaudeCodePage";
import { TroubleProfileCard } from "../components/ClaudeOverviewPage";
import { ClaudeTranscriptSearch } from "../components/ClaudeTranscriptSearch";

const ipc = vi.hoisted(() => ({ search: vi.fn(), coverage: vi.fn(), session: vi.fn(), profile: vi.fn() }));
const events = vi.hoisted(() => ({ delay: false, resolve: undefined as (() => void) | undefined, handlers: new Set<() => void>(), stop: vi.fn(), listen: vi.fn() }));
vi.mock("./transport", async (original) => ({ ...await original<object>(), listen: (name: string, handler: () => void) => {
  events.listen(name); events.handlers.add(handler);
  const stop = () => { events.handlers.delete(handler); events.stop(); };
  return events.delay ? new Promise<() => void>(resolve => { events.resolve = () => resolve(stop); }) : Promise.resolve(stop);
} }));
vi.mock("./tauri", async (original) => ({ ...await original<object>(), claudeSearchTranscripts: ipc.search, claudeIndexCoverage: ipc.coverage, claudeSessionEvents: ipc.session, claudeEventProfile: ipc.profile }));
const cov = { indexed: 1, total: 2, unreadable: [], truncated: 0, last_indexed_at: "2026-10-17" };
const partial = { coverage: cov, verdict: { kind: "none_yet", indexed: 1, total: 2 } };
const hit = { coverage: { ...cov, indexed: 2 }, verdict: { kind: "matches", hits: [{ session_id: "synthetic", snippet: "new committed hit", truncated: false }] } };
function setup() {
  focusManager.setFocused(true);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { client, wrapper };
}
async function receipt() { await act(async () => { for (const handler of events.handlers) { handler(); handler(); handler(); } }); }
afterEach(() => { cleanup(); focusManager.setFocused(undefined); vi.clearAllMocks(); events.handlers.clear(); events.delay = false; events.resolve = undefined; });
it("refreshes mounted search hits and coherent coverage, resubmits trimmed text, and never reads blank", async () => {
  const { wrapper } = setup(); ipc.coverage.mockResolvedValue(cov); ipc.search.mockResolvedValue(partial);
  render(<ClaudeTranscriptSearch />, { wrapper });
  fireEvent.click(screen.getByRole("button", { name: "Search" })); expect(ipc.search).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: " needle " } });
  fireEvent.click(screen.getByRole("button", { name: "Search" }));
  await screen.findByText(/No matches in the 1 of 2/);
  ipc.search.mockResolvedValue(hit); await receipt();
  await screen.findByText("new committed hit"); expect(screen.getByText("All 2 sessions are searchable.")).toBeTruthy();
  expect(ipc.search).toHaveBeenCalledTimes(2);
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "needle" } });
  fireEvent.click(screen.getByRole("button", { name: "Search" })); await waitFor(() => expect(ipc.search).toHaveBeenCalledTimes(3));
  ipc.search.mockRejectedValue(new Error("synthetic read failed")); await receipt();
  await screen.findByText(/Showing the previous search result/); expect(screen.getByText("new committed hit")).toBeTruthy();
});
it("coalesces receipts during a read into one follow-up without cancellation", async () => {
  const { wrapper } = setup(); let settle!: (value: unknown) => void;
  ipc.search.mockImplementationOnce(() => new Promise(resolve => { settle = resolve; })).mockResolvedValue(hit);
  const view = renderHook(() => useClaudeTranscriptSearch("needle"), { wrapper });
  await waitFor(() => expect(ipc.search).toHaveBeenCalledTimes(1)); await receipt();
  expect(ipc.search).toHaveBeenCalledTimes(1);
  await act(async () => { settle(partial); });
  await waitFor(() => expect(view.result.current.data).toEqual(hit)); expect(ipc.search).toHaveBeenCalledTimes(2);
});
it("shares session/profile subscriptions, marks hidden caches stale, and catches listener gaps including old reads", async () => {
  const { client, wrapper } = setup(); ipc.profile.mockResolvedValue({ version: 1 }); ipc.session.mockResolvedValue({ version: 1 });
  const view = renderHook(() => [useClaudeEventProfile(), useClaudeSessionEvents("synthetic")], { wrapper });
  await waitFor(() => expect(view.result.current[0].data).toEqual({ version: 1 })); expect(events.listen).toHaveBeenCalledTimes(1);
  focusManager.setFocused(false); await receipt(); expect(ipc.profile).toHaveBeenCalledTimes(1);
  expect(client.getQueryState(["claude-event-profile"])?.isInvalidated).toBe(true);
  ipc.profile.mockResolvedValue({ version: 2 }); await act(async () => { focusManager.setFocused(true); });
  await waitFor(() => expect(view.result.current[0].data).toEqual({ version: 2 }));
  view.unmount(); expect(events.stop).toHaveBeenCalledOnce();
  let settle!: (value: unknown) => void; ipc.profile.mockImplementationOnce(() => new Promise(resolve => { settle = resolve; })).mockResolvedValue({ version: 3 });
  const oldRead = client.fetchQuery({ queryKey: ["claude-event-profile"], queryFn: ipc.profile, staleTime: 0 });
  const next = renderHook(() => useClaudeEventProfile(), { wrapper });
  await act(async () => { settle({ version: 2 }); await oldRead; });
  await waitFor(() => expect(next.result.current.data).toEqual({ version: 3 }));
});

it("updates actual trouble consumers from unobserved through changing tallies and retains evidence on failure", async () => {
  const { wrapper } = setup();
  ipc.session.mockResolvedValue({ state: "unobserved" });
  ipc.profile.mockResolvedValue({ observation: { state: "unobserved" }, sessions: 2, sessions_observed: 0, sessions_with_events: 0 });
  render(<><SessionTrouble sessionId="synthetic" /><TroubleProfileCard /></>, { wrapper });
  await waitFor(() => expect(screen.getAllByText(/Not recorded/)).toHaveLength(2));
  const observed = (count: number) => ({ state: "observed", profile: { turn_failures: [{ name: "synthetic_failure", count }], tool_failures: [], denials: [] } });
  ipc.session.mockResolvedValue(observed(2));
  ipc.profile.mockResolvedValue({ observation: observed(2), sessions: 2, sessions_observed: 2, sessions_with_events: 1 });
  await receipt(); await waitFor(() => expect(screen.getAllByText("synthetic_failure")).toHaveLength(2));
  ipc.session.mockResolvedValue(observed(7));
  ipc.profile.mockResolvedValue({ observation: observed(7), sessions: 2, sessions_observed: 2, sessions_with_events: 1 });
  await receipt(); await waitFor(() => expect(screen.getAllByText("7")).toHaveLength(2));
  ipc.session.mockRejectedValue(new Error("synthetic failure")); ipc.profile.mockRejectedValue(new Error("synthetic failure"));
  await receipt(); await waitFor(() => expect(screen.getAllByText(/Showing the previous record/)).toHaveLength(2));
  expect(screen.getAllByText("7")).toHaveLength(2);
});
it("isolates caches, leaves disabled and inactive queries unread, and cleans pending work", async () => {
  const { client, wrapper } = setup(); ipc.profile.mockResolvedValue({ version: 1 });
  const other = new QueryClient(); other.setQueryData(["claude-event-profile"], { version: 9 });
  client.setQueryData(["claude-session-events", "inactive"], { state: "unobserved" });
  client.setQueryData(["unrelated-provider"], { version: 1 });
  const view = renderHook(() => [useClaudeEventProfile(), useClaudeTranscriptSearch(""), useClaudeTranscriptSearch("disabled", false), useClaudeSessionEvents(null)], { wrapper });
  await waitFor(() => expect(ipc.profile).toHaveBeenCalledTimes(1));
  await receipt(); expect(ipc.search).not.toHaveBeenCalled(); expect(ipc.session).not.toHaveBeenCalled();
  expect(client.getQueryState(["claude-session-events", "inactive"])?.isInvalidated).toBe(true);
  expect(client.getQueryState(["unrelated-provider"])?.isInvalidated).toBe(false);
  expect(other.getQueryState(["claude-event-profile"])?.isInvalidated).toBe(false);
  const calls = ipc.profile.mock.calls.length;
  act(() => { for (const handler of events.handlers) handler(); view.unmount(); });
  await act(async () => {}); expect(ipc.profile).toHaveBeenCalledTimes(calls); expect(events.handlers.size).toBe(0);
});

it("keeps a receipt stale after an in-flight read settles while hidden, then reads once on focus", async () => {
  const { client, wrapper } = setup(); let settle!: (value: unknown) => void;
  ipc.profile.mockImplementationOnce(() => new Promise(resolve => { settle = resolve; })).mockResolvedValue({ version: 2 });
  const view = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(ipc.profile).toHaveBeenCalledTimes(1));
  focusManager.setFocused(false); await receipt();
  await act(async () => { settle({ version: 1 }); });
  expect(client.getQueryState(["claude-event-profile"])?.isInvalidated).toBe(true);
  expect(ipc.profile).toHaveBeenCalledTimes(1);
  await act(async () => { focusManager.setFocused(true); });
  await waitFor(() => expect(view.result.current.data).toEqual({ version: 2 }));
  expect(ipc.profile).toHaveBeenCalledTimes(2);
});
it("invalidates an Infinity cache after a zero-subscriber gap even when the old read settled during the gap", async () => {
  const { client, wrapper } = setup(); let settle!: (value: unknown) => void;
  ipc.profile.mockImplementationOnce(() => new Promise(resolve => { settle = resolve; })).mockResolvedValue({ version: 2 });
  const first = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(ipc.profile).toHaveBeenCalledTimes(1)); first.unmount();
  await act(async () => { settle({ version: 1 }); });
  expect(client.getQueryData(["claude-event-profile"])).toEqual({ version: 1 });
  const next = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(next.result.current.data).toEqual({ version: 2 }));
  expect(ipc.profile).toHaveBeenCalledTimes(2);
});

it("follows a failed in-flight read once after receipts, without a retry loop", async () => {
  const { wrapper } = setup(); let reject!: (error: Error) => void;
  ipc.profile.mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; })).mockResolvedValue({ version: 2 });
  const view = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(ipc.profile).toHaveBeenCalledTimes(1)); await receipt();
  await act(async () => { reject(new Error("synthetic failure")); });
  await waitFor(() => expect(view.result.current.data).toEqual({ version: 2 }));
  expect(ipc.profile).toHaveBeenCalledTimes(2);
});
it("disposes a listener that resolves after the final subscriber unmounts", async () => {
  const { wrapper } = setup(); events.delay = true; ipc.profile.mockResolvedValue({ version: 1 });
  const view = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(ipc.profile).toHaveBeenCalledTimes(1)); view.unmount();
  await receipt(); expect(ipc.profile).toHaveBeenCalledTimes(1);
  await act(async () => { events.resolve?.(); }); expect(events.stop).toHaveBeenCalledOnce();
  expect(events.handlers.size).toBe(0);
});
it("does not repeat a manual read that already consumes a queued receipt", async () => {
  const { wrapper } = setup(); ipc.profile.mockResolvedValue({ version: 1 });
  const view = renderHook(() => useClaudeEventProfile(), { wrapper });
  await waitFor(() => expect(view.result.current.data).toEqual({ version: 1 }));
  ipc.profile.mockResolvedValue({ version: 2 });
  await act(async () => { for (const handler of events.handlers) handler(); await view.result.current.refetch(); });
  await waitFor(() => expect(view.result.current.data).toEqual({ version: 2 }));
  expect(ipc.profile).toHaveBeenCalledTimes(2);
});
