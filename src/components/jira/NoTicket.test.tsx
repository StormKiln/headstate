import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TitleOutcome, UiPrefs } from "@/api/tauri";
import type { GitLabActionRequest, GitLabReceipt } from "@/types/gitlabActions";
import type { IssueBatch, JiraConnection, JiraIssue, JiraLink, LinkIndex } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const assigned = vi.hoisted(() => vi.fn<() => Promise<IssueBatch>>());
const search = vi.hoisted(() => vi.fn<(text: string) => Promise<IssueBatch>>());
const setPrTitle = vi.hoisted(() =>
  vi.fn<(id: string, repo: string, number: number, expected: string, title: string) => Promise<TitleOutcome>>(),
);
const gitLabAction = vi.hoisted(() => vi.fn<(r: GitLabActionRequest) => Promise<GitLabReceipt>>());
vi.mock("@/api/tauri", () => ({
  jiraConnection: connection,
  jiraLinks: links,
  jiraIssues: vi.fn(),
  jiraAssigned: assigned,
  jiraSearch: search,
  setPrTitle,
  gitLabAction,
}));

import { JiraProvider } from "./JiraContext";
import { NoTicketMarker } from "./NoTicket";
import { titleWritePreview, type TitleTarget } from "@/lib/jira/titleWrite";

const prefs = (over: Partial<UiPrefs> = {}): UiPrefs => ({
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: true,
  claude_integrations_enabled: false,
  diagnostic_logging: false,
  terminal_command: "",
  stale_venv_days: 0,
  battery_low_percent: 0,
  jira_enabled: true,
  jira_excluded_projects: [],
  jira_status_expectations: {},
  jira_chips_on_prs: true,
  jira_no_ticket_marker: true,
  ...over,
});

const issue = (key: string): JiraIssue => ({
  key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: "To Do", category: "new" },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: key.split("-")[0],
  url: null,
  updated: null,
});
const batch = (issues: JiraIssue[], over: Partial<IssueBatch> = {}): IssueBatch => ({
  issues,
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
  ...over,
});

const URL = "https://github.com/octocat/hello-world/pull/7";
const ROW = { url: URL, title: "Fix login", head_ref: "fix-login" };
const GITHUB: TitleTarget = { kind: "github", id: "PR_7", repo: "octocat/hello-world", number: 7 };

const authored = (key: string): JiraLink => ({
  key,
  found_in: "title",
  current: true,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
    role: "authored",
    repo: "octocat/hello-world",
    number: 7,
    title: ROW.title,
    url: URL,
    head_ref: ROW.head_ref,
    is_draft: false,
    state: null,
    author: null,
  },
});
const index = (found: JiraLink[], checked: LinkIndex["checked"]): LinkIndex => ({ links: found, projects: ["ABC", "XY"], error: null, checked });
const READ = [{ url: URL, title: ROW.title, head_ref: ROW.head_ref }];

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  links.mockReset().mockResolvedValue(index([], READ));
  assigned.mockReset().mockResolvedValue(batch([issue("ABC-1"), issue("XY-2")]));
  search.mockReset().mockResolvedValue(batch([issue("XY-2")]));
  setPrTitle.mockReset();
  gitLabAction.mockReset();
});
afterEach(cleanup);

const mount = (p: UiPrefs = prefs(), row = ROW, target: TitleTarget = GITHUB) =>
  renderWithQuery(
    <JiraProvider prefs={p}>
      <NoTicketMarker row={row} target={target} />
    </JiraProvider>,
  );

/// Long enough for the provider's two reads to land.
const settle = () => waitFor(() => expect(links).toHaveBeenCalled()).then(() => new Promise((r) => setTimeout(r, 20)));

describe("the no-ticket marker", () => {
  it("marks a pull request the index read, as it is now, with no key", async () => {
    mount();
    expect(await screen.findByRole("button", { name: "no ticket" })).toBeTruthy();
  });

  it("is off by default: nothing is marked while the preference is off", async () => {
    const { container } = mount(prefs({ jira_no_ticket_marker: false }));
    await settle();
    expect(container.textContent).toBe("");
  });

  /// Pending is not "no ticket" (#1042): a pull request the index has not
  /// read, or read under another title, shows nothing.
  it("shows nothing for a pull request the index has not read yet", async () => {
    links.mockResolvedValue(index([], []));
    const { container } = mount();
    await settle();
    expect(container.textContent).toBe("");
  });

  it("shows nothing when the title changed since the index read it", async () => {
    const { container } = mount(prefs(), { ...ROW, title: "Fix login, again" });
    await settle();
    expect(container.textContent).toBe("");
  });

  it("shows nothing for a pull request that names a key", async () => {
    links.mockResolvedValue(index([authored("ABC-1")], READ));
    const { container } = mount();
    await settle();
    expect(container.textContent).toBe("");
  });
});

describe("linking a ticket", () => {
  it("picks an assigned issue, previews the write, and reports what GitHub read back", async () => {
    setPrTitle.mockResolvedValue({ state: "written", title: "ABC-1: Fix login" });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.click(await screen.findByRole("button", { name: /ABC-1/ }));
    expect(screen.getByText("ABC-1: Fix login")).toBeTruthy();
    // The write, previewed: the mutation and both its inputs, one box each.
    const preview = screen.getByLabelText("Title change");
    expect(preview.textContent).toContain("updatePullRequest");
    expect(preview.textContent).toContain("pullRequestId: PR_7");
    expect(preview.textContent).toContain("title: ABC-1: Fix login");
    expect(setPrTitle).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Change title" }));
    expect(await screen.findByText("GitHub now reads: ABC-1: Fix login")).toBeTruthy();
    // Only over the title the row showed.
    expect(setPrTitle).toHaveBeenCalledWith("PR_7", "octocat/hello-world", 7, "Fix login", "ABC-1: Fix login");
  });

  it("searches what the user types", async () => {
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.change(screen.getByLabelText("Search Jira"), { target: { value: "login" } });
    await waitFor(() => expect(search).toHaveBeenCalledWith("login"));
    expect(await screen.findByText("Summary of XY-2")).toBeTruthy();
  });

  it("does not claim a change GitHub did not confirm", async () => {
    setPrTitle.mockResolvedValue({ state: "unconfirmed", title: null });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.click(await screen.findByRole("button", { name: /ABC-1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Change title" }));
    expect(await screen.findByText(/could not be read back/)).toBeTruthy();
  });

  it("says when GitHub's title moved and nothing was written", async () => {
    setPrTitle.mockResolvedValue({ state: "changed", title: "Someone renamed it" });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.click(await screen.findByRole("button", { name: /ABC-1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Change title" }));
    expect(await screen.findByText(/Not changed: the title on GitHub is now "Someone renamed it"/)).toBeTruthy();
  });

  it("writes a GitLab title through glab, over the title the row showed", async () => {
    const target: TitleTarget = {
      kind: "gitlab",
      identity: { source: { provider: "gitlab", host: "gitlab.com" }, repo: "group/project", number: 3 },
      viewer: "octocat",
      head: "abc123",
    };
    gitLabAction.mockResolvedValue({ identity: target.identity, action: "set_title", outcome: "verified", message: "GitLab action verified." });
    mount(prefs(), ROW, target);
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.click(await screen.findByRole("button", { name: /XY-2/ }));
    expect(screen.getByLabelText("Title change").textContent).toContain("projects/group%2Fproject/merge_requests/3");
    fireEvent.click(screen.getByRole("button", { name: "Change title" }));
    expect(await screen.findByText("GitLab now reads: XY-2: Fix login")).toBeTruthy();
    expect(gitLabAction).toHaveBeenCalledWith(
      expect.objectContaining({ action: "set_title", body: "XY-2: Fix login", expected_title: "Fix login", expected_head: "abc123" }),
    );
  });
});

describe("titleWritePreview", () => {
  it("is the glab argv for GitLab, with the body on its input", () => {
    const p = titleWritePreview(
      { kind: "gitlab", identity: { source: { provider: "gitlab", host: "gitlab.example.internal" }, repo: "a/b", number: 9 }, viewer: null, head: null },
      "ABC-1: x",
    );
    expect(p.program).toBe("glab");
    expect(p.args).toEqual([
      "api", "--hostname", "gitlab.example.internal", "-i", "projects/a%2Fb/merge_requests/9",
      "--method", "PUT", "--input", "-", "--header", "Content-Type: application/json",
    ]);
    expect(p.stdin).toBe('{"title":"ABC-1: x"}');
  });
});
