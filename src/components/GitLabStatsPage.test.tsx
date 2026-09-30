import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, it, expect, vi } from "vitest";
import { GitLabStatsPage, GitLabStatsResults } from "./GitLabStatsPage";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { call } from "../api/transport";
import type { GitLabStatsReport } from "../api/gitlabStats";
vi.mock("../api/transport", () => ({ call: vi.fn() }));
function report(complete: boolean): GitLabStatsReport {
  return {
    source: { provider: "gitlab", host: "gitlab.com" }, viewer: "1", scope: { kind: "mine" },
    start: "2026-09-01T00:00:00Z", end: "2026-09-02T00:00:00Z", fetched_at: "2026-09-02T00:00:00Z",
    coverage: { complete, stop: complete ? "complete" : "rate_limited", pages: 1, received: 1, total: 2, rate_remaining: 0, rate_reset: null },
    counts: { created: 1, merged: 1, closed: 0, opened: 0 }, series: [{ day: "2026-09-01", created: 1 }],
    authors: [{ username: "author", created: 1, merged: 1, mean_merge_hours: 24, timed_merges: 1 }],
    reviewers: [{ username: "reviewer", assigned: 1 }], reviewer_rows_measured: 1, review_activity: null, history: [],
  };
}
describe("GitLab statistics evidence", () => {
  it("qualifies partial counts and suppresses a biased mean", () => {
    render(<GitLabStatsResults report={report(false)} />);
    expect(screen.getByRole("status").textContent).toContain("rate limited");
    expect(screen.getAllByText("At least 0")).toHaveLength(2);
    expect(screen.queryByText(/24.0 hours/)).toBeNull();
    expect(screen.getByText("Unavailable")).toBeTruthy();
    expect(screen.getByText(/Comment activity and response times unavailable/)).toBeTruthy();
  });
  it("renders a measured zero and a fully measured mean only for complete coverage", () => {
    render(<GitLabStatsResults report={report(true)} />);
    expect(within(screen.getByLabelText("MR counts")).getAllByText("0")).toHaveLength(2);
    expect(screen.getByText("24.0 hours (1 MRs)")).toBeTruthy();
    expect(screen.queryByText("At least 0")).toBeNull();
  });
  it("withholds the mean when merged timestamps are missing", () => {
    const data = report(true); data.authors[0].timed_merges = 0; data.authors[0].mean_merge_hours = null;
    render(<GitLabStatsResults report={data} />);
    expect(screen.getByText("Unavailable")).toBeTruthy();
  });
});

function mountPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><GitLabStatsPage /></QueryClientProvider>);
  return client;
}
function tree(viewer: string, projects: string[], complete = true) {
  return { source: { provider: "gitlab", host: "gitlab.com" }, viewer,
    projects: projects.map(path => ({ path, namespace: "" })),
    coverage: { ...report(complete).coverage, complete } };
}
function accountReport(viewer: string) {
  const result = report(true);
  result.viewer = viewer;
  result.authors[0].username = `author-${viewer}`;
  return result;
}
describe("GitLab statistics account refresh", () => {
  beforeEach(() => vi.mocked(call).mockReset());

  it("loads personal statistics when project discovery times out before page one", async () => {
    vi.mocked(call).mockImplementation(async (name) => {
      if (name === "gitlab_stats_tree") {
        const partial = tree("1", [], false);
        partial.coverage = { complete: false, stop: "timeout", pages: 0, received: 0, total: null, rate_remaining: null, rate_reset: null };
        return partial;
      }
      return accountReport("1");
    });
    mountPage();
    await screen.findByText("author-1");
    expect(screen.getByText(/Project discovery is partial \(timeout\)/)).toBeTruthy();
    expect(screen.getByRole("option", { name: "My authored MRs" })).toBeTruthy();
    expect(screen.queryByRole("option", { name: /^Project:/ })).toBeNull();
    const loads = vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load");
    expect(loads).toHaveLength(1);
    expect(loads[0][1]?.scope).toEqual({ kind: "mine" });
  });

  it("rediscovers partial scopes and refreshes the same account's statistics", async () => {
    let discovery = 0;
    vi.mocked(call).mockImplementation(async (name) => {
      if (name === "gitlab_stats_tree") {
        discovery += 1;
        return tree("1", discovery === 1 ? ["one/project"] : ["one/project", "two/project"], discovery > 1);
      }
      return accountReport("1");
    });
    mountPage();
    await screen.findByText("author-1");
    expect(screen.getByText(/Project discovery is partial/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByRole("option", { name: "Project: two/project" });
    await screen.findByText("author-1");
    expect(screen.queryByText(/Project discovery is partial/)).toBeNull();
    expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_tree")).toHaveLength(2);
    expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load")).toHaveLength(2);
  });

  it("clears account A's scopes and selected project when Refresh discovers account B", async () => {
    let viewer = "1";
    vi.mocked(call).mockImplementation(async (name) => name === "gitlab_stats_tree"
      ? tree(viewer, [`account-${viewer}/project`]) : accountReport(viewer));
    mountPage();
    await screen.findByText("author-1");
    fireEvent.change(screen.getByLabelText("GitLab statistics scope"), { target: { value: JSON.stringify({ kind: "project", path: "account-1/project" }) } });
    await waitFor(() => expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load")).toHaveLength(2));
    viewer = "2";
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("author-2");
    expect(screen.queryByText("author-1")).toBeNull();
    expect(screen.queryByRole("option", { name: "Project: account-1/project" })).toBeNull();
    expect(screen.getByRole("option", { name: "Project: account-2/project" })).toBeTruthy();
    expect((screen.getByLabelText("GitLab statistics scope") as HTMLSelectElement).value).toBe(JSON.stringify({ kind: "mine" }));
    const loads = vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load");
    expect(loads.at(-1)?.[1]?.scope).toEqual({ kind: "mine" });
  });

  it("rejects a mid-request account switch without caching B under A and recovers through discovery", async () => {
    let discoveredViewer = "1";
    vi.mocked(call).mockImplementation(async (name) => name === "gitlab_stats_tree"
      ? tree(discoveredViewer, [`account-${discoveredViewer}/project`]) : accountReport("2"));
    const client = mountPage();
    await screen.findByRole("alert");
    expect(screen.getByRole("alert").textContent).toContain("account changed");
    expect(screen.queryByText("author-2")).toBeNull();
    expect(client.getQueryData(["stats", "gitlab", "gitlab.com", "1", { kind: "mine" }, 30])).toBeUndefined();
    discoveredViewer = "2";
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("author-2");
    expect(client.getQueryData<GitLabStatsReport>(["stats", "gitlab", "gitlab.com", "2", { kind: "mine" }, 30])?.viewer).toBe("2");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("GitLab merged and participation coverage", () => {
  it("keeps merged window counts separate and suppresses partial means", () => {
    const data = report(true);
    data.merged_window = { fetched_at: data.fetched_at, coverage: { ...data.coverage, complete: false, stop: "page_limit" }, count: 7, series: [{ day: "2026-09-01", merged: 7 }], authors: [{ username: "other-author", merged: 7, mean_merge_hours: 99 }], history: [] };
    render(<GitLabStatsResults report={data} />);
    expect(screen.getByText("At least 7 merged MRs")).toBeTruthy();
    expect(screen.queryByText(/99.0 hours/)).toBeNull();
    expect(screen.getByText("24.0 hours (1 MRs)")).toBeTruthy();
  });
  it("withholds absent comments and incomplete response latency", () => {
    const data = report(true);
    data.activity = { complete: false, mrs_checked: 0, mrs_total: 1, comments: null, participants: [], mean_first_response_hours: 3, responded_mrs: 1, failures: ["GitLab rate limit reached; try again later"], rate_remaining: 0, rate_reset: null };
    render(<GitLabStatsResults report={data} />);
    expect(screen.getByText(/Unavailable comments/)).toBeTruthy();
    expect(screen.queryByText(/3.0 hours/)).toBeNull();
    expect(screen.getByText(/Comment read requests remaining: 0/)).toBeTruthy();
  });
  it("qualifies partial formal outcomes and never labels approval as first review", () => {
    const data = report(true);
    data.review_evidence = {
      mrs_total: 2, approvals_checked: 1, changes_checked: 0,
      approvals_complete: false, changes_complete: false,
      current_approvals: 1, current_change_requests: null,
      mean_first_current_approval_hours: null, timed_approved_mrs: 1,
      reviewers: [{ username: "alice", approvals: 1, change_requests: 0 }],
      failures: ["GitLab rate limit reached; try again later"], rate_remaining: 0, rate_reset: null,
    };
    render(<GitLabStatsResults report={data} />);
    expect(screen.getAllByText(/At least 1 current approvals/)).toHaveLength(2);
    expect(screen.getAllByText(/Unavailable current change requests/)).toHaveLength(2);
    expect(screen.queryByText(/At least 0 current change requests/)).toBeNull();
    expect(screen.getByText(/First formal review: Unavailable/)).toBeTruthy();
  });
  it("keeps per-person approvals unavailable when only change requests were measured", () => {
    const data = report(true);
    data.review_evidence = {
      mrs_total: 1, approvals_checked: 0, changes_checked: 1,
      approvals_complete: false, changes_complete: true,
      current_approvals: null, current_change_requests: 1,
      mean_first_current_approval_hours: null, timed_approved_mrs: 0,
      reviewers: [{ username: "alice", approvals: 0, change_requests: 1 }],
      failures: ["GitLab denied access to this statistics scope"], rate_remaining: null, rate_reset: null,
    };
    render(<GitLabStatsResults report={data} />);
    expect(screen.getAllByText(/Unavailable current approvals/)).toHaveLength(2);
    expect(screen.queryByText(/At least 0 current approvals/)).toBeNull();
    expect(screen.getByText("alice: Unavailable current approvals; 1 current change requests")).toBeTruthy();
  });
});

describe("GitLab explicit history", () => {
  beforeEach(() => vi.mocked(call).mockReset());
  it("spends only after clicking and resets the receipt when scope changes", async () => {
    vi.mocked(call).mockImplementation(async name => name === "gitlab_stats_tree" ? tree("1", ["group/project"]) : name === "gitlab_stats_backfill" ? { source: { provider: "gitlab", host: "gitlab.com" }, viewer: "1", scope: { kind: "mine" }, requested_days: 30, attempted_days: 1, complete_days: 1, slices: [], error: null } : accountReport("1"));
    mountPage(); await screen.findByText("author-1");
    expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_backfill")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Load next history day" }));
    await screen.findByText(/1 of 30 closed days/);
    fireEvent.change(screen.getByLabelText("GitLab statistics scope"), { target: { value: JSON.stringify({ kind: "project", path: "group/project" }) } });
    await waitFor(() => expect(screen.queryByText(/1 of 30 closed days/)).toBeNull());
  });
  it("rejects account changes in the history reply", async () => {
    vi.mocked(call).mockImplementation(async name => name === "gitlab_stats_tree" ? tree("1", []) : name === "gitlab_stats_backfill" ? { source: { provider: "gitlab", host: "gitlab.com" }, viewer: "2" } : accountReport("1"));
    mountPage(); await screen.findByText("author-1");
    fireEvent.click(screen.getByRole("button", { name: "Load next history day" }));
    expect((await screen.findByRole("alert")).textContent).toContain("account changed");
  });
});

it("reuses fresh statistics across remounts and forces only explicit Refresh", async () => {
  vi.mocked(call).mockReset();
  vi.mocked(call).mockImplementation(async (name) => name === "gitlab_stats_tree" ? tree("1", []) : accountReport("1"));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const page = <QueryClientProvider client={client}><GitLabStatsPage /></QueryClientProvider>;
  const first = render(page);
  await screen.findByText("author-1");
  expect(vi.mocked(call).mock.calls.find(([name]) => name === "gitlab_stats_load")?.[1]?.refresh).toBe(false);
  first.unmount();
  render(page);
  await screen.findByText("author-1");
  expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load")).toHaveLength(1);
  expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_tree")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load")).toHaveLength(2));
  expect(vi.mocked(call).mock.calls.filter(([name]) => name === "gitlab_stats_load").at(-1)?.[1]?.refresh).toBe(true);
});
