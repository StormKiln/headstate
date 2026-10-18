import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { focusManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { SourcePollUpdate } from "./tauri";
import type { MergeRequest } from "../types/gitlab";
import type { GitLabCapabilities, GitLabDetail } from "../types/gitlabActions";
import { GitLabViewerProvider } from "./authAvailability";
import { useGitLabQueue } from "./gitlabQueues";
import { GitLabSummary } from "../components/SourceQueue";
import { prKey } from "../lib/prIdentity";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), listeners: new Map<string, Set<(e: { payload: unknown }) => void>>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, cb) => { const set = ipc.listeners.get(name) ?? new Set(); set.add(cb); ipc.listeners.set(name, set); return () => set.delete(cb); }) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../components/GitLabClaudify", () => ({ GitLabClaudify: () => null }));
const identity = { source: { provider: "gitlab" as const, host: "gitlab.example" }, repo: "team/project", number: 7 };
const row: MergeRequest = { ...identity, viewer: "alice", id: 7, title: "Old title", url: "https://gitlab.example/team/project/-/merge_requests/7", author: "author", is_draft: false, head_ref: "topic", head_oid: "old-head", base_ref: "main", created_at: "2026-01-01", updated_at: "2026-01-01", labels: [], reviewers: [], assignees: [], comment_count: 0, detailed_merge_status: "mergeable", ci: null, review: null, unresolved_threads: null };
function detail(title = "Old title", head = "old-head", draft = false, state = "opened"): GitLabDetail {
  return { viewer: "alice", core: { identity, id: 7, title, head_oid: head, state, is_draft: draft, body: "Retained description", url: row.url, author: "author", head_ref: "topic", base_ref: "main", detailed_merge_status: "mergeable", blocking_discussions_resolved: true }, pipelines: { pipelines: { state: "available", value: { items: [], total: 0, coverage: "complete" } }, current_head_jobs: null }, approvals: { state: "unavailable", issue: "forbidden" }, approval_rules: { state: "unavailable", issue: "forbidden" }, comments: { state: "available", value: { items: [], total: 0, coverage: "complete" } }, discussions: { state: "available", value: { discussions: { items: [], total: 0, coverage: "complete" }, unresolved_resolvable: 0 } } };
}
const caps = (head = "old-head"): GitLabCapabilities => ({ viewer: "alice", identity, head_oid: head, actions: [{ action: "comment", allowed: true, reason: null }], discussions: [], discussions_complete: true });
let client: QueryClient;
let next: GitLabDetail;
let fail: boolean;
let revision: number;
let pending: Promise<GitLabDetail> | undefined;
function Queues() { useGitLabQueue("authored", true, "gitlab.example"); useGitLabQueue("authored", true, "gitlab.example"); useGitLabQueue("reviewing", true, "gitlab.example"); return null; }
function Tree({ viewer = "alice" }: { viewer?: string }) { return <QueryClientProvider client={client}><GitLabViewerProvider viewer={viewer}><Queues /><GitLabSummary identity={identity} onBack={() => {}} /></GitLabViewerProvider></QueryClientProvider>; }
function mount() { return render(<Tree />); }
async function receipt(mrs = [row], extra: Partial<SourcePollUpdate> = {}) {
  const frame = { source: identity.source, list: "authored", owner: "alice", phase: "ready", error: null, session: "desktop", revision: ++revision, receipt_revision: revision, completed_request: null, last_received_at: new Date().toISOString(), coverage: "complete", mrs, ...extra };
  await act(async () => { for (const cb of ipc.listeners.get("source-poll-status") ?? []) cb({ payload: frame }); });
}
const reads = () => ipc.invoke.mock.calls.filter(([name]) => name === "get_gitlab_detail").length;
beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } }); next = detail(); fail = false; revision = 0; pending = undefined; ipc.listeners.clear(); ipc.invoke.mockReset();
  client.setQueryData(["gitlab-detail", prKey(identity), "alice"], next);
  client.setQueryData(["gitlab-actions", prKey(identity), "alice"], caps());
  ipc.invoke.mockImplementation(async (name: string) => {
    if (name === "get_source_snapshot") return { source: identity.source, list: "authored", data: { state: "git_lab_available", mrs: [row], coverage: "complete", fetched_at: new Date().toISOString(), stale_secs: null } };
    if (name === "get_gitlab_detail") { if (fail) throw new Error("Synthetic readback failed"); return pending ?? next; }
    if (name === "gitlab_action_capabilities") return caps(next.core.head_oid!);
    throw new Error(`Unexpected ${name}`);
  });
});
afterEach(() => { cleanup(); client.clear(); focusManager.setFocused(undefined); });
it("reconciles initial live facts with cached mounted detail and preserves comment and expansion", async () => {
  mount(); await screen.findByText("Old title");
  const comment = screen.getByRole("textbox", { name: "MR comment" }); fireEvent.change(comment, { target: { value: "unfinished draft" } }); comment.focus();
  const disclosure = screen.getByText("Action availability").closest("details")!; disclosure.open = true;
  next = detail("New title", "new-head", true);
  await receipt([{ ...row, title: "New title", head_oid: "new-head", is_draft: true }]);
  await screen.findByText("New title"); expect(screen.getByText(/Head new-head/)).toBeTruthy();
  expect((comment as HTMLTextAreaElement).value).toBe("unfinished draft"); expect(document.activeElement).toBe(comment); expect(disclosure.open).toBe(true); expect(reads()).toBe(1);
  await receipt([{ ...row, title: "New title", head_oid: "new-head", is_draft: true }]);
  await receipt([{ ...row, title: "New title", head_oid: "new-head", is_draft: true }], { list: "reviewing" });
  expect(reads()).toBe(1);
});
it("rejects replayed, retained, wrong host and wrong owner evidence", async () => {
  mount(); await screen.findByText("Old title"); await receipt(); const baseline = reads();
  for (const extra of [{ receipt_revision: 1 }, { owner: "bob" }, { source: { provider: "gitlab" as const, host: "elsewhere" } }]) await receipt([{ ...row, title: "Wrong" }], extra);
  await receipt([{ ...row, title: "Retained", observation: { state: "retained", last_observed_at: null, unknown_fields: [], retained_fields: [] } }]);
  expect(reads()).toBe(baseline); expect(screen.getByText("Old title")).toBeTruthy();
});
it("only owner-proven complete absence revalidates and reads terminal state rather than guessing", async () => {
  mount(); await screen.findByText("Old title"); await receipt(); const baseline = reads(); next = detail("Closed title", "old-head", false, "closed");
  await receipt([], { coverage: { partial: { total: 10 } } }); await receipt([], { owner: "bob" }); await receipt([], { owner: undefined }); expect(reads()).toBe(baseline);
  await receipt([]); await screen.findByText("Closed title"); expect(screen.getByText(/closed · Head/)).toBeTruthy(); expect(reads()).toBe(baseline + 1);
  await receipt([]); expect(reads()).toBe(baseline + 1);
});
it("coalesces receipts during a read into one follow-up and retains useful content on failure", async () => {
  mount(); await screen.findByText("Old title"); let finish!: (d: GitLabDetail) => void; pending = new Promise(resolve => { finish = resolve; });
  await receipt([{ ...row, title: "Middle" }]); await waitFor(() => expect(reads()).toBe(1));
  next = detail("Latest"); await receipt([{ ...row, title: "Later" }]); await receipt([{ ...row, title: "Latest" }]); expect(reads()).toBe(1);
  pending = undefined; await act(async () => finish(detail("Middle"))); await screen.findByText("Latest"); expect(reads()).toBe(2);
  fail = true; await receipt([{ ...row, title: "Failed" }]); await screen.findByText(/Detail refresh failed/); expect(screen.getByText("Latest")).toBeTruthy(); expect(screen.getByText("Retained description")).toBeTruthy();
});
it("does not interpret unknown or retained field groups as changed facts", async () => {
  mount(); await screen.findByText("Old title"); await receipt();
  await receipt([{ ...row, head_oid: "unknown-head", is_draft: true, detailed_merge_status: "blocked", observation: { state: "observed", last_observed_at: null, unknown_fields: ["head", "merge"], retained_fields: ["draft"] } }]);
  expect(reads()).toBe(0); expect(screen.getByText(/Head old-head/)).toBeTruthy();
});
it("reconciles a late receipt independently of desktop and webview clocks", async () => {
  mount(); await screen.findByText("Old title"); next = detail("Authoritative title");
  await receipt([{ ...row, title: "Late queue" }], { last_received_at: "2020-01-01T00:00:00Z" });
  await screen.findByText("Authoritative title"); expect(reads()).toBe(1);
});
it("accepts positive legacy row ownership but rejects a conflicting owner envelope", async () => {
  mount(); await screen.findByText("Old title"); next = detail("Legacy title");
  await receipt([{ ...row, title: "Legacy title" }], { owner: "bob" }); expect(reads()).toBe(0);
  await receipt([{ ...row, title: "Legacy title" }], { owner: undefined }); await screen.findByText("Legacy title"); expect(reads()).toBe(1);
});
it("refreshes from a correlated command reply without a transport event", async () => {
  mount(); await screen.findByText("Old title"); next = detail("Reply title");
  const original = ipc.invoke.getMockImplementation()!;
  ipc.invoke.mockImplementation(async (name: string, args: { requestId?: string }) => name === "refresh_source" ? { request_id: args.requestId, update: { source: identity.source, list: "authored", owner: "alice", phase: "ready", error: null, session: "desktop", revision: 1, receipt_revision: 1, completed_request: args.requestId, last_received_at: new Date().toISOString(), coverage: "complete", mrs: [{ ...row, title: "Reply title" }] } } : original(name, args));
  await act(async () => { for (const cb of ipc.listeners.get("refresh-requested") ?? []) cb({ payload: null }); });
  await screen.findByText("Reply title"); expect(reads()).toBe(1);
});
it("marks hidden detail stale without reads and reconciles once when focused", async () => {
  mount(); await screen.findByText("Old title"); act(() => focusManager.setFocused(false)); next = detail("Visible again");
  await receipt([{ ...row, title: "Visible again" }]); expect(reads()).toBe(0); expect(screen.getByText("Old title")).toBeTruthy();
  await act(async () => focusManager.setFocused(true)); await screen.findByText("Visible again"); expect(reads()).toBe(1);
});
it("marks an inactive cached identity stale without fetching it", async () => {
  const other = { ...identity, number: 8 }; client.setQueryData(["gitlab-detail", prKey(other), "alice"], { ...detail(), core: { ...detail().core, identity: other } });
  mount(); await screen.findByText("Old title"); await receipt([{ ...row, number: 8, title: "Other changed" }, row]);
  expect(reads()).toBe(0); expect(client.getQueryState(["gitlab-detail", prKey(other), "alice"])?.isInvalidated).toBe(true);
});
it("does not refresh other-owner keys when the accepted owner changes facts", async () => {
  const key = ["gitlab-detail", prKey(identity), "bob"]; client.setQueryData(key, { ...detail(), viewer: "bob" });
  mount(); await screen.findByText("Old title"); next = detail("Alice changed"); await receipt([{ ...row, title: "Alice changed" }]);
  await screen.findByText("Alice changed"); expect(client.getQueryState(key)?.isInvalidated).toBe(false);
});
it("does not oscillate readbacks when independent lists have different membership", async () => {
  mount(); await screen.findByText("Old title"); await receipt(); await receipt([], { list: "reviewing" }); const count = reads();
  await receipt(); await receipt([], { list: "reviewing" }); await receipt(); expect(reads()).toBe(count);
});
it("keeps a focused draft mounted while head readback is pending and after failure", async () => {
  mount(); await screen.findByText("Old title"); const input = screen.getByRole("textbox", { name: "MR comment" }); fireEvent.change(input, { target: { value: "Keep me" } }); input.focus();
  let reject!: (reason: Error) => void; pending = new Promise((_, failRead) => { reject = failRead; }); next = detail("Next", "new-head");
  await receipt([{ ...row, head_oid: "new-head" }]); await waitFor(() => expect(reads()).toBe(1));
  expect(screen.getByRole("textbox", { name: "MR comment" })).toBe(input); expect(document.activeElement).toBe(input);
  await act(async () => reject(new Error("Synthetic delayed failure"))); await screen.findByText(/Detail refresh failed/);
  expect(screen.getByRole("textbox", { name: "MR comment" })).toBe(input); expect((input as HTMLTextAreaElement).value).toBe("Keep me"); expect(document.activeElement).toBe(input);
  expect((screen.getByRole("button", { name: "Post comment" }) as HTMLButtonElement).disabled).toBe(true);
});
it.each([{ ci: "failure" as const }, { review: "changes_requested" as const }, { in_merge_queue: true }])("revalidates newly known capability facts %j", async patch => {
  mount(); await screen.findByText("Old title"); next = detail("Fresh capability evidence"); await receipt([{ ...row, ...patch }]);
  await screen.findByText("Fresh capability evidence"); expect(reads()).toBe(1);
});
it("keeps a discussion reply draft mounted while the detail head is being reconciled", async () => {
  const d = detail(); d.discussions = { state: "available", value: { discussions: { items: [{ id: "thread", individual_note: false, notes: [{ comment: { id: 9, author: "author", body: "Synthetic thread", created_at: null, system: false }, kind: null, resolvable: true, resolved: false }] }], total: 1, coverage: "complete" }, unresolved_resolvable: 1 } };
  const c = caps(); c.actions.push({ action: "reply", allowed: true, reason: null }); c.discussions.push({ id: "thread", can_reply: true, can_resolve: false, resolved: false });
  client.setQueryData(["gitlab-detail", prKey(identity), "alice"], d); client.setQueryData(["gitlab-actions", prKey(identity), "alice"], c);
  const original = ipc.invoke.getMockImplementation()!; ipc.invoke.mockImplementation((name: string, args: unknown) => name === "gitlab_action_capabilities" ? Promise.resolve({ ...c, head_oid: "new-head" }) : original(name, args));
  mount(); const input = await screen.findByRole("textbox", { name: "Discussion reply" }); fireEvent.change(input, { target: { value: "Reply draft" } }); input.focus();
  let finish!: (d: GitLabDetail) => void; pending = new Promise(resolve => { finish = resolve; });
  await receipt([{ ...row, head_oid: "new-head" }]); await screen.findByText(/Action permissions and MR head/); expect(screen.getByRole("textbox", { name: "Discussion reply" })).toBe(input); expect(document.activeElement).toBe(input);
  await act(async () => finish({ ...d, core: { ...d.core, head_oid: "new-head" } }));
  expect((screen.getByRole("textbox", { name: "Discussion reply" }) as HTMLTextAreaElement).value).toBe("Reply draft"); expect(document.activeElement).toBe(input);
});
it("does not let an old-account command reply refresh the new-account cache", async () => {
  const bobKey = ["gitlab-detail", prKey(identity), "bob"]; client.setQueryData(bobKey, { ...detail("Bob title"), viewer: "bob" }); client.setQueryData(["gitlab-actions", prKey(identity), "bob"], { ...caps(), viewer: "bob" });
  const view = mount(); await screen.findByText("Old title"); let finish!: (reply: unknown) => void; const reply = new Promise(resolve => { finish = resolve; }); const original = ipc.invoke.getMockImplementation()!;
  ipc.invoke.mockImplementation((name: string, args: unknown) => name === "refresh_source" ? reply : original(name, args));
  await act(async () => { for (const cb of ipc.listeners.get("refresh-requested") ?? []) cb({ payload: null }); });
  view.rerender(<Tree viewer="bob" />); await screen.findByText("Bob title");
  await act(async () => finish({ request_id: "old", update: { source: identity.source, list: "authored", owner: "alice", phase: "ready", error: null, session: "desktop", revision: 3, receipt_revision: 3, completed_request: null, last_received_at: new Date().toISOString(), coverage: "complete", mrs: [{ ...row, title: "Alice changed" }] } }));
  expect(screen.getByText("Bob title")).toBeTruthy(); expect(client.getQueryState(bobKey)?.isInvalidated).toBe(false); expect(reads()).toBe(0);
});
it("does not issue pending follow-up work after consumers unmount", async () => {
  const view = mount(); await screen.findByText("Old title"); let finish!: (d: GitLabDetail) => void; pending = new Promise(resolve => { finish = resolve; });
  await receipt([{ ...row, title: "Middle" }]); await receipt([{ ...row, title: "Latest" }]); expect(reads()).toBe(1);
  view.unmount(); await act(async () => finish(detail("Middle"))); expect(reads()).toBe(1);
  expect([...ipc.listeners.values()].every(set => set.size === 0)).toBe(true);
});
