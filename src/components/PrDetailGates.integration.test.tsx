import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail } from "@/types/pr";
import { stubViewport } from "@/test-utils";
const boundary = vi.hoisted(() => ({ invoke: vi.fn(), listener: undefined as undefined | ((event: { payload: unknown }) => void) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: boundary.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, cb: (event: { payload: unknown }) => void) => { if (name === "source-poll-status") boundary.listener = cb; return () => {}; } }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
vi.mock("../api/hooks", async original => ({ ...await original<object>(),
  useWorktrees: () => ({ repos: [], unreadable: [], isError: false }),
  useUiPrefs: () => ({ prefs: { terminal_command: "" } }),
  useClaudeSessionsForPr: () => ({ state: "done", links: [], elsewhere: [] }),
}));
import { useSourceRefresh } from "@/api/sourceRefreshHooks";
import { PrDetailView } from "./PrDetailView";
import { useReadyStacks } from "@/api/useReadyStacks";
function Selected() { useSourceRefresh("reviewing"); return <PrDetailView repo="octocat/repo-1" number={1} onBack={() => {}} />; }
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach(qc => qc.clear()); vi.clearAllMocks(); boundary.listener = undefined; stubViewport(null); });
function detail(head = "head-1"): PrDetail {
  return { id: "PR_1", number: 1, title: "Synthetic review 1", repo: "octocat/repo-1",
    url: "https://github.com/octocat/repo-1/pull/1", state: "open", is_draft: false,
    author: "synthetic-author", body: "Retained integrated description", head_ref: "topic", base_ref: "main",
    head_repo: "octocat/repo-1", head_oid: head, head_ref_id: null, merge_status: "clean", review: "none", latest_reviews: [],
    merge_queue_enabled: false, in_merge_queue: false, additions: 1, deletions: 0, changed_files: 1,
    unresolved_threads: 0, comment_count: 2, comments: [1, 2].map(number => ({
      id: `comment-${number}`, author: `synthetic-commenter-${number}`, created_at: "2026-10-01T12:00:00Z", author_is_bot: false,
      body: `<details><summary>Open report</summary>${head} open content</details><details><summary>Closed report</summary>${head} closed content</details>`,
    })), review_threads: [], review_threads_total: 0,
    checks: [{ name: "Integrated check", state: "success", url: "", run_id: null }], checks_total: 1 };
}

it("refreshes failed same-head gates with the primary detail, coalesces clicks and preserves disclosure and draft focus", async () => {
  stubViewport(1200);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(qc);
  qc.setQueryData(["pr-detail", "octocat/repo-1", 1], detail()); qc.setQueryData(["viewer"], "synthetic-viewer");
  let gateCalls = 0;
  let finishGate!: (value: unknown) => void;
  boundary.invoke.mockImplementation(async (name: string) => {
    if (name === "get_pr_detail") return detail();
    if (name === "get_review_gates") {
      gateCalls++;
      if (gateCalls === 1) return { rules: { state: "unreadable", reason: "offline" }, last_pusher: { state: "not_needed" }, rules_valid_for_ms: 5000 };
      return new Promise(resolve => { finishGate = resolve; });
    }
    return undefined;
  });
  render(<QueryClientProvider client={qc}><Selected /></QueryClientProvider>);
  await waitFor(() => expect(gateCalls).toBe(1));
  const comment = screen.getByText("synthetic-commenter-1").closest("button")!;
  fireEvent.click(comment);
  const disclosure = document.querySelector("details")!;
  fireEvent.click(disclosure.querySelector("summary")!);
  const draft = screen.getByPlaceholderText("Leave a comment (required to request changes)…");
  fireEvent.change(draft, { target: { value: "Keep this draft" } }); draft.focus();
  const refresh = screen.getByRole("button", { name: "Refresh" });
  await waitFor(() => expect((refresh as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(refresh); fireEvent.click(refresh);
  await waitFor(() => expect(gateCalls).toBe(2));
  await act(async () => finishGate({ rules: { state: "read", require_last_push_approval: true, required_review_thread_resolution: false }, last_pusher: { state: "known", login: "synthetic-viewer" }, rules_valid_for_ms: 600000, pusher_valid_for_ms: 60000 }));
  await screen.findByText("Won't count toward merging");
  expect(document.querySelector("details")).toBe(disclosure);
  expect(disclosure.open).toBe(true);
  expect((draft as HTMLTextAreaElement).value).toBe("Keep this draft");
  expect(document.activeElement).toBe(draft);
  expect(boundary.invoke.mock.calls.filter(([name]) => name === "get_pr_detail")).toHaveLength(1);
});

it.each([false, true])("Refresh recovers stack evidence without a background tick (after approval: %s)", async approve => {
  stubViewport(1200);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(qc);
  qc.setQueryData(["pr-detail", "octocat/repo-1", 1], detail());
  qc.setQueryData(["viewer"], "synthetic-viewer");
  let stacks = 0;
  let approved = false;
  let finishStack!: (value: unknown) => void;
  boundary.invoke.mockImplementation(async (name: string) => {
    if (name === "review_pr_at_head") {
      approved = true;
      return { outcome: "acknowledged", receipt: { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "head-1", submitted_at: "2026-10-07T12:00:00Z", pr_id: "PR_1", repo: "octocat/repo-1", number: 1 } };
    }
    if (name === "get_pr_detail") return { ...detail(), latest_reviews: approved ? [{ id: "review-1", author: "synthetic-viewer", state: "APPROVED", commit_oid: "head-1", submitted_at: "2026-10-07T12:00:00Z" }] : [] };
    if (name === "get_review_gates") return { rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false }, last_pusher: { state: "not_needed" }, rules_valid_for_ms: 600000 };
    if (name === "get_ready_stacks") {
      stacks++;
      if (stacks === 1) return [{ repo: "octocat/repo-1", number: 1, head_oid: "head-1", base_ref: "main", stack: { kind: "unknown" }, valid_for_ms: 5000 }];
      return new Promise(resolve => { finishStack = resolve; });
    }
    return undefined;
  });
  render(<QueryClientProvider client={qc}><Selected /></QueryClientProvider>);
  await waitFor(() => expect(stacks).toBe(1));
  const refresh = screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement;
  await waitFor(() => expect(refresh.disabled).toBe(false));
  expect(screen.getAllByRole("button", { name: "Merge" }).every(button => (button as HTMLButtonElement).disabled)).toBe(true);
  if (approve) {
    fireEvent.click(screen.getAllByRole("button", { name: "Approve" })[0]);
    await screen.findAllByRole("button", { name: "Approved" });
    await waitFor(() => expect(qc.getQueryData<PrDetail>(["pr-detail", "octocat/repo-1", 1])?.merge_status).toBe("clean"));
  }
  const readsBeforeRefresh = boundary.invoke.mock.calls.filter(([name]) => name === "get_pr_detail").length;
  fireEvent.click(refresh); fireEvent.click(refresh);
  await waitFor(() => expect(stacks).toBe(2));
  await waitFor(() => expect(refresh.disabled).toBe(true));
  expect(screen.getByText(/Cannot merge: Stack membership/)).toBeTruthy();
  await act(async () => finishStack([{ repo: "octocat/repo-1", number: 1, head_oid: "head-1", base_ref: "main", stack: { kind: "none" }, valid_for_ms: 60000 }]));
  await waitFor(() => expect(screen.getAllByRole("button", { name: "Merge" }).every(button => !(button as HTMLButtonElement).disabled)).toBe(true));
  expect(stacks).toBe(2);
  const lines = boundary.invoke.mock.calls.filter(([name]) => name === "diag_log").map(([, args]) => args.line as string);
  expect(lines.some(line => line.startsWith("detail actions ") && line.includes("merge=clean stack=none") && line.includes(`approved=${approve}`))).toBe(true);
  expect(lines.filter(line => line.startsWith("detail actions ")).join("\n")).not.toMatch(/octocat|synthetic|head-1|PR_1/);
  expect(boundary.invoke.mock.calls.filter(([name]) => name === "get_pr_detail")).toHaveLength(readsBeforeRefresh + 1);
});


it.each([false, true])("recovers selected approval and the actual action beside 150 budget-refused strip rows (queue: %s)", async queued => {
  stubViewport(1200);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(qc);
  qc.setQueryData(["viewer"], "synthetic-viewer");
  const subject = { ...detail(), merge_queue_enabled: queued };
  qc.setQueryData(["pr-detail", subject.repo, 1], subject);
  const rows = Array.from({ length: 150 }, (_, i) => ({ ...subject, repo: `octocat/repo-${i + 1}`, number: i + 1, head_oid: `head-${i + 1}` }));
  function Strip() { useReadyStacks(rows); return null; }
  let writes = 0;
  let selected = 0;
  let strips = 0;
  boundary.invoke.mockImplementation(async (name: string, args: Record<string, unknown>) => {
    if (name === "get_ready_stacks") {
      if (args.selected === true) selected++; else strips++;
      return (args.rows as object[]).map(row => ({ ...row, stack: { kind: args.selected ? "none" : "unknown" }, valid_for_ms: args.selected ? 60_000 : 5_000 }));
    }
    if (name === "get_review_gates") return { rules: { state: "read", require_last_push_approval: false, required_review_thread_resolution: false }, last_pusher: { state: "not_needed" }, rules_valid_for_ms: 600000 };
    if (name === "get_pr_detail") return { ...subject, body: "Accepted full readback" };
    if (name === "review_pr_at_head") {
      writes++;
      return { outcome: "acknowledged", receipt: { review_id: "review-1", state: "APPROVED", actor: "synthetic-viewer", commit_oid: "head-1", submitted_at: new Date().toISOString(), pr_id: "PR_1", repo: subject.repo, number: 1 } };
    }
    return undefined;
  });
  const view = render(<QueryClientProvider client={qc}><Strip /></QueryClientProvider>);
  await waitFor(() => expect(strips).toBeGreaterThan(0));
  view.rerender(<QueryClientProvider client={qc}><Strip /><Selected /></QueryClientProvider>);
  await waitFor(() => expect(selected).toBe(1));
  fireEvent.click(screen.getAllByRole("button", { name: "Approve" })[0]);
  await screen.findByText("Accepted full readback");
  await screen.findAllByRole("button", { name: "Approved" });
  const label = queued ? "Add to merge queue" : "Merge";
  await waitFor(() => expect(screen.getAllByRole("button", { name: label }).every(button => !button.matches(":disabled"))).toBe(true));
  expect(writes).toBe(1); expect(selected).toBe(1);
  expect(boundary.invoke.mock.calls.filter(([name]) => name === "get_pr_detail")).toHaveLength(1);
  expect(strips).toBeLessThanOrEqual(16);
});
