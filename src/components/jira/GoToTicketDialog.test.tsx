import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { JiraConnection, JiraLink, KeyLookup, LinkIndex } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { useGoToTicket } from "@/store/goToTicket";
import { useJiraView } from "@/store/jira";
import { useSourceSelection } from "@/store/sourceSelection";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const find = vi.hoisted(() => vi.fn<(key: string, more: boolean) => Promise<KeyLookup>>());
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraLinks: links, jiraFind: find }));
vi.mock("sonner", () => ({ toast }));

import { JiraProvider } from "./JiraContext";
import { GoToTicketDialog, GoToTicketEntry } from "./GoToTicketDialog";

const prefs = { jira_enabled: true, jira_excluded_projects: [] } as unknown as UiPrefs;

const prLink = (number: number, over: Partial<Extract<JiraLink["target"], { kind: "pr" }>> = {}, current = true): JiraLink => ({
  key: "ABC-1",
  found_in: "title",
  current,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
    role: "authored",
    repo: "octocat/hello-world",
    number,
    title: `ABC-1: part ${number}`,
    url: `https://github.com/octocat/hello-world/pull/${number}`,
    head_ref: `abc-1-part-${number}`,
    is_draft: false,
    state: null,
    author: null,
    ...over,
  },
});
const index = (found: JiraLink[]): LinkIndex => ({ links: found, projects: ["ABC"], error: null });
const known: KeyLookup["search"] = { state: "not_asked", reason: "already known from your pull requests" };

beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  links.mockReset().mockResolvedValue(index([prLink(7)]));
  find.mockReset().mockImplementation((key) => Promise.resolve({ key, links: [], search: known, gitlab: null }));
  toast.success.mockReset();
  useJiraView.setState({ selected: null, filter: "all" });
  useFilters.getState().setView("worktrees");
  useSourceSelection.setState({ selection: "github" });
  useGoToTicket.setState({ open: true });
});
afterEach(() => {
  cleanup();
  useGoToTicket.setState({ open: false });
});

const quiet = () => act(() => new Promise((r) => setTimeout(r, 30)));
const mount = () =>
  renderWithQuery(
    <JiraProvider prefs={prefs}>
      <GoToTicketEntry compact={false} />
      <GoToTicketDialog />
    </JiraProvider>,
  );
const type = async (text: string) => {
  const box = await screen.findByRole("searchbox", { name: "Go to ticket" });
  fireEvent.change(box, { target: { value: text } });
  fireEvent.keyDown(box, { key: "Enter" });
};

describe("Go to ticket from any page", () => {
  it("opens the one pull request, and still offers the issue in My Jira issues and Jira", async () => {
    mount();
    await type("abc-1");
    expect(useFilters.getState().view).toBe("my-prs");
    expect(useFilters.getState().selectedPr).toMatchObject({ number: 7 });
    expect(useGoToTicket.getState().open).toBe(false);
    expect(toast.success).toHaveBeenCalledWith("Opened octocat/hello-world#7 for ABC-1", expect.anything());
    // The toast's two ways on, rendered as the toast would.
    const { description } = toast.success.mock.calls[0][1] as { description: React.ReactElement };
    renderWithQuery(description);
    expect(screen.getByRole("link", { name: "Open in Jira" }).getAttribute("href")).toBe("https://jira.example.internal/browse/ABC-1");
    fireEvent.click(screen.getByRole("button", { name: "Open ABC-1 in My Jira issues" }));
    expect(useFilters.getState().view).toBe("jira");
    expect(useJiraView.getState().selected).toBe("ABC-1");
  });

  it("lists several with state and branch, and opens the one picked", async () => {
    links.mockResolvedValue(index([prLink(7, { is_draft: true }), prLink(6, {}, false)]));
    mount();
    await type("ABC-1");
    const list = await screen.findByRole("list", { name: "Work that names ABC-1" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("octocat/hello-world#7")).toBeTruthy();
    expect(within(rows[0]).getByText("draft")).toBeTruthy();
    expect(within(rows[0]).getByText("abc-1-part-7")).toBeTruthy();
    expect(within(rows[1]).getByText("no longer open")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Open in Jira" })).toBeTruthy();
    await quiet();
    fireEvent.click(within(rows[1]).getByRole("button", { name: "Open in Headstate" }));
    expect(useFilters.getState().selectedPr).toMatchObject({ number: 6 });
    expect(useGoToTicket.getState().open).toBe(false);
  });

  /// Not found locally: Go to ticket asks the desktop itself, says it is
  /// searching while it does, then names what was searched.
  it("searches when nothing of yours names it, pending first, then names the scope", async () => {
    links.mockResolvedValue(index([]));
    let answer: (l: KeyLookup) => void = () => {};
    find.mockImplementation(() => new Promise((r) => (answer = r)));
    mount();
    await type("ABC-9");
    expect((await screen.findByRole("status")).textContent).toBe("Searching GitHub…");
    expect(find).toHaveBeenCalledWith("ABC-9", false);
    expect(screen.queryByText(/None of your/)).toBeNull();
    await act(async () =>
      answer({
        key: "ABC-9",
        links: [],
        search: { state: "asked", at: "2026-10-28T15:04:00Z", total: 0, read: 0, truncated: false, scope: ["octocat"], unsearched: [] },
        gitlab: { state: "not_asked", reason: "the GitLab CLI (glab) is not installed" },
      }),
    );
    expect(await screen.findByText(/^No pull requests name ABC-9 in repositories owned by octocat on GitHub/)).toBeTruthy();
    expect(screen.getByText("GitLab not searched: the GitLab CLI (glab) is not installed.")).toBeTruthy();
    expect(screen.getByText("None of your pull requests, worktrees or branches names ABC-9.")).toBeTruthy();
  });

  it("lists what a search found, opening it on its host", async () => {
    links.mockResolvedValue(index([]));
    find.mockResolvedValue({
      key: "ABC-1",
      links: [
        prLink(9, {
          role: "searched",
          state: "merged",
          source: { provider: "gitlab", host: "gitlab.example.test" },
          repo: "group/app",
          url: "https://gitlab.example.test/group/app/-/merge_requests/9",
        }, false),
      ],
      search: { state: "failed", error: "GitHub did not answer within 15s" },
      gitlab: { state: "asked", at: "2026-10-28T15:04:00Z", total: 1, read: 1, truncated: false, scope: ["gitlab.example.test/group/app"], unsearched: [] },
    });
    mount();
    await type("ABC-1");
    const row = (await screen.findAllByRole("listitem"))[0];
    expect(within(row).getByText("group/app!9")).toBeTruthy();
    expect(within(row).getByText("merged")).toBeTruthy();
    expect(within(row).queryByRole("button", { name: "Open in Headstate" })).toBeNull();
    expect(within(row).getByRole("link", { name: "Open on GitLab" })).toBeTruthy();
    expect(screen.getByText("Searching GitHub failed: GitHub did not answer within 15s")).toBeTruthy();
  });

  it("says Jira is not ready rather than doing nothing", async () => {
    connection.mockResolvedValue({ state: "not_logged_in", detail: "" });
    mount();
    expect(await screen.findByText(/Jira is not connected/)).toBeTruthy();
    expect(screen.queryByRole("searchbox")).toBeNull();
    // And there is no header button to reach it by.
    expect(screen.queryByRole("button", { name: "Go to ticket" })).toBeNull();
  });

  it("opens from the header button", async () => {
    useGoToTicket.setState({ open: false });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Go to ticket" }));
    expect(useGoToTicket.getState().open).toBe(true);
    expect(await screen.findByRole("searchbox", { name: "Go to ticket" })).toBeTruthy();
  });
});
