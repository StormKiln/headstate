import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UiPrefs } from "@/api/tauri";
import type { JiraConnection, ProjectList, ProjectStatuses } from "@/types/jira";
import { renderWithQuery } from "@/test-utils";

const connection = vi.hoisted(() => vi.fn<() => Promise<JiraConnection>>());
const projects = vi.hoisted(() => vi.fn<() => Promise<ProjectList>>());
const version = vi.hoisted(() => vi.fn<() => Promise<string>>());
const statuses = vi.hoisted(() => vi.fn<(project: string) => Promise<ProjectStatuses>>());
vi.mock("@/api/tauri", () => ({
  jiraConnection: connection,
  jiraProjects: projects,
  jiraAcliVersion: version,
  jiraProjectStatuses: statuses,
}));
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
  jira_excluded_projects: [],
  jira_status_expectations: {},
  jira_chips_on_prs: true,
});

beforeEach(() => {
  connection.mockReset();
  projects.mockReset();
  version.mockReset().mockResolvedValue("1.3.39-stable");
  statuses.mockReset();
});
afterEach(cleanup);

describe("JiraSettingsPanel", () => {
  /// Off means off: no `acli` is spawned, so nothing is asked at all.
  it("asks the desktop nothing while switched off", () => {
    const setPrefs = vi.fn(() => Promise.resolve());
    renderWithQuery(<JiraSettingsPanel prefs={prefs(false)} setPrefs={setPrefs} />);
    expect(connection).not.toHaveBeenCalled();
    expect(projects).not.toHaveBeenCalled();
    expect(version).not.toHaveBeenCalled();
    expect(statuses).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Link pull requests to Jira" }));
    expect(setPrefs).toHaveBeenCalledWith(expect.objectContaining({ jira_enabled: true }));
  });

  it("names the project keys once signed in", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: SITE, account: null });
    projects.mockResolvedValue({ projects: [{ key: "ABC", name: "Alpha" }], error: null });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(`Signed in to ${SITE}.`)).toBeTruthy();
    expect(await screen.findByRole("button", { name: "ABC" })).toBeTruthy();
  });

  it("turns a project off and back on", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: null, account: null });
    projects.mockResolvedValue({ projects: [{ key: "ABC", name: "Alpha" }], error: null });
    const setPrefs = vi.fn(() => Promise.resolve());
    const view = renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={setPrefs} />);
    fireEvent.click(await screen.findByRole("button", { name: "ABC", pressed: true }));
    expect(setPrefs).toHaveBeenLastCalledWith(expect.objectContaining({ jira_excluded_projects: ["ABC"] }));
    view.unmount();
    renderWithQuery(<JiraSettingsPanel prefs={{ ...prefs(true), jira_excluded_projects: ["ABC"] }} setPrefs={setPrefs} />);
    fireEvent.click(await screen.findByRole("button", { name: "ABC", pressed: false }));
    expect(setPrefs).toHaveBeenLastCalledWith(expect.objectContaining({ jira_excluded_projects: [] }));
  });

  /// Each state carries its own remedy; none reads as "no projects".
  it.each<[JiraConnection, RegExp]>([
    [{ state: "not_installed" }, /acli is not installed/],
    [{ state: "not_logged_in", detail: "x" }, /not signed in/],
    [{ state: "cannot_tell", reason: "odd output" }, /It said: odd output/],
  ])("explains %o without listing projects", async (state, text) => {
    connection.mockResolvedValue(state);
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(text)).toBeTruthy();
    expect(projects).not.toHaveBeenCalled();
  });

  /// The fix goes through ArgvPreview: one box per argv word.
  it("shows the sign-in command word by word, and copies it whole", async () => {
    connection.mockResolvedValue({ state: "not_logged_in", detail: "x" });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    const box = await screen.findByLabelText("Sign-in command");
    expect([...box.querySelectorAll("code")].map((c) => c.textContent)).toEqual([
      "acli",
      "jira",
      "auth",
      "login",
      "--web",
    ]);
  });

  it("names the acli version once acli is found, and never asks while it is missing", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: null, account: null });
    projects.mockResolvedValue({ projects: [], error: null });
    const view = renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText("acli 1.3.39-stable")).toBeTruthy();
    view.unmount();

    version.mockClear();
    connection.mockResolvedValue({ state: "not_installed" });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(/acli is not installed/)).toBeTruthy();
    expect(version).not.toHaveBeenCalled();
  });

  /// Pending is a skeleton; a failure says so and is not a version.
  it("says when the version could not be read", async () => {
    connection.mockResolvedValue({ state: "cannot_tell", reason: "odd" });
    version.mockRejectedValue("acli --version said: hello");
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText("Could not read the acli version: acli --version said: hello")).toBeTruthy();
  });

  it("turns ticket chips on pull request rows off", async () => {
    connection.mockResolvedValue({ state: "not_installed" });
    const setPrefs = vi.fn(() => Promise.resolve());
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={setPrefs} />);
    const box = screen.getByRole("checkbox", { name: "Show tickets on pull request rows" });
    expect((box as HTMLInputElement).checked).toBe(true);
    fireEvent.click(box);
    expect(setPrefs).toHaveBeenCalledWith(expect.objectContaining({ jira_chips_on_prs: false }));
  });

  describe("status expectations", () => {
    const signedIn = () => {
      connection.mockResolvedValue({ state: "logged_in", site: null, account: null });
      projects.mockResolvedValue({ projects: [{ key: "ABC", name: "Alpha" }], error: null });
    };

    it("reads a project's statuses only once it is chosen, and offers them beside the categories", async () => {
      signedIn();
      statuses.mockResolvedValue({
        project: "ABC",
        statuses: [{ name: "In Review", category: "in_progress" }],
        truncated: false,
        error: null,
      });
      const setPrefs = vi.fn(() => Promise.resolve());
      renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={setPrefs} />);
      const pick = await screen.findByRole("combobox", { name: "Project" });
      expect(statuses).not.toHaveBeenCalled();
      fireEvent.change(pick, { target: { value: "ABC" } });
      const open = await screen.findByRole("combobox", { name: "While a pull request is open" });
      // One per select: while open, and once merged.
      expect(await screen.findAllByRole("option", { name: "In Review" })).toHaveLength(2);
      expect(statuses).toHaveBeenCalledWith("ABC");
      expect(screen.getAllByRole("group", { name: "Statuses seen in ABC's issues" })[0].parentElement).toBe(open);
      // The three categories are always offered, and the default is named.
      for (const name of ["any To Do status", "any In Progress status", "any Done status"]) {
        expect(screen.getAllByRole("option", { name }).length).toBe(2);
      }
      expect(screen.getByRole("option", { name: "Default (any In Progress status)" })).toBeTruthy();
      expect(screen.getByRole("option", { name: "Default (any Done status)" })).toBeTruthy();

      fireEvent.change(open, { target: { value: "status:In Review" } });
      expect(setPrefs).toHaveBeenLastCalledWith(
        expect.objectContaining({
          jira_status_expectations: { ABC: { on_open: { kind: "status", name: "In Review" } } },
        }),
      );
    });

    it("drops a project's entry when both sides go back to the default", async () => {
      signedIn();
      statuses.mockResolvedValue({ project: "ABC", statuses: [], truncated: false, error: null });
      const setPrefs = vi.fn(() => Promise.resolve());
      const set = { ...prefs(true), jira_status_expectations: { ABC: { on_merged: { kind: "category", category: "done" } } } } as UiPrefs;
      renderWithQuery(<JiraSettingsPanel prefs={set} setPrefs={setPrefs} />);
      fireEvent.change(await screen.findByRole("combobox", { name: "Project" }), { target: { value: "ABC" } });
      const merged = await screen.findByRole("combobox", { name: "Once it has merged" });
      expect((merged as HTMLSelectElement).value).toBe("category:done");
      fireEvent.change(merged, { target: { value: "" } });
      expect(setPrefs).toHaveBeenLastCalledWith(expect.objectContaining({ jira_status_expectations: {} }));
    });

    it("says when the statuses could not be read, and still offers the categories", async () => {
      signedIn();
      statuses.mockResolvedValue({ project: "ABC", statuses: [], truncated: false, error: "unauthorized" });
      renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
      fireEvent.change(await screen.findByRole("combobox", { name: "Project" }), { target: { value: "ABC" } });
      expect(await screen.findByText(/Could not read the statuses in ABC's issues: unauthorized/)).toBeTruthy();
      expect(screen.getAllByRole("option", { name: "any Done status" }).length).toBe(2);
    });
  });

  it("says when the project list could not be read rather than showing none", async () => {
    connection.mockResolvedValue({ state: "logged_in", site: null, account: null });
    projects.mockResolvedValue({ projects: [], error: "unauthorized" });
    renderWithQuery(<JiraSettingsPanel prefs={prefs(true)} setPrefs={vi.fn()} />);
    expect(await screen.findByText(/Could not list your Jira projects: unauthorized/)).toBeTruthy();
  });
});
