import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ClaudeHooksStatus, UiPrefs } from "@/api/tauri";

const install = vi.hoisted(() => vi.fn());
const reinstall = vi.hoisted(() => vi.fn());
const uninstall = vi.hoisted(() => vi.fn());
/// The re-check (#961). Exported from `useClaudeHooks` for the first time by
/// that issue: before it, every caller was a `.finally` on one of the three
/// buttons that are disabled or hidden in the state that needs it most.
const reread = vi.hoisted(() => vi.fn());
const hookState = vi.hoisted(() => ({ status: undefined as ClaudeHooksStatus | undefined }));
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/p")));

vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));
vi.mock("@/api/hooks", () => ({
  // #1127. Disabled by default: the inventory section is collapsed and
  // does not read the file until opened, which is the behaviour every
  // test in this file assumes.
  useClaudeHookInventory: () => ({ data: undefined, error: null }),
  // #1130. Collapsed by default, so nothing is read until opened --
  // which is what these tests assume.
  useClaudeEffectiveSettings: () => ({ data: undefined, error: null }),
  useClaudeHooks: () => ({
    status: hookState.status,
    isLoading: hookState.status === undefined,
    error: null,
    install,
    reinstall,
    uninstall,
    reread,
  }),
}));
vi.mock("@/lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("@/api/tauri", () => ({ claudeRevealPath: revealFn }));
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false }));

import { ClaudeIntegrationsPanel, refusalPath, statusLine } from "./ClaudeIntegrationsPanel";

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
  toastError.mockClear();
  reread.mockClear();
  copyFn.mockClear().mockResolvedValue(null);
  revealFn.mockClear().mockResolvedValue("/p");
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
  ///
  /// And the INSTALL CONTROLS stay live, which is the half worth pinning. The
  /// epic asks for a greyed section and the tempting reading is to grey
  /// everything, but a panel that cannot uninstall while the view is off is a
  /// dead end: someone who turned the view off months ago still has a
  /// legitimate reason to remove a hook that is still writing to disk. Only
  /// the description of the hidden view is dimmed.
  it("offers the install controls even while the integrations are off", () => {
    panel();
    expect(screen.getByRole("checkbox", { name: /show claude code sessions/i })).toHaveProperty(
      "checked",
      false,
    );
    expect(screen.getByRole("button", { name: /install/i }).hasAttribute("disabled")).toBe(false);
    // The view description says it is currently hidden, rather than
    // describing a view the user cannot reach as though they could.
    expect(document.body.textContent).toMatch(/hidden while this is off/i);
  });

  /// With the switch ON, the "hidden while this is off" note is gone.
  ///
  /// Guards the guard above: a component that printed that sentence
  /// unconditionally would satisfy it while telling an enabled user their
  /// view is hidden.
  it("drops the hidden note once the integrations are on", () => {
    render(
      <ClaudeIntegrationsPanel
        prefs={{ ...PREFS, claude_integrations_enabled: true }}
        setPrefs={setPrefs}
      />,
    );
    expect(document.body.textContent).not.toMatch(/hidden while this is off/i);
    // The permanence warning is NOT conditional and must survive.
    expect(document.body.textContent).toMatch(/does\s*not\s*remove the hook/i);
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

/// #961: "Fix the JSON by hand, then install" has to name things that
/// exist.
///
/// The message ended with an instruction naming a `disabled` button, the
/// path was interpolated as bare text, and there was no re-check at all --
/// `reread` was unexported and every caller was a `.finally` on install,
/// reinstall or uninstall, which are the buttons that are disabled or hidden
/// in exactly this state.
describe("a settings file that cannot be read", () => {
  const MALFORMED: ClaudeHooksStatus = {
    state: "cannot_tell",
    kind: "malformed",
    path: "/Users/acme/.claude/settings.json",
    detail: "expected `,` at line 14 column 3",
  };

  /// **The one the issue is named after.** After hand-fixing the file there
  /// is a control that re-reads it.
  ///
  /// SABOTAGE: remove `reread` from `useClaudeHooks`'s returned object and
  /// this file stops compiling; leave it exported but drop the button and
  /// this test fails by name. Nothing else catches it -- every other test
  /// here exercises a state where one of the three write buttons is live,
  /// and `refetchOnWindowFocus` is invisible to a render test for the same
  /// reason it is invisible to a user.
  it("offers a re-check, and it re-reads rather than writing", () => {
    hookState.status = MALFORMED;
    panel();

    fireEvent.click(screen.getByRole("button", { name: /check again/i }));

    expect(reread).toHaveBeenCalledOnce();
    // And wrote NOTHING. The whole point of the disabled Install is that a
    // file we cannot parse must not be rewritten, so a "Check again" that
    // quietly installed would be the refusal defeated by its own remedy.
    expect(install).not.toHaveBeenCalled();
    expect(reinstall).not.toHaveBeenCalled();
    expect(uninstall).not.toHaveBeenCalled();
  });

  /// Install stays disabled, which is the load-bearing half of the
  /// three-state status.
  ///
  /// Asserted in the SAME test as the re-check, because the tempting fix for
  /// "the instruction names a disabled button" is to enable the button. It
  /// is not: a malformed file must not offer the control whose whole job is
  /// to refuse. The status changing is the only thing that should enable it.
  it("does not enable Install just because a re-check exists", () => {
    hookState.status = MALFORMED;
    panel();

    expect(screen.getByRole("button", { name: /^install$/i }).hasAttribute("disabled")).toBe(
      true,
    );
    fireEvent.click(screen.getByRole("button", { name: /check again/i }));
    expect(screen.getByRole("button", { name: /^install$/i }).hasAttribute("disabled")).toBe(
      true,
    );
    // And Reinstall and Remove stay HIDDEN, because we genuinely do not
    // know what is in the file.
    expect(screen.queryByRole("button", { name: /reinstall/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /remove/i })).toBeNull();
  });

  /// The reason the button is dead, in VISIBLE text and not only a tooltip.
  ///
  /// `RevealButton` argues this at length for the reveal buttons two views
  /// over: "the visible text is what a keyboard or screen-reader user gets,
  /// the title is what a mouse user reaching for a greyed control looks
  /// for". The disabled Install had neither, so the amber paragraph above
  /// and the grey button below were two things the reader had to connect.
  it("says why Install is dead, in text and in the title", () => {
    hookState.status = MALFORMED;
    panel();

    expect(document.body.textContent).toMatch(/install is unavailable while that file/i);
    expect(
      screen.getByRole("button", { name: /^install$/i }).getAttribute("title"),
    ).toMatch(/\.claude\/settings\.json/);
  });

  /// The path is copyable, and the copy reports its own failure.
  ///
  /// `copyText` distinguishes an insecure context from a rejected write, and
  /// the two have different remedies -- so "could not copy" alone would
  /// reproduce, one level down, the defect this whole change is fixing.
  it("copies the path, and names the reason when the copy fails", async () => {
    hookState.status = MALFORMED;
    panel();

    fireEvent.click(screen.getByRole("button", { name: /copy path/i }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledWith(MALFORMED.path));

    copyFn.mockResolvedValue("This window has no clipboard access.");
    fireEvent.click(screen.getByRole("button", { name: /copy path/i }));
    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(expect.stringMatching(/no clipboard access/i)),
    );
  });

  /// Reveal calls `claude_reveal_path` against the path the refusal named.
  it("reveals the settings file", async () => {
    hookState.status = MALFORMED;
    panel();

    fireEvent.click(screen.getByRole("button", { name: /reveal in finder/i }));
    await waitFor(() => expect(revealFn).toHaveBeenCalledWith(MALFORMED.path));
  });

  /// None of the three appears for a state where the file WAS read.
  ///
  /// A re-check beside a working status is furniture, and a "Copy path"
  /// there would need a path this status does not carry. `refusalPath`
  /// returning `null` is what makes that true by construction rather than by
  /// three conditions someone has to keep in step.
  it.each([
    ["installed", { state: "installed", command: "x" } as ClaudeHooksStatus],
    ["not_installed", { state: "not_installed" } as ClaudeHooksStatus],
    ["stale", { state: "stale", detail: "old path" } as ClaudeHooksStatus],
  ])("offers none of the three in the %s state", (_name, status) => {
    hookState.status = status;
    panel();

    expect(screen.queryByRole("button", { name: /check again/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /copy path/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /reveal in finder/i })).toBeNull();
    expect(document.body.textContent).not.toMatch(/install is unavailable while that file/i);
  });

  /// All four refusal kinds carry a path and get the controls, and the four
  /// SENTENCES stay distinct.
  ///
  /// The panel's own doc makes the same case `revealRefusal` does: a single
  /// shared "unavailable" string would collapse the kinds into the shrug the
  /// four-state exists to prevent. The buttons are shared; the sentence
  /// above them is not.
  it.each([
    ["malformed", MALFORMED],
    [
      "hooks_not_an_object",
      { state: "cannot_tell", kind: "hooks_not_an_object", path: "/p/a.json", found: "a string" },
    ],
    [
      "matcher_not_understood",
      {
        state: "cannot_tell",
        kind: "matcher_not_understood",
        path: "/p/b.json",
        event: "SessionStart",
      },
    ],
    [
      "io",
      { state: "cannot_tell", kind: "io", path: "/p/c.json", detail: "Permission denied" },
    ],
  ] as Array<[string, ClaudeHooksStatus]>)("offers the controls for the %s refusal", (_n, s) => {
    hookState.status = s;
    panel();

    expect(screen.getByRole("button", { name: /check again/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /copy path/i })).toBeTruthy();
    expect(refusalPath(s)).not.toBeNull();
  });

  /// The four refusal sentences are still four sentences.
  it("does not collapse the four refusals into one", () => {
    const texts = (
      [
        MALFORMED,
        { state: "cannot_tell", kind: "hooks_not_an_object", path: "/p", found: "a string" },
        { state: "cannot_tell", kind: "matcher_not_understood", path: "/p", event: "SessionStart" },
        { state: "cannot_tell", kind: "io", path: "/p", detail: "Permission denied" },
      ] as ClaudeHooksStatus[]
    ).map((s) => statusLine(s).text);
    expect(new Set(texts).size).toBe(4);
  });

  /// The re-check is NOT greyed by the integrations switch.
  ///
  /// The panel already argues that greying the install controls "would imply
  /// the switch disables them", and `:179` names a panel that cannot act
  /// while disabled as a dead end. A panel that cannot RE-CHECK while the
  /// view is off is the same dead end.
  it("re-checks even while the integrations are switched off", () => {
    hookState.status = MALFORMED;
    render(
      <ClaudeIntegrationsPanel
        prefs={{ ...PREFS, claude_integrations_enabled: false }}
        setPrefs={setPrefs}
      />,
    );

    const button = screen.getByRole("button", { name: /check again/i });
    expect(button.hasAttribute("disabled")).toBe(false);
    fireEvent.click(button);
    expect(reread).toHaveBeenCalledOnce();
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


/// #1127: the hooks the app never showed.
describe("the hook inventory", () => {
  it("is collapsed until asked for", () => {
    render(<ClaudeIntegrationsPanel prefs={{ ...PREFS, claude_integrations_enabled: true }} setPrefs={async () => {}} />);
    const toggle = screen.getByRole("button", { name: /every hook in this file/i });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
  });

  it("offers to show every hook, not only ours", () => {
    render(<ClaudeIntegrationsPanel prefs={{ ...PREFS, claude_integrations_enabled: true }} setPrefs={async () => {}} />);
    expect(screen.getByRole("button", { name: /every hook in this file/i })).toBeTruthy();
  });

  /// The section is gated on the integration being enabled, so a user
  /// who has not turned it on is not offered a view of a file the app
  /// is not otherwise reading.
  it("is absent when the integration is off", () => {
    render(<ClaudeIntegrationsPanel prefs={{ ...PREFS, claude_integrations_enabled: false }} setPrefs={async () => {}} />);
    expect(screen.queryByRole("button", { name: /every hook in this file/i })).toBeNull();
  });
});
