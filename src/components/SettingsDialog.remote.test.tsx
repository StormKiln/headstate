import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setRemote = vi.fn<(enabled: boolean) => Promise<void>>(() => Promise.resolve());
const remoteState = { enabled: false };

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
  useUiPrefs: () => ({
    prefs: { hidden_views: [], close_hides_to_tray: true },
    set: () => Promise.resolve(),
  }),
  useCleanupPrefs: () => ({ prefs: undefined, set: () => Promise.resolve() }),
  useAutostart: () => ({ enabled: false, set: () => Promise.resolve() }),
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
  useNotifyPrefs: () => ({
    prefs: { enabled: true, ci_failed: true, conflicted: true },
    set: () => Promise.resolve(),
  }),
  useRemoteEnabled: () => ({ enabled: remoteState.enabled, set: setRemote }),
  useIssuePairingToken: () => () => Promise.reject("not in this test"),
  usePairedDevices: () => ({ data: [], isLoading: false, error: null }),
  useRevokePairedDevice: () => () => Promise.resolve(),
}));

import { SettingsDialog } from "./SettingsDialog";

beforeEach(() => {
  setRemote.mockClear();
  setRemote.mockImplementation(() => Promise.resolve());
  remoteState.enabled = false;
});
afterEach(cleanup);

const show = () => render(<SettingsDialog open onOpenChange={() => {}} />);
const box = () => screen.getByRole("checkbox", { name: /allow phone connections/i });

/// The listener opens a port on every interface. It must be off until
/// the user says otherwise, and the switch must say what it does.
describe("phone connections setting", () => {
  it("mounts the away-from-home guide with phone connections still off", () => {
    show();
    expect(screen.getByRole("region", { name: "Use away from home" })).toBeTruthy();
    expect(setRemote).not.toHaveBeenCalled();
  });
  it("offers the switch, off by default", () => {
    show();
    expect(box()).toHaveProperty("checked", false);
  });

  it("has its own topic in the left rail", () => {
    show();
    expect(screen.getByRole("button", { name: /^phone$/i })).toBeTruthy();
  });

  it("turns the listener on", () => {
    show();
    fireEvent.click(box());
    expect(setRemote).toHaveBeenCalledWith(true);
  });

  it("turns the listener off when it is on", () => {
    remoteState.enabled = true;
    show();
    expect(box()).toHaveProperty("checked", true);
    fireEvent.click(box());
    expect(setRemote).toHaveBeenCalledWith(false);
  });

  // Binding a port or writing the keychain can genuinely refuse; the
  // reason must reach the user rather than the box silently staying
  // where it was.
  it("shows the backend's error when the change is refused", async () => {
    let refuse!: (reason: string) => void;
    const pending = new Promise<void>((_, reject) => { refuse = reject; });
    setRemote.mockReturnValueOnce(pending);
    show();
    const phone = screen.getByRole("button", { name: /^phone$/i });
    fireEvent.click(phone);
    expect(phone.getAttribute("aria-current")).toBe("page");
    expect(box().closest(".hidden")).toBeNull();
    fireEvent.click(box());
    expect(setRemote).toHaveBeenCalledExactlyOnceWith(true);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(box()).toHaveProperty("checked", false);

    // This test owns the backend promise. Flush its actual rejection and
    // React's catch-driven render before querying, rather than polling the
    // entire dialog against an unrelated one-second arrival deadline.
    await act(async () => {
      refuse("could not listen on 0.0.0.0:41919: address already in use");
    });
    expect(screen.getByRole("alert")).toHaveProperty(
      "textContent",
      "could not listen on 0.0.0.0:41919: address already in use",
    );
    expect(box()).toHaveProperty("checked", false);
    expect(setRemote).toHaveBeenCalledExactlyOnceWith(true);
  });

  it("names the port, so the user knows what was opened", () => {
    show();
    expect(screen.getByText(/Lets the Headstate companion app reach this desktop on port 41919/)).toBeTruthy();
  });

  // Same topic as the switch: pairing is the only reason to turn it on.
  it("offers pairing and the paired-device list on the same topic", () => {
    show();
    expect(screen.getByRole("button", { name: /pair a phone/i })).toBeTruthy();
    expect(screen.getByText(/paired devices/i)).toBeTruthy();
  });
});
