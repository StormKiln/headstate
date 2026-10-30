import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { IssueBatch, LinkIndex } from "@/types/jira";
import { renderWithQuery, stubViewport } from "@/test-utils";

/// Linking a ticket from the phone (#2046). `set_pr_title` and
/// `gitlab_action` are Write on the companion surface, so the phone may
/// send them -- and the confirmation must say the change runs on the
/// paired desktop, not on the phone.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));
vi.mock("@/api/connection", () => ({
  useConnectionState: () => ({ kind: "connected", desktop: "Studio Mac", lastPoll: null, protocolVersion: null, stale: false }),
}));

const URL = "https://github.com/octocat/hello-world/pull/7";
const links: LinkIndex = {
  links: [],
  projects: ["ABC"],
  error: null,
  checked: [{ url: URL, title: "Fix login", head_ref: "fix-login" }],
};
const assigned: IssueBatch = {
  issues: [
    {
      key: "ABC-1",
      summary: "Login",
      issue_type: { name: "Story", hierarchy_level: 0 },
      status: { name: "To Do", category: "new" },
      assignee: null,
      parent_key: null,
      parent_checked: true,
      project_key: "ABC",
      url: null,
      updated: null,
    },
  ],
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
};
vi.mock("@/api/tauri", () => ({
  jiraConnection: () => Promise.resolve({ state: "logged_in", site: "jira.example.internal", account: null }),
  jiraLinks: () => Promise.resolve(links),
  jiraIssues: vi.fn(),
  jiraAssigned: () => Promise.resolve(assigned),
  jiraSearch: vi.fn(),
  setPrTitle: vi.fn(),
  gitLabAction: vi.fn(),
}));

import { JiraProvider } from "./JiraContext";
import { NoTicketMarker } from "./NoTicket";

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
  jira_branch_template: "",
  jira_no_ticket_marker: true,
};

beforeAll(() => stubViewport(390));
afterAll(() => stubViewport(null));
afterEach(cleanup);

describe("linking a ticket on a phone", () => {
  it("says the title change runs on the desktop", async () => {
    renderWithQuery(
      <JiraProvider prefs={prefs}>
        <NoTicketMarker
          row={{ url: URL, title: "Fix login", head_ref: "fix-login" }}
          target={{ kind: "github", id: "PR_7", repo: "octocat/hello-world", number: 7 }}
        />
      </JiraProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "no ticket" }));
    fireEvent.click(await screen.findByRole("button", { name: /ABC-1/ }));
    expect(screen.getByText(/This runs on/).textContent).toContain("Studio Mac");
    expect(screen.getByRole("button", { name: "Change title" })).toBeTruthy();
  });
});
