import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { PrDetail, PullRequest } from "@/types/pr";
import { PR_FIXTURES } from "@/fixtures/prs";
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
  useReviewGates: () => ({ data: undefined }),
}));
import { useSourceRefresh } from "@/api/sourceRefreshHooks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetailView } from "./PrDetailView";
function Selected() { useSourceRefresh("reviewing"); return <PrDetailView repo="octocat/repo-1" number={1} onBack={() => {}} />; }
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach(qc => qc.clear()); vi.clearAllMocks(); boundary.listener = undefined; stubViewport(null); });
function detail(head = "head-1"): PrDetail {
  return { id: "PR_1", number: 1, title: "Synthetic review 1", repo: "octocat/repo-1",
    url: "https://github.com/octocat/repo-1/pull/1", state: "open", is_draft: false,
    author: "synthetic-author", body: "Retained integrated description", head_ref: "topic", base_ref: "main",
    head_oid: head, head_ref_id: null, merge_status: "clean", review: "none", latest_reviews: [],
    merge_queue_enabled: false, in_merge_queue: false, additions: 1, deletions: 0, changed_files: 1,
    unresolved_threads: 0, comment_count: 2, comments: [1, 2].map(number => ({
      id: `comment-${number}`, author: `synthetic-commenter-${number}`, created_at: "2026-10-01T12:00:00Z", author_is_bot: false,
      body: `<details><summary>Open report</summary>${head} open content</details><details><summary>Closed report</summary>${head} closed content</details>`,
    })), review_threads: [], review_threads_total: 0,
    checks: [{ name: "Integrated check", state: "success", url: "", run_id: null }], checks_total: 1 };
}

it.each([1200, 390])("preserves full content, disclosure nodes and external navigation through automatic refresh and failure at width %s", async width => {
  stubViewport(width);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(qc);
  const key = ["pr-detail", "octocat/repo-1", 1];
  qc.setQueryData(key, detail()); qc.setQueryData(["viewer"], "synthetic-viewer");
  let reject!: (error: Error) => void;
  let finish!: (value: PrDetail) => void;
  let calls = 0;
  boundary.invoke.mockImplementation((name: string) => {
    if (name === "get_pr_detail") { calls++; return new Promise((resolve, fail) => { finish = resolve; reject = fail; }); }
    return Promise.resolve();
  });
  render(<QueryClientProvider client={qc}><Selected /></QueryClientProvider>);
  const comment = screen.getByText("synthetic-commenter-1").closest("button")!;
  fireEvent.click(comment);
  const disclosures = [...document.querySelectorAll("details")];
  fireEvent.click(disclosures[0].querySelector("summary")!);
  expect(disclosures.map(node => node.open)).toEqual([true, false]);
  const row: PullRequest = { ...PR_FIXTURES[0], repo: "octocat/repo-1", number: 1, head_oid: "head-2", base_ref: "main", merge_status: "clean", review: "none", observation: { state: "observed", last_observed_at: null, unknown_fields: [], retained_fields: [] } };
  const publish = async (revision: number, head = "head-2") => act(async () => boundary.listener?.({ payload: { source: { provider: "github", host: "github.com" }, list: "reviewing", phase: "ready", error: null, session: "session", revision, receipt_revision: revision, prs: [{ ...row, head_oid: head }], coverage: "complete" } }));
  await publish(1);
  await waitFor(() => expect(calls).toBe(1));
  expect(screen.getByText("Retained integrated description")).toBeTruthy();
  expect(comment.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelectorAll("details")[0]).toBe(disclosures[0]);
  await act(async () => reject(new Error("Synthetic detail unavailable")));
  await screen.findByRole("alert");
  expect(screen.getByText("Retained integrated description")).toBeTruthy();
  expect(disclosures.map(node => node.open)).toEqual([true, false]);
  fireEvent.click(screen.getByRole("link", { name: /^GitHub$/ }));
  await waitFor(() => expect(openUrl).toHaveBeenCalledWith("https://github.com/octocat/repo-1/pull/1"));
  // A new positive target gets one automatic read; the failed h2 target is
  // separately covered under the real backoff clock in the hook regression.
  await publish(2, "head-3");
  await waitFor(() => expect(calls).toBe(2));
  await act(async () => finish(detail("head-3")));
  await waitFor(() => expect(qc.getQueryData<PrDetail>(key)?.head_oid).toBe("head-3"));
  expect(document.querySelectorAll("details")[0]).toBe(disclosures[0]);
  expect(disclosures.map(node => node.open)).toEqual([true, false]);
  expect(disclosures[0].textContent).toContain("head-3 open content");
  expect(screen.getAllByRole("button", { name: "Approve" }).some(button => !button.matches(":disabled"))).toBe(true);
  await publish(3, "head-3"); expect(calls).toBe(2);
});
