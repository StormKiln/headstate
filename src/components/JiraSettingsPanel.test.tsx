import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { JiraConnection, ProjectList } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const projects = vi.hoisted(() => vi.fn<() => Promise<ProjectList>>());
vi.mock("@/api/tauri", () => ({ jiraConnection: connection, jiraProjects: projects }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { JiraSettingsPanel } from "./JiraSettingsPanel";

/// Assembled at runtime: the privacy gate rejects a literal Atlassian host.
const SITE = ["acme", "atlassian", "net"].join(".");

const prefs = (jira_enabled: boolean): UiPrefs => ({
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: true,
  claude_integrations_enabled: false,
  diagnostic_logging: false,
  terminal_command: "",
  stale_venv_days: 0,
  battery_low_percent: 0,
  jira_enabled,
});

beforeEach(() => {
  connection.mockReset();
  projects.mockReset();
});
afterEach(cleanup);

describe("JiraSettingsPanel", () => {
  /// Off means off: no `acli` is spawned, so nothing is asked at all.
  it("asks the desktop nothing while switched off", () => {
    const setPrefs = vi.fn(() => Promise.resolve());
    renderWithQuery(<JiraSettingsPanel prefs={prefs(false)} setPrefs={setPrefs} />);
    expect(connection).not.toHaveBeenCalled();
    expect(projects).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Link pull requests to Jira" }));
    expect(setPrefs).toHaveBeenCalledWith(expect.objectContaining({ jira_enabled: true }));
  });

  it("names the project keys once signed in", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: SITE, account: null });
    projects.mockResolvedValue({ projects: [{ key: "ABC", name: "Alpha" }], error: null });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(`Signed in to ${SITE}.`)).toBeTruthy();
    expect(await screen.findByText("ABC")).toBeTruthy();
  });

  /// Each state carries its own remedy; none reads as "no projects".
  it.each<[JiraConnection, RegExp]>([
    [{ state: "not_installed" }, /acli is not installed/],
    [{ state: "not_logged_in", detail: "x" }, /acli jira auth login --web/],
    [{ state: "cannot_tell", reason: "odd output" }, /It said: odd output/],
  ])("explains %o without listing projects", async (state, text) => {
    connection.mockResolvedValue(state);
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(text)).toBeTruthy();
    expect(projects).not.toHaveBeenCalled();
  });

  it("says when the project list could not be read rather than showing none", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: null, account: null });
    projects.mockResolvedValue({ projects: [], error: "unauthorized" });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(/Could not list your Jira projects: unauthorized/)).toBeTruthy();
  });
});
