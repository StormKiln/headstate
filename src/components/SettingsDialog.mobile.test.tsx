import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/// The phone layout of Settings, which nothing else covers.
///
/// `useIsMobile` reads `matchMedia`, which jsdom does not implement, so
/// every other SettingsDialog test renders the DESKTOP layout. Without
/// this file the phone's list navigation (#650) would ship untested --
/// and it is the half that changed.
vi.mock("@/lib/useIsMobile", () => ({
  useIsMobile: () => true,
  MOBILE_BREAKPOINT: 768,
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../api/tauri", () => ({ revealLog: vi.fn() }));
// The same shape SettingsDialog.test.tsx mocks: the dialog reads half a
// dozen preference hooks, none of which this file is about.
vi.mock("../api/hooks", () => ({
  useUiPrefs: () => ({
    prefs: { hidden_views: [], close_hides_to_tray: true, diagnostic_logging: false },
    set: () => Promise.resolve(),
  }),
  useCleanupPrefs: () => ({ prefs: {}, set: () => Promise.resolve() }),
  useAutostart: () => ({ enabled: false, set: () => Promise.resolve() }),
  useRemoteEnabled: () => ({ enabled: false, set: () => Promise.resolve() }),
  useIssuePairingToken: () => () => Promise.reject("not in this test"),
  usePairedDevices: () => ({ data: [], isLoading: false, error: null }),
  useRevokePairedDevice: () => () => Promise.resolve(),
  usePollInterval: () => ({ seconds: 120, set: () => Promise.resolve() }),
  useWorktreeDirs: () => ({ dirs: [], set: () => Promise.resolve() }),
  // #915's panel. Status is a `Class::Read` and so is present on the phone;
  // the three install actions are `Class::Local` and the panel hides their
  // buttons behind `IS_MOBILE_BUILD`, which is why they are never called here.
  useClaudeHooks: () => ({
    status: { state: "not_installed" },
    isLoading: false,
    error: null,
    install: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    reinstall: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    uninstall: () => Promise.resolve({ removed: [], was_absent: true }),
  }),
  useNotifyPrefs: () => ({
    prefs: { enabled: true, ci_failed: true, conflicted: true },
    set: () => Promise.resolve(),
  }),
}));

import { SettingsDialog } from "./SettingsDialog";

describe("SettingsDialog on a phone", () => {
  beforeEach(() => vi.clearAllMocks());

  const open = (initialSection?: "general" | "phone") =>
    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        {...(initialSection ? { initialSection } : {})}
      />,
    );

  it("lists the sections vertically instead of a sideways strip", () => {
    open();
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    // The bug: `overflow-x-auto` made sections past the third
    // undiscoverable, with no affordance saying they existed.
    expect(nav.className).not.toContain("overflow-x-auto");
    expect(nav.className).toContain("flex-col");
  });

  it("pushes to a section and offers a way back", () => {
    open();
    const navAfter0 = () =>
      screen.getByRole("navigation", { name: /settings sections/i }).className;
    // Opens on a section, as the desktop does, so the list starts hidden.
    expect(navAfter0()).toContain("hidden");
    // Back to the list first, which is the state a phone user lands in
    // after tapping Settings from the banner and then backing out.
    fireEvent.click(screen.getByRole("button", { name: /^settings$/i }));
    expect(navAfter0()).not.toContain("hidden");

    fireEvent.click(screen.getByRole("button", { name: /^phone$/i }));
    // Re-queried, not reused: the className is read off whatever node
    // is mounted now, so a stale reference cannot make this pass.
    const navAfter = () =>
      screen.getByRole("navigation", { name: /settings sections/i }).className;
    // The list gives way to the section, as an iOS push would.
    expect(navAfter()).toContain("hidden");
    fireEvent.click(screen.getByRole("button", { name: /^settings$/i }));
    expect(navAfter()).not.toContain("hidden");
  });

  it("opens straight to a section when asked, with the way back", () => {
    // `ConnectionBanner` opens Settings on "phone". That must still land
    // on the section, not on the list.
    open("phone");
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    expect(nav.className).toContain("hidden");
    expect(screen.getByRole("button", { name: /^settings$/i })).toBeTruthy();
  });

  it("keeps every section's controls mounted for a screen reader", () => {
    // The panels are hidden with CSS, never unmounted and never with the
    // `hidden` ATTRIBUTE -- unmounting loses control state, and the
    // attribute removes them from the accessibility tree.
    open();
    for (const label of ["General", "Phone", "Notifications"]) {
      expect(screen.getByRole("button", { name: new RegExp(`^${label}$`, "i") })).toBeTruthy();
    }
  });
});

