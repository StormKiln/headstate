import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, JiraConnection, JiraIssue, JiraLink, LinkIndex } from "@/types/jira";
import { createQueryClient } from "@/api/queryClient";
import { JIRA_REASON } from "@/api/jira";
import { renderWithQuery, stubViewport } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";

/// Ticket chips on the phone (#2047).
///
/// The chips are the same component on both surfaces, and every Jira
/// command is `Read` on both, so the phone renders them rather than
/// hiding them. What differs is how a failure arrives: through the
/// companion's relay a rejection is text, and a current desktop's typed
/// reason travels behind `JIRA_REASON`. These tests pin that a chip at
/// phone width under the mobile build says the real reason, keeps an
/// answer already shown when a refetch fails, and still opens the issue.
///
/// The viewport is stubbed with the build flag, as
/// `RepositoriesPage.mobile.test.tsx` argues: one without the other is
/// not a phone.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const links = vi.hoisted(() => vi.fn<() => Promise<LinkIndex>>());
const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraLinks: links, jiraIssues: issues }));

import { JiraProvider } from "./JiraContext";
import { PrRowTicketChips, TicketChips } from "./TicketChips";

const prefs: UiPrefs = {
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
};

const issue = (key: string): JiraIssue => ({
  key,
  summary: `Summary of ${key}`,
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: "In Review", category: "in_progress" },
  assignee: "Dana Example",
  parent_key: null,
  parent_checked: true,
  project_key: key.split("-")[0],
  url: `https://jira.example.internal/browse/${key}`,
  updated: null,
});
const batch = (over: Partial<IssueBatch>): IssueBatch => ({
  issues: [],
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
  ...over,
});

const URL = "https://github.com/octocat/hello-world/pull/7";
const prLink = (key: string): JiraLink => ({
  key,
  found_in: "title",
  current: true,
  target: {
    kind: "pr",
    source: { provider: "github", host: "github.com" },
    role: "authored",
    repo: "octocat/hello-world",
    number: 7,
    title: `${key}: fix login`,
    url: URL,
    head_ref: "fix-login",
    is_draft: false,
    state: null,
    author: null,
  },
});
const index = (key: string): LinkIndex => ({ links: [prLink(key)], projects: ["ABC"], error: null });

/// As the companion rejects a Jira call when the desktop sent its reason.
const overThePhone = (reason: unknown, message: string) => `${JIRA_REASON}${JSON.stringify({ reason, message })}`;

beforeAll(() => {
  stubViewport(390);
});
afterAll(() => {
  stubViewport(null);
});
beforeEach(() => {
  connection.mockReset().mockResolvedValue({ state: "logged_in", site: "jira.example.internal", account: null });
  links.mockReset();
  issues.mockReset();
  useJiraView.setState({ selected: null, filter: "all" });
});
afterEach(cleanup);

/// The module's loader remembers answers per key, so each test uses its own.
describe("ticket chips on a phone", () => {
  it("renders a pull request row's chip, and a tap opens the issue", async () => {
    links.mockResolvedValue(index("ABC-1"));
    issues.mockResolvedValue(batch({ issues: [issue("ABC-1")] }));
    renderWithQuery(
      <JiraProvider prefs={prefs}>
        <PrRowTicketChips prUrl={URL} draft={false} />
      </JiraProvider>,
    );
    const chip = await screen.findByRole("button", { name: /ABC-1 · In Review/ });
    expect(chip.getAttribute("title")).toContain("Summary of ABC-1 — Dana Example");
    fireEvent.click(chip);
    expect(useJiraView.getState().selected).toBe("ABC-1");
    expect(useFilters.getState().view).toBe("jira");
  });

  /// The reason a phone shows is the desktop's real one: a lost sign-in,
  /// not "failed" with a sentence that happens to say so.
  it("says why a read failed in the desktop's typed terms", async () => {
    links.mockResolvedValue(index("ABC-2"));
    issues.mockRejectedValue(overThePhone({ state: "not_logged_in" }, "acli is not signed in to Jira"));
    renderWithQuery(
      <JiraProvider prefs={prefs}>
        <TicketChips row={{ prUrl: URL }} />
      </JiraProvider>,
    );
    const chip = await screen.findByText("ABC-2 · not loaded");
    expect(chip.getAttribute("title")).toBe(
      "Could not ask Jira about ABC-2: acli is not signed in to Jira. Run `acli jira auth login --web` in a terminal.",
    );
    // The marker is never on screen.
    expect(chip.getAttribute("title")).not.toContain(JIRA_REASON.trim());
  });

  /// #2036 on the phone: an answer already on screen stays, marked stale,
  /// when a refetch through the relay fails -- with the typed reason.
  it("keeps a shown status, marked stale, when a refetch fails", async () => {
    links.mockResolvedValue(index("ABC-3"));
    issues.mockResolvedValue(batch({ issues: [issue("ABC-3")] }));
    const client = createQueryClient();
    render(
      <QueryClientProvider client={client}>
        <JiraProvider prefs={prefs}>
          <TicketChips row={{ prUrl: URL }} />
        </JiraProvider>
      </QueryClientProvider>,
    );
    await screen.findByRole("button", { name: /ABC-3 · In Review/ });
    expect(screen.queryByLabelText("Not current")).toBeNull();

    issues.mockRejectedValue(overThePhone({ state: "timed_out", secs: 20 }, "acli did not answer within 20s"));
    await client.refetchQueries({ queryKey: ["jira-issue", "ABC-3"] });
    await waitFor(() => expect(screen.getByLabelText("Not current")).toBeTruthy());
    const chip = screen.getByRole("button", { name: /ABC-3 · In Review/ });
    expect(chip.className).toContain("border-dashed");
    expect(chip.getAttribute("title")).toMatch(/the latest read failed \(acli did not answer within 20s\./);
    expect(screen.queryByText(/not loaded/)).toBeNull();
  });
});
