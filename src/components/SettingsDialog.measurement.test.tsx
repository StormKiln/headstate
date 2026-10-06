import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ status: vi.fn(), save: vi.fn() }));
vi.mock("../api/transport", () => ({ call: (name: string) => {
  if (name === "measurement_status") return calls.status();
  if (name === "measurement_export") return calls.save();
  return Promise.resolve(undefined);
} }));
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
  calls.status.mockReset().mockResolvedValue({ enabled: false, schema: 1, epochs: 1,
    oldestWallMs: 1000, newestWallMs: 2000, durableRecords: 2, bytes: 400,
    dropped: 0, invalid: 0, coalesced: 0, rotatedOut: 0, writerState: "disabled", incomplete: false });
  calls.save.mockReset().mockResolvedValue({ canceled: false, records: 2, bytes: 500, incomplete: false });
});
afterEach(cleanup);
const showGeneral = () => {
  render(<SettingsDialog open onOpenChange={() => {}} initialSection="phone" />);
  fireEvent.click(screen.getByRole("button", { name: /^general$/i }));
};
it("mounts the separate measurement export in the actual General section", async () => {
  render(<SettingsDialog open onOpenChange={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: /^general$/i }));
  expect(screen.getByRole("button", { name: /save measurement report/i }).closest(".hidden")).toBeNull();
});

it("uses actual commands for disabled retained capture, pending save, cancel and retry", async () => {
  showGeneral();
  await screen.findByText("Measurement capture off. 2 retained records.");
  let resolve!: (value: unknown) => void;
  calls.save.mockReturnValueOnce(new Promise(r => { resolve = r; }));
  fireEvent.click(screen.getByRole("button", { name: /save measurement report/i }));
  expect(calls.save).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: /saving measurement report/i })).toHaveProperty("disabled", true);
  await act(async () => resolve({ canceled: true, records: 0, bytes: 0, incomplete: false }));
  expect(await screen.findByText(/Save canceled/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /save measurement report/i }));
  expect(await screen.findByText("Saved 2 measurement records.")).toBeTruthy();
});
it("keeps status failure separate from empty capture and exposes save refusal without private errors", async () => {
  calls.status.mockRejectedValueOnce("PRIVATE_HOST/path"); showGeneral();
  expect(await screen.findByText(/Measurement status unavailable/)).toBeTruthy();
  expect(screen.queryByText(/0 retained records/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /retry measurement status/i }));
  await screen.findByText(/2 retained records/);
  calls.save.mockRejectedValueOnce("PRIVATE_HOST/path");
  fireEvent.click(screen.getByRole("button", { name: /save measurement report/i }));
  expect(await screen.findByText(/Could not save the measurement report/)).toBeTruthy();
  expect(screen.queryByText(/PRIVATE_HOST/)).toBeNull();
});
it("does not query measurement status in a hidden General section", () => {
  render(<SettingsDialog open onOpenChange={() => {}} initialSection="phone" />);
  expect(calls.status).not.toHaveBeenCalled();
});
