import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { useReadyStacks } from "@/api/useReadyStacks";
import { PrActions } from "./PrActions";
import type { PrDetail, PrStack } from "@/types/pr";
const invoke = vi.hoisted(() => vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
afterEach(cleanup);
const detail: PrDetail = {
  id: "synthetic-stack", number: 30, title: "Synthetic layer", url: "https://github.com/octocat/hello-world/pull/30",
  state: "open", is_draft: false, body: "", author: "octocat", repo: "octocat/hello-world", head_ref: "feature", head_oid: "synthetic-head", head_ref_id: null,
  base_ref: "main", merge_status: "clean", review: "approved", additions: 1, deletions: 0, changed_files: 1,
  unresolved_threads: 0, comment_count: 0, comments: [], review_threads: [], review_threads_total: 0, latest_reviews: [], merge_queue_enabled: false, in_merge_queue: false, checks: [], checks_total: 0,
};
const native: PrStack = { kind: "stacked", native: true, stack_number: 7, position: 1, size: 1, position_exact: true, size_exact: true, below: null,
  members_complete: true, members: [{ position: 1, number: 30, title: "Synthetic layer", state: "open" }] };
function Consumer({ pr }: { pr: PrDetail }) {
  const stack = useReadyStacks([pr]).of(pr);
  return <><span>{stack?.kind ?? "pending"}</span><PrActions pr={{ ...pr, stack: stack ?? { kind: "unknown" } }} /></>;
}
it("reuses matching warm selected-detail evidence and keeps singleton native StackMerge routing", async () => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["viewer"], "octocat");
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_ready_stacks") return (args?.rows as object[]).map(row => ({ ...row, stack: native }));
    if (command === "merge_stack") return { kind: "enqueued" };
    return null;
  });
  const view = render(<QueryClientProvider client={qc}><Consumer pr={detail} /></QueryClientProvider>);
  await screen.findByText("stacked");
  view.rerender(<QueryClientProvider client={qc}><span>away</span></QueryClientProvider>);
  view.rerender(<QueryClientProvider client={qc}><Consumer pr={detail} /></QueryClientProvider>);
  expect(screen.getByText("stacked")).toBeTruthy();
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "get_ready_stacks")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Merge" }));
  const dialog = screen.getByRole("dialog");
  fireEvent.click(within(dialog).getByRole("button", { name: "Merge 1 pull request" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("merge_stack", expect.objectContaining({ expectedHead: "synthetic-head" })));
  expect(invoke.mock.calls.some(([cmd]) => cmd === "act_on_pr")).toBe(false);
  view.unmount(); qc.clear();
});
