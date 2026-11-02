import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { renderWithQuery } from "@/test-utils";
import { useSourceSelection } from "@/store/sourceSelection";
import { useSettingsDialog } from "@/store/settingsDialog";
import type { GitLabAuthState } from "@/api/tauri";
import { AuthGate } from "./AuthGate";
import { SourceQueue } from "./SourceQueue";
import { gitlabHostLine } from "@/lib/gitlabAuthRemedy";

afterEach(() => {
  cleanup();
  useSourceSelection.setState({ selection: "github" });
  useSettingsDialog.setState({ section: null });
  clearMocks();
});

/// The reported case: nothing saved, so gitlab.com is checked and refused,
/// while glab is signed in to a self-hosted server.
const DEFAULT_MISMATCH: GitLabAuthState = {
  host: "gitlab.com", ok: false, issue: "unverified", viewer: null, configured: false,
  signedInHosts: ["gitlab.example.com"],
  message: "GitLab authentication for gitlab.com could not be verified. No GitLab host is saved in Settings, so gitlab.com, the default, was checked. glab on the desktop is signed in to gitlab.example.com. Save that host in Settings on the desktop.",
};

function mount(answers: () => GitLabAuthState, onCall: (cmd: string, args: unknown) => void = () => {}) {
  let host = "gitlab.com";
  let checks = 0;
  mockIPC((cmd, args) => {
    onCall(cmd, args);
    if (cmd === "get_auth_state") return { ok: true, message: "" };
    if (cmd === "get_gitlab_host") return host;
    if (cmd === "set_gitlab_host") { host = (args as { host: string }).host; return host; }
    if (cmd === "get_gitlab_auth_state") { checks += 1; return answers(); }
    return undefined;
  }, { shouldMockEvents: true });
  useSourceSelection.setState({ selection: "gitlab" });
  renderWithQuery(<AuthGate><div>content</div></AuthGate>);
  return { checks: () => checks, host: () => host };
}

describe("GitLab sign-in remedy", () => {
  it("does not offer a retry that would check the same wrong host; offers the signed-in host and Settings", async () => {
    mount(() => DEFAULT_MISMATCH);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("gitlab.com, the default, was checked");
    expect(within(alert).queryByRole("button", { name: /Retry/ })).toBeNull();
    fireEvent.click(within(alert).getByRole("button", { name: "Open GitLab settings" }));
    expect(useSettingsDialog.getState().section).toBe("gitlab");
  });

  it("saves the signed-in host in one click and re-checks against it", async () => {
    const hosts: string[] = [];
    const state = mount(() => (state?.host() === "gitlab.example.com"
      ? { host: "gitlab.example.com", ok: true, issue: null, viewer: "fixture", message: "", configured: true, signedInHosts: null }
      : DEFAULT_MISMATCH), (cmd, args) => { if (cmd === "set_gitlab_host") hosts.push((args as { host: string }).host); });
    fireEvent.click(await screen.findByRole("button", { name: "Use gitlab.example.com" }));
    await waitFor(() => expect(hosts).toEqual(["gitlab.example.com"]));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(state.checks()).toBeGreaterThanOrEqual(2);
  });

  it("a retry visibly runs, re-asks, and reports when it finished", async () => {
    let release: () => void = () => {};
    let call = 0;
    const failed: GitLabAuthState = { host: "gitlab.example.com", ok: false, issue: "unverified", viewer: null, configured: true, signedInHosts: null, message: "GitLab authentication for gitlab.example.com could not be verified." };
    const state = mount(() => failed);
    const retry = await screen.findByRole("button", { name: "Retry GitLab authentication" });
    expect(screen.getByText(/Checked at/)).toBeTruthy();
    // Hold the second answer so the pending state can be observed.
    mockIPC((cmd) => {
      if (cmd === "get_auth_state") return { ok: true, message: "" };
      if (cmd === "get_gitlab_host") return "gitlab.example.com";
      if (cmd === "get_gitlab_auth_state") { call += 1; return new Promise((resolve) => { release = () => resolve(failed); }); }
      return undefined;
    }, { shouldMockEvents: true });
    fireEvent.click(retry);
    const checking = await screen.findByRole("button", { name: "Checking GitLab authentication…" });
    expect((checking as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByText(/Checked at/)).toBeNull();
    expect(call).toBe(1);
    release();
    expect(await screen.findByRole("button", { name: "Retry GitLab authentication" })).toBeTruthy();
    expect(screen.getByText(/Checked at/)).toBeTruthy();
    expect(state.checks()).toBe(1);
  });
});

describe("the GitLab queue's own line", () => {
  it("offers the sign-in remedy, not a queue refresh that sends nothing, while sign-in is unverified", async () => {
    let refreshes = 0;
    mockIPC((cmd) => {
      if (cmd === "get_auth_state") return { ok: true, message: "" };
      if (cmd === "get_gitlab_host") return "gitlab.com";
      if (cmd === "get_gitlab_auth_state") return DEFAULT_MISMATCH;
      return undefined;
    }, { shouldMockEvents: true });
    useSourceSelection.setState({ selection: "gitlab" });
    renderWithQuery(
      <AuthGate><SourceQueue
        selection="gitlab" github={[]} gitlab={undefined} githubLoading={false} gitlabLoading={false}
        githubError={null} gitlabError="Not requested: the GitLab sign-in is not verified." gitlabAuthBlocked
        githubCoverage={null} gitlabCoverage={null} githubStaleSecs={null} gitlabStaleSecs={null}
        canWriteGitHub={false} onOpen={() => {}} onRefreshGitHub={() => {}} onRefreshGitLab={() => { refreshes += 1; }}
      /></AuthGate>,
    );
    const line = (await screen.findByText(/Not requested: the GitLab sign-in is not verified/)).closest("[role]") as HTMLElement;
    // Declined, not failed: a status, not an alert.
    expect(line.getAttribute("role")).toBe("status");
    expect(within(line).queryByRole("button", { name: "Retry" })).toBeNull();
    expect(await within(line).findByRole("button", { name: "Use gitlab.example.com" })).toBeTruthy();
    expect(await screen.findByText("GitLab queues use gitlab.com, the default. No GitLab host is saved in Settings on the desktop.")).toBeTruthy();
    expect(refreshes).toBe(0);
  });
});

describe("gitlabHostLine", () => {
  it("names the host the queues use, and says when it is only the default", () => {
    expect(gitlabHostLine("gitlab.com", false)).toBe("GitLab queues use gitlab.com, the default. No GitLab host is saved in Settings on the desktop.");
    expect(gitlabHostLine("gitlab.example.com", true)).toBe("GitLab queues use gitlab.example.com, set in Settings on the desktop.");
    expect(gitlabHostLine(undefined, undefined)).toContain("configured in Settings");
  });
});
