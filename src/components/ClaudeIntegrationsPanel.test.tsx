import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ClaudeHooksStatus, UiPrefs } from "@/api/tauri";

const install = vi.hoisted(() => vi.fn());
const reinstall = vi.hoisted(() => vi.fn());
const uninstall = vi.hoisted(() => vi.fn());
const hookState = vi.hoisted(() => ({ status: undefined as ClaudeHooksStatus | undefined }));
const toastSuccess = vi.hoisted(() => vi.fn());

vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: vi.fn() } }));
vi.mock("@/api/hooks", () => ({
  useClaudeHooks: () => ({
    status: hookState.status,
    isLoading: hookState.status === undefined,
    error: null,
    install,
    reinstall,
    uninstall,
  }),
}));
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false }));

import { ClaudeIntegrationsPanel, statusLine } from "./ClaudeIntegrationsPanel";

const PREFS: UiPrefs = {
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: true,
  claude_integrations_enabled: false,
  diagnostic_logging: false,
  stale_venv_days: 0,
  battery_low_percent: 0,
};

const setPrefs = vi.fn(() => Promise.resolve());

beforeEach(() => {
  install.mockReset().mockResolvedValue({ command: "/app headstate claude-hook", added: ["SessionStart", "SessionEnd"], replaced: [], created_file: false });
  reinstall.mockReset().mockResolvedValue({ command: "/app headstate claude-hook", added: [], replaced: ["SessionStart", "SessionEnd"], created_file: false });
  uninstall.mockReset().mockResolvedValue({ removed: ["SessionStart", "SessionEnd"], was_absent: false });
  setPrefs.mockClear();
  toastSuccess.mockClear();
  hookState.status = { state: "not_installed" };
});
afterEach(cleanup);

const panel = () => render(<ClaudeIntegrationsPanel prefs={PREFS} setPrefs={setPrefs} />);

/// #915: the settings section, and the two controls that must not be
/// confused for each other.
describe("the Claude integrations settings section", () => {
  /// The headline UI rule of #915 and epic §6: the switch HIDES, it does not
  /// uninstall. A user who reads "off" as "nothing is running" would be
  /// wrong -- the hook keeps appending -- and discovering that from a file
  /// they did not expect is worse than being told.
  it("says in the panel that switching off does not remove the hook", () => {
    panel();
    const body = document.body.textContent ?? "";
    expect(body).toMatch(/hides the view/i);
    expect(body).toMatch(/does\s*not\s*remove the hook/i);
    // And says WHY it is a separate control, which is the part that makes
    // the design decision legible rather than arbitrary.
    expect(body).toMatch(/permanent gap/i);
  });

  /// Toggling the switch writes only `claude_integrations_enabled`, in BOTH
  /// directions. A toggle that also called install or uninstall would be the
  /// exact conflation the epic settled against.
  ///
  /// Both directions, and that is not padding. SABOTAGE FOUND THE GAP: a
  /// first version of this test only toggled ON, and a sabotage that
  /// uninstalled on the OFF transition -- which is the only transition the
  /// design decision is actually about, since that is the one that would
  /// destroy history -- left it green. A guard that cannot see the defect it
  /// exists for is worse than no guard, because it gets cited.
  it.each([
    ["on", false, true],
    ["off", true, false],
  ])("toggling the switch %s changes only the preference", (_name, from, to) => {
    const prefs = { ...PREFS, claude_integrations_enabled: from };
    render(<ClaudeIntegrationsPanel prefs={prefs} setPrefs={setPrefs} />);

    fireEvent.click(screen.getByRole("checkbox", { name: /show claude code sessions/i }));

    expect(setPrefs).toHaveBeenCalledWith({ ...prefs, claude_integrations_enabled: to });
    expect(install).not.toHaveBeenCalled();
    expect(uninstall).not.toHaveBeenCalled();
    expect(reinstall).not.toHaveBeenCalled();
  });

  /// The section is reachable while the capability is OFF -- greyed, not
  /// hidden, per the epic -- because that is where the switch lives.
  it("offers the install controls even while the integrations are off", () => {
    panel();
    expect(screen.getByRole("checkbox", { name: /show claude code sessions/i })).toHaveProperty(
      "checked",
      false,
    );
    expect(screen.getByRole("button", { name: /install/i }).hasAttribute("disabled")).toBe(false);
  });

  /// A malformed settings file must NOT offer the Install button.
  ///
  /// This is the load-bearing half of the three-state status. `cannot_tell`
  /// rendered as `not_installed` would show an enabled Install button whose
  /// only possible outcome is a refusal -- a loop with no explanation, for a
  /// user whose every hook is already silently dead.
  it("refuses to offer Install when it cannot read the settings file", () => {
    hookState.status = {
      state: "cannot_tell",
      kind: "malformed",
      path: "/Users/acme/.claude/settings.json",
      detail: "expected `,` or `}` at line 12 column 3",
    };
    panel();

    expect(screen.getByRole("button", { name: /install/i }).hasAttribute("disabled")).toBe(true);
    // And it explains, loudly enough for a screen reader, including the
    // position -- because fixing the JSON by hand is the only remedy.
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toMatch(/line 12 column 3/);
    expect(alert.textContent).toMatch(/silently ignores/i);
    expect(alert.textContent).toMatch(/other tools/i);
  });

  /// Nothing of ours in the file is `not_installed`, which is the one state
  /// that SHOULD invite the button -- and must not carry an alert.
  it("offers Install, and no alarm, when nothing is installed", () => {
    panel();
    expect(screen.getByRole("button", { name: /install/i }).hasAttribute("disabled")).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: /remove/i })).toBeNull();
  });

  /// Installed: Reinstall and Remove, and no Install.
  it("offers Reinstall and Remove once the hook is in", () => {
    hookState.status = { state: "installed", command: "'/Applications/Headstate.app/Contents/MacOS/headstate' claude-hook" };
    panel();

    expect(screen.getByRole("button", { name: /reinstall/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /remove/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^install$/i })).toBeNull();
    // The command line is SHOWN: a user has to be able to see what Claude
    // Code will actually run, especially after the app has moved.
    expect(document.body.textContent).toMatch(/Headstate\.app\/Contents\/MacOS\/headstate/);
  });

  /// A stale install is an alert with the reason in it, and offers both the
  /// repair and the removal -- a user might reasonably want either.
  it("reports a stale install and offers both repair and removal", () => {
    hookState.status = {
      state: "stale",
      detail: "the installed command is /old/path rather than /new/path",
    };
    panel();

    expect(screen.getByRole("alert").textContent).toMatch(/old\/path/);
    expect(screen.getByRole("button", { name: /reinstall/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /remove/i })).toBeTruthy();
  });

  /// A replaced hand-edit is REPORTED (§5.4): silently reverting someone's
  /// edit is its own defect, and this toast is where they find out.
  it("says so when a reinstall replaced an existing entry", async () => {
    hookState.status = { state: "installed", command: "x" };
    panel();

    fireEvent.click(screen.getByRole("button", { name: /reinstall/i }));

    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
    const [, opts] = toastSuccess.mock.calls[0] as [string, { description?: string }];
    expect(opts.description).toMatch(/replaced/i);
    expect(opts.description).toMatch(/SessionStart and SessionEnd/);
  });

  /// An uninstall that found nothing says that, rather than claiming to have
  /// removed something.
  it("distinguishes removing something from there being nothing to remove", async () => {
    uninstall.mockResolvedValue({ removed: [], was_absent: true });
    hookState.status = { state: "installed", command: "x" };
    panel();

    fireEvent.click(screen.getByRole("button", { name: /remove/i }));

    await waitFor(() => expect(toastSuccess).toHaveBeenCalledWith(expect.stringMatching(/nothing/i)));
  });

  /// A refusal is SHOWN rather than swallowed. This is the only channel the
  /// refusal text has, and for a malformed file it is the only warning the
  /// user will ever get.
  it("shows the refusal when an install is rejected", async () => {
    install.mockRejectedValue(
      "/Users/acme/.claude/settings.json is not valid JSON (expected value at line 3 column 1). " +
        "Headstate will not rewrite it",
    );
    panel();

    fireEvent.click(screen.getByRole("button", { name: /install/i }));

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toMatch(/will not rewrite it/),
    );
  });
});

/// The sentence per state, tested without rendering so the four states
/// cannot quietly collapse into three.
describe("the status sentence", () => {
  it("never describes an unreadable file the way it describes an absent hook", () => {
    const cannotTell = statusLine({
      state: "cannot_tell",
      kind: "malformed",
      path: "/p",
      detail: "bad",
    });
    const notInstalled = statusLine({ state: "not_installed" });

    expect(cannotTell.problem).toBe(true);
    expect(notInstalled.problem).toBe(false);
    expect(cannotTell.text).not.toEqual(notInstalled.text);
  });

  it("says what is lost without the hook, rather than only that it is absent", () => {
    // "Not installed." alone invites the reader to assume nothing works.
    // What is actually true is narrower and worth stating: the transcript
    // backstop still lists sessions, and what is missing is the pid -- which
    // is exactly the thing that cannot be recovered later.
    const { text } = statusLine({ state: "not_installed" });
    expect(text).toMatch(/transcripts/i);
    expect(text).toMatch(/process id/i);
  });

  it("carries the io refusal's own detail", () => {
    const { text, problem } = statusLine({
      state: "cannot_tell",
      kind: "io",
      path: "/p",
      detail: "Permission denied (os error 13)",
    });
    expect(problem).toBe(true);
    expect(text).toMatch(/Permission denied/);
  });

  it("is a neutral 'checking' line before the first read lands", () => {
    const { text, problem } = statusLine(undefined);
    expect(problem).toBe(false);
    // Specifically NOT "not installed": an unresolved query must not render
    // as a fact about the file.
    expect(text).not.toMatch(/not installed/i);
  });
});
