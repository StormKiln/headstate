import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { GitLabCapabilities, GitLabDetail } from "@/types/gitlabActions";
import { stubViewport } from "@/test-utils";
import { prKey } from "@/lib/prIdentity";
const invoke = vi.hoisted(() => vi.fn<(name: string) => Promise<unknown>>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
// Claude launch discovery is separate from provider detail and permissions.
vi.mock("./GitLabClaudify", () => ({ GitLabClaudify: () => null }));
import { GitLabSummary } from "./SourceQueue";
const identity = { source: { provider: "gitlab" as const, host: "gitlab.example" }, repo: "octocat/hello-world", number: 7 };
const detail: GitLabDetail = {
  core: { identity, id: 7, title: "Synthetic MR", url: "https://gitlab.example/octocat/hello-world/-/merge_requests/7", state: "opened", is_draft: false, body: "Retained GitLab description", author: "author", head_ref: "topic", head_oid: "head", base_ref: "main", detailed_merge_status: "mergeable", blocking_discussions_resolved: true },
  pipelines: { pipelines: { state: "available", value: { items: [], total: 0, coverage: "complete" } }, current_head_jobs: null },
  approvals: { state: "unavailable", issue: "forbidden" }, approval_rules: { state: "unavailable", issue: "forbidden" },
  comments: { state: "available", value: { items: [], total: 0, coverage: "complete" } },
  discussions: { state: "available", value: { discussions: { items: [], total: 0, coverage: "complete" }, unresolved_resolvable: 0 } },
};
const capabilities: GitLabCapabilities = { identity, head_oid: "head", actions: [{ action: "approve", allowed: true, reason: null }], discussions: [], discussions_complete: true };
let qc: QueryClient;
let fail = false;
beforeEach(() => {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } }); fail = false;
  invoke.mockImplementation(async name => {
    if (name === "get_gitlab_detail") { if (fail) throw new Error("Synthetic refresh unavailable"); return detail; }
    if (name === "gitlab_action_capabilities") return capabilities;
    throw new Error(`Unexpected command: ${name}`);
  });
});
afterEach(() => { cleanup(); qc.clear(); stubViewport(null); });
it.each([1200, 390])("retains the real GitLab summary/detail and gates stale actions at width %s", async width => {
  stubViewport(width);
  render(<QueryClientProvider client={qc}><GitLabSummary identity={identity} onBack={() => {}} /></QueryClientProvider>);
  await screen.findByText("Retained GitLab description");
  await screen.findByRole("button", { name: "Approve" });
  fail = true;
  await act(async () => { await qc.refetchQueries({ queryKey: ["gitlab-detail", prKey(identity)] }); });
  expect(screen.getByText("Retained GitLab description")).toBeTruthy();
  expect(screen.getByText("Synthetic MR")).toBeTruthy();
  await screen.findByText(/Detail refresh failed/);
  expect(screen.getByRole("link", { name: /gitlab/i })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "Refresh details" }));
  await waitFor(() => expect(screen.queryByText(/Detail refresh failed/)).toBeNull());
  expect(screen.getByRole("button", { name: "Approve" })).toBeTruthy();
});
