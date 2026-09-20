import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setPrefs = vi.fn(() => Promise.resolve());
const prefsState: { prefs: unknown } = { prefs: undefined };

vi.mock("../api/hooks", () => ({
  // #1154. Undefined renders nothing, which is what these tests assume:
  // a failed probe must not draw "not found" for every tool.
  useToolVersions: () => ({ data: undefined, isError: false }),
  // #1127. The inventory section is collapsed and reads nothing until
  // opened, which is what these tests assume.
  useClaudeHookInventory: () => ({ data: undefined, error: null }),
  // #1130. Collapsed by default, so nothing is read until opened --
  // which is what these tests assume.
  useClaudeEffectiveSettings: () => ({ data: undefined, error: null }),
  useClaudeConfigHealth: () => ({
    data: undefined,
    error: null,
    isFetching: false,
    refetch: () => Promise.resolve(),
  }),
  // Defaults, matching the Rust side: nothing hidden, close hides.
  useUiPrefs: () => ({
    prefs: { hidden_views: [], close_hides_to_tray: true },
    set: () => Promise.resolve(),
  }),
  useCleanupPrefs: () => ({ prefs: undefined, set: () => Promise.resolve() }),
  useAutostart: () => ({ enabled: false, set: () => Promise.resolve() }),
  useRemoteEnabled: () => ({ enabled: false, set: () => Promise.resolve() }),
  useIssuePairingToken: () => () => Promise.reject("not in this test"),
  usePairedDevices: () => ({ data: [], isLoading: false, error: null }),
  useRevokePairedDevice: () => () => Promise.resolve(),
  usePollInterval: () => ({ seconds: 120, set: vi.fn() }),
  useWorktreeDirs: () => ({ dirs: [], set: vi.fn(() => Promise.resolve()) }),
  // #915's panel, which this file does not exercise but does mount.
  useClaudeHooks: () => ({
    status: { state: "not_installed" },
    isLoading: false,
    error: null,
    install: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    reinstall: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    uninstall: () => Promise.resolve({ removed: [], was_absent: true }),
  }),
  useNotifyPrefs: () => ({ prefs: prefsState.prefs, set: setPrefs }),
}));

import { SettingsDialog } from "./SettingsDialog";

beforeEach(() => {
  setPrefs.mockClear();
  prefsState.prefs = { enabled: true, ci_failed: true, conflicted: true };
});
afterEach(cleanup);

const show = () => render(<SettingsDialog open onOpenChange={() => {}} />);

/// Notifications were the only interruption channel in the app with no
/// off switch -- not in Settings, not anywhere. The only escape was
/// denying permission at the OS level, which the poll loop treats as
/// permanent.
describe("notification settings", () => {
  it("offers a master switch and one toggle per kind", () => {
    show();
    expect(screen.getByRole("checkbox", { name: /desktop notifications/i })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /ci/i })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: /conflict/i })).toBeTruthy();
  });

  it("turns everything off through the master switch", () => {
    show();
    fireEvent.click(screen.getByRole("checkbox", { name: /desktop notifications/i }));
    expect(setPrefs).toHaveBeenCalledWith({
      enabled: false,
      ci_failed: true,
      conflicted: true,
    });
  });

  // Turning the master switch off must not discard the per-kind choices,
  // so turning it back on restores what the user picked.
  it("keeps the per-kind choices when the master switch goes off", () => {
    prefsState.prefs = { enabled: true, ci_failed: true, conflicted: false };
    show();
    fireEvent.click(screen.getByRole("checkbox", { name: /desktop notifications/i }));
    expect(setPrefs).toHaveBeenCalledWith({
      enabled: false,
      ci_failed: true,
      conflicted: false,
    });
  });

  it("silences one kind without touching the other", () => {
    show();
    fireEvent.click(screen.getByRole("checkbox", { name: /conflict/i }));
    expect(setPrefs).toHaveBeenCalledWith({
      enabled: true,
      ci_failed: true,
      conflicted: false,
    });
  });

  // With the master switch off, the per-kind boxes must not imply they
  // still do something.
  it("disables the per-kind toggles when notifications are off", () => {
    prefsState.prefs = { enabled: false, ci_failed: true, conflicted: true };
    show();
    expect(screen.getByRole("checkbox", { name: /ci/i })).toHaveProperty("disabled", true);
  });

  // The behaviour is otherwise undiscoverable: nothing in the UI said
  // the app sends notifications at all.
  /// The wording was "only when a pull request newly BREAKS", which was
  /// accurate while every notification was breakage. Ready-for-review
  /// (#436) is good news, so the promise is now about the TRANSITION
  /// rather than the direction -- and that is the part users rely on:
  /// no repeats, and nothing on first launch.
  it("says notifications fire on a change, not repeatedly", () => {
    show();
    expect(screen.getByText(/only when something newly changes/i)).toBeTruthy();
    expect(screen.getByText(/never on first launch/i)).toBeTruthy();
  });
});

/// #789 gave the app two categories it had never had: a pull request
/// APPEARING, and this machine's health.
describe("the new-PR and health categories (#789)", () => {
  beforeEach(() => {
    prefsState.prefs = {
      enabled: true,
      ci_failed: true,
      conflicted: true,
      ready_to_review: true,
      new_pr: true,
      health_battery: true,
      health_cpu: true,
    };
  });

  it("offers a toggle for a pull request appearing", () => {
    show();
    const box = screen.getByRole("checkbox", { name: /a pull request appears/i });
    fireEvent.click(box);
    expect(setPrefs).toHaveBeenCalledWith(
      expect.objectContaining({ new_pr: false, ready_to_review: true }),
    );
  });

  /// **The gap #789 closed.** Until now the battery alerts notified
  /// unconditionally -- only the threshold was adjustable -- so a user
  /// who wanted pull-request notifications and not machine ones had no
  /// way to say so.
  it("offers separate toggles for battery and CPU", () => {
    show();
    fireEvent.click(screen.getByRole("checkbox", { name: /battery problems/i }));
    expect(setPrefs).toHaveBeenCalledWith(
      expect.objectContaining({ health_battery: false, health_cpu: true }),
    );

    setPrefs.mockClear();
    fireEvent.click(screen.getByRole("checkbox", { name: /cpu is busy/i }));
    expect(setPrefs).toHaveBeenCalledWith(
      expect.objectContaining({ health_cpu: false, health_battery: true }),
    );
  });

  /// A threshold for an alert that cannot fire is a control that does
  /// nothing, and leaving it live would imply the alert was still
  /// coming.
  it("disables the battery threshold when battery notifications are off", () => {
    prefsState.prefs = {
      enabled: true,
      ci_failed: true,
      conflicted: true,
      ready_to_review: true,
      new_pr: true,
      health_battery: false,
      health_cpu: true,
    };
    show();
    expect(
      screen.getByRole("spinbutton", { name: /battery charge warning threshold/i }),
    ).toHaveProperty("disabled", true);
  });

  /// The CPU row must say what it looks for. "Tell me when my machine is
  /// busy" is something nobody wants and is not what the rule does: the
  /// aggregate shape is the whole point, because ten runaway processes
  /// each look unremarkable in a sorted list.
  it("says the CPU alert is about load nothing explains", () => {
    show();
    expect(screen.getByText(/no single process accounts for/i)).toBeTruthy();
  });

  it("silences both health categories through the master switch", () => {
    prefsState.prefs = {
      enabled: false,
      ci_failed: true,
      conflicted: true,
      ready_to_review: true,
      new_pr: true,
      health_battery: true,
      health_cpu: true,
    };
    show();
    for (const name of [/battery problems/i, /cpu is busy/i, /a pull request appears/i]) {
      expect(screen.getByRole("checkbox", { name })).toHaveProperty("disabled", true);
    }
  });
});
