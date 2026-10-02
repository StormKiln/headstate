import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail, PullRequest, PusherAsk, RowPusher } from "@/types/pr";
import { stubViewport } from "@/test-utils";
import publications from "../../src-tauri/tests/fixtures/review-reliability-publications.json";

const boundary = vi.hoisted(() => ({
  invoke: vi.fn<(name: string, args?: Record<string, unknown>) => Promise<unknown>>(),
  listeners: new Map<string, Set<(event: { payload: unknown }) => void>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: boundary.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, callback: (event: { payload: unknown }) => void) => {
  const callbacks = boundary.listeners.get(name) ?? new Set();
  callbacks.add(callback); boundary.listeners.set(name, callbacks);
  return () => { callbacks.delete(callback); };
} }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
// Only unrelated local integrations are replaced. Inventory, event handling,
// advisory demand, detail, viewer and review operation hooks are production.
vi.mock("../api/hooks", async original => ({ ...await original<object>(),
  useWorktrees: () => ({ repos: [], unreadable: [], isError: false }),
  useUiPrefs: () => ({ prefs: { terminal_command: "" } }),
  useClaudeSessionsForPr: () => ({ state: "done", links: [], elsewhere: [] }),
  useReviewGates: () => ({ data: undefined }),
}));
import { useReviewing } from "@/api/hooks";
import { remoteEventError } from "@/api/wireContract";
import { ReadyStrip } from "./ReadyStrip";
import { PrDetailView } from "./PrDetailView";

const clients: QueryClient[] = [];
function Observer({ name }: { name: string }) {
  const inventory = useReviewing(false);
  const [selected, select] = useState<PullRequest | null>(null);
  return <section data-testid={name}><ReadyStrip prs={inventory.data ?? []} onOpen={select} />
    {selected && <PrDetailView repo={selected.repo} number={selected.number} onBack={() => select(null)} />}
  </section>;
}
function detail(head = "head-1"): PrDetail {
  return { id: "PR_1", number: 1, title: "Synthetic review 1", repo: "octocat/repo-1",
    url: "https://github.com/octocat/repo-1/pull/1", state: "open", is_draft: false,
    author: "synthetic-author", body: "Retained integrated description", head_ref: "topic", base_ref: "main",
    head_oid: head, head_ref_id: null, merge_status: "clean", review: "none", latest_reviews: [],
    merge_queue_enabled: false, in_merge_queue: false, additions: 1, deletions: 0, changed_files: 1,
    unresolved_threads: 0, comment_count: 0, comments: [], review_threads: [], review_threads_total: 0,
    checks: [{ name: "Integrated check", state: "success", url: "", run_id: null }], checks_total: 1 };
}
async function emit(stage: keyof typeof publications.frames) {
  const payload = publications.frames[stage];
  expect(remoteEventError("source-poll-status", payload)).toBeNull();
  await act(async () => { for (const callback of boundary.listeners.get("source-poll-status") ?? []) callback({ payload }); });
}
afterEach(() => { cleanup(); for (const client of clients.splice(0)) client.clear(); boundary.listeners.clear(); vi.clearAllMocks(); stubViewport(null); });

it.each([1200, 390])("stitches publication payloads into independent Ready/detail observers at width %s", async width => {
  stubViewport(width);
  let failedDetail = false;
  let head = "head-1";
  boundary.invoke.mockImplementation(async (name, args) => {
    if (name === "get_viewer") return "synthetic-viewer";
    if (name === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, valid_for_ms: 60_000, stack: { kind: "none" } }));
    if (name === "get_ready_pushers") return (args?.rows as PusherAsk[]).map(row => ({ ...row, rules: { state: "unreadable", reason: "Synthetic rules unavailable" }, last_pusher: { state: "unknown", reason: "Synthetic pusher unavailable" } } satisfies RowPusher));
    if (name === "get_pr_detail") { if (failedDetail) throw new Error("Synthetic read outage"); return detail(head); }
    if (name === "review_pr_at_head") return { outcome: "acknowledged", receipt: { review_id: "REVIEW_1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "head-1", submitted_at: "2026-10-01T12:00:00Z", pr_id: "PR_1", repo: "octocat/repo-1", number: 1 } };
    if (name === "refresh_now") throw new Error("Synthetic read outage");
    throw new Error(`Unexpected IPC command ${name}`);
  });
  for (const name of ["desktop", "companion"]) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
    render(<QueryClientProvider client={client}><Observer name={name} /></QueryClientProvider>);
  }
  await waitFor(() => expect(boundary.listeners.get("source-poll-status")?.size).toBe(2));
  await emit("complete");
  expect(screen.getAllByRole("heading", { name: /Ready for review \(20\)/ })).toHaveLength(2);
  await emit("large");
  expect(screen.getAllByRole("heading", { name: /Ready for review \(275\)/ })).toHaveLength(2);
  await emit("partial");
  expect(screen.queryByText("Synthetic review 276")).toBeNull();
  expect(screen.getAllByText("Last known — not confirmed by latest refresh").length).toBeGreaterThan(0);
  const desktop = within(screen.getByTestId("desktop"));
  fireEvent.click(desktop.getByText("Synthetic review 1"));
  await desktop.findByText("Retained integrated description");
  failedDetail = true;
  await act(async () => { await clients[0].refetchQueries({ queryKey: ["pr-detail", "octocat/repo-1", 1] }); });
  await emit("cooldown");
  expect(screen.getAllByRole("heading", { name: /Ready for review \(275\)/ })).toHaveLength(2);
  expect(desktop.getByText("Retained integrated description")).toBeTruthy();
  expect(desktop.getByRole("alert")).toBeTruthy();
  fireEvent.click(desktop.getAllByRole("button", { name: "Approve" })[0]);
  await waitFor(() => expect(boundary.invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1));
  expect(boundary.invoke.mock.calls.find(([name]) => name === "review_pr_at_head")?.[1]).toMatchObject({ request: { expected_head: "head-1", expected_viewer: "synthetic-viewer" } });
  await emit("acknowledged");
  await emit("confirmed");
  await emit("recovered");
  expect(screen.getAllByRole("heading", { name: /Ready for review \(274\)/ })).toHaveLength(2);
  expect(desktop.getAllByRole("button", { name: "Approved" }).length).toBeGreaterThan(0);
  failedDetail = false;
  await act(async () => { await clients[0].refetchQueries({ queryKey: ["pr-detail", "octocat/repo-1", 1] }); });
  expect(desktop.getAllByRole("button", { name: "Approved" }).length).toBeGreaterThan(0);
  head = "head-1-new";
  await emit("changed_head");
  await act(async () => { await clients[0].refetchQueries({ queryKey: ["pr-detail", "octocat/repo-1", 1] }); });
  expect(screen.getAllByRole("heading", { name: /Ready for review \(275\)/ })).toHaveLength(2);
  expect(desktop.getAllByRole("button", { name: "Approve" }).some(button => !button.matches(":disabled"))).toBe(true);
  expect(boundary.invoke.mock.calls.filter(([name]) => name === "review_pr_at_head")).toHaveLength(1);
  // Another provider source must not clear either GitHub inventory.
  const foreign = { ...publications.frames.changed_head, source: { provider: "github", host: "other.synthetic.invalid" }, revision: 999, receipt_revision: 999, prs: [] };
  expect(remoteEventError("source-poll-status", foreign)).toBeNull();
  await act(async () => { for (const callback of boundary.listeners.get("source-poll-status") ?? []) callback({ payload: foreign }); });
  expect(screen.getAllByRole("heading", { name: /Ready for review \(275\)/ })).toHaveLength(2);
  const advisory = boundary.invoke.mock.calls.filter(([name]) => name === "get_ready_stacks" || name === "get_ready_pushers");
  expect(advisory.length).toBeGreaterThan(0);
  expect(advisory.every(([, args]) => (args?.rows as object[]).length <= 8)).toBe(true);
  expect(advisory.reduce((sum, [, args]) => sum + (args?.rows as object[]).length, 0)).toBeLessThan(275);
  expect(publications.cycle_http_attempts).toHaveLength(6);
  expect(publications.cycle_http_attempts.every(count => count <= 3)).toBe(true);
});
