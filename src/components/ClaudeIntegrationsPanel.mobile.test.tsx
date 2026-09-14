import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ClaudeHooksStatus, UiPrefs } from "@/api/tauri";
import { stubViewport } from "@/test-utils";

/// #961's controls on the companion, and which of the three the phone gets.
///
/// | control | class | phone |
/// |---|---|---|
/// | Check again | `claude_hooks_status` is `Read` | yes |
/// | Copy path | no command at all | yes |
/// | Reveal in Finder | `claude_reveal_path` is `Class::Local` | NO |
///
/// `surfaceGuard.test.ts` already proves `claude_reveal_path` is `Local` and
/// that the phone may not call it -- but that is a check on the ALLOWLIST,
/// not on what gets RENDERED. A panel that draws a button the allowlist
/// refuses still ships a control that can only fail, and the allowlist test
/// stays green the whole time. This file is the other half.
///
/// Its sibling, `ClaudeIntegrationsPanel.test.tsx`, mocks `IS_MOBILE_BUILD`
/// to `false` and is where the desktop's three buttons are asserted. Two
/// files rather than a flag, because `IS_MOBILE_BUILD` is a Vite `define`
/// that folds to a literal: there is no environment variable to flip at test
/// time, so the build has to be chosen per module graph.
///
/// # And at a PHONE VIEWPORT
///
/// `IS_MOBILE_BUILD` says which BUILD this is; `matchMedia` is what
/// `useIsMobile()` reads to decide whether the narrow LAYOUT is on, and jsdom
/// provides none -- so a file that mocks the first and not the second runs at
/// desktop width while claiming to be a phone. That bug was real in
/// `ClaudeCodePage.mobile.test.tsx` and was fixed by adding `stubViewport`.
///
/// This panel does not branch on `useIsMobile()` today, which is exactly why
/// the viewport is stubbed and ASSERTED below rather than left out: a later
/// change that adds a width branch would otherwise be tested at the wrong
/// width by a file whose name says phone.
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

const install = vi.hoisted(() => vi.fn());
const reinstall = vi.hoisted(() => vi.fn());
const uninstall = vi.hoisted(() => vi.fn());
const reread = vi.hoisted(() => vi.fn());
const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/p")));
const hookState = vi.hoisted(() => ({ status: undefined as ClaudeHooksStatus | undefined }));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/api/hooks", () => ({
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

const { ClaudeIntegrationsPanel } = await import("./ClaudeIntegrationsPanel");

const PREFS: UiPrefs = {
  hidden_views: [],
  close_hides_to_tray: true,
  announce_updates: true,
  claude_integrations_enabled: true,
  diagnostic_logging: false,
  stale_venv_days: 0,
  battery_low_percent: 0,
};

const MALFORMED: ClaudeHooksStatus = {
  state: "cannot_tell",
  kind: "malformed",
  path: "/Users/acme/.claude/settings.json",
  detail: "expected `,` at line 14 column 3",
};

const panel = () =>
  render(<ClaudeIntegrationsPanel prefs={PREFS} setPrefs={vi.fn(() => Promise.resolve())} />);

beforeEach(() => {
  // 390px: an iPhone 15's CSS width, comfortably under `MOBILE_BREAKPOINT`.
  stubViewport(390);
  hookState.status = MALFORMED;
  reread.mockClear();
  copyFn.mockClear().mockResolvedValue(null);
  revealFn.mockClear();
});

afterEach(() => {
  stubViewport(null);
  cleanup();
});

describe("the companion's broken-settings controls", () => {
  /// The viewport really is a phone's. Asserted rather than assumed, because
  /// a file that stubs the build and not the width tests the desktop tree
  /// while claiming otherwise -- which is the bug this file's doc comment
  /// describes.
  it("runs at a phone width", () => {
    expect(window.matchMedia("(max-width: 767px)").matches).toBe(true);
    expect(window.matchMedia("(max-width: 389px)").matches).toBe(false);
  });

  /// **The sabotage test.** Drop the `IS_MOBILE_BUILD` guard around the
  /// reveal button and this fails, naming it.
  it("renders no reveal button, because claude_reveal_path is Class::Local", () => {
    panel();
    expect(screen.queryByRole("button", { name: /reveal in finder/i })).toBeNull();
  });

  /// Never CALLED either, which is a different claim from not rendered: an
  /// effect or a shortcut could reach the wrapper with no button on screen.
  it("never calls claudeRevealPath", () => {
    panel();
    expect(revealFn).not.toHaveBeenCalled();
  });

  /// An absent button with no explanation leaves the reader unable to tell
  /// "this app has no such action" from "not from here". `RevealButton` makes
  /// this argument at length; the sentence is the phone's version of it.
  it("says why the reveal is missing rather than leaving a gap", () => {
    panel();
    expect(document.body.textContent).toMatch(/a finder this phone cannot see/i);
    expect(document.body.textContent).toMatch(/opened at that mac/i);
    // And points at the one control that DOES help from here, rather than
    // only naming what is absent.
    expect(document.body.textContent).toMatch(/the path above copies here/i);
  });

  /// The two that DO work on the phone are offered.
  ///
  /// This is the half that would be lost if someone "fixed" the hidden
  /// reveal by hiding the whole block. `claude_hooks_status` is `Read` and
  /// `surface.rs` argues that reading status remotely is "both harmless and
  /// useful"; `copyText` is no command at all.
  it("still offers the re-check and the copy", () => {
    panel();

    fireEvent.click(screen.getByRole("button", { name: /check again/i }));
    expect(reread).toHaveBeenCalledOnce();

    expect(screen.getByRole("button", { name: /copy path/i })).toBeTruthy();
  });

  /// Copy is the one of the three that helps most from another room: a path
  /// the user can send themselves. So it really copies, rather than being a
  /// button that renders on the phone and does nothing.
  it("copies the path to the phone's own clipboard", async () => {
    panel();
    fireEvent.click(screen.getByRole("button", { name: /copy path/i }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledWith(MALFORMED.path));
  });

  /// The install controls stay off the phone, as they were before #961.
  ///
  /// All three are `Class::Local` and would be refused before reaching the
  /// wire. Asserted here because the new controls sit in the same block, and
  /// a fix that moved the `IS_MOBILE_BUILD` boundary to let them through
  /// would ship three buttons that cannot work.
  it("still hides the install controls", () => {
    panel();
    expect(screen.queryByRole("button", { name: /^install$/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /reinstall/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /remove/i })).toBeNull();
    // And says why, which was already true and must stay so.
    expect(document.body.textContent).toMatch(/done at that mac rather than from here/i);
  });

  /// The refusal itself still reaches the phone, loudly.
  ///
  /// This message is "the ONLY warning the user will ever get that their
  /// hooks are dead", and dead hooks mean every other tool's hooks in that
  /// file are dead too. It is a `Read`, so the phone gets it.
  it("still shows the refusal, with its position", () => {
    panel();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toMatch(/line 14 column 3/);
    expect(alert.textContent).toMatch(/silently ignores/i);
  });
});
