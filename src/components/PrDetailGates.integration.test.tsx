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
