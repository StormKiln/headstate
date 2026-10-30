import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useSettingsDialog } from "@/store/settingsDialog";
import { settle } from "@/test/settle";

vi.mock("../api/tauri", () => ({ latestRelease: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("2.0.2") }));
vi.mock("../api/hooks", () => ({
  useBackgroundPanicked: () => false,
  useUiPrefs: () => ({ prefs: { hidden_views: [], close_hides_to_tray: true }, set: () => Promise.resolve() }),
  usePollState: () => "idle",
  useRemovalProgress: () => null,
  useUpdateProgress: () => null,
  useCancelUpdateRun: () => () => Promise.resolve(),
  usePollError: () => null,
  usePollInterval: () => ({ seconds: 120, set: () => Promise.resolve(120) }),
}));
// The dialog itself is tested on its own; here only WHICH topic it opens on.
vi.mock("./SettingsDialog", () => ({
  SettingsDialog: ({ initialSection, onOpenChange }: { initialSection: string; onOpenChange: (open: boolean) => void }) => (
    <div role="dialog" aria-label="Settings">
      {initialSection}
      <button type="button" onClick={() => onOpenChange(false)}>Close</button>
    </div>
  ),
}));

import { StatusBar } from "./StatusBar";

/// The gear, the menu bar's Settings… (#2084) and "Open Jira settings"
/// (#2085) share one dialog, each opening it at its own topic.
describe("StatusBar's Settings dialog", () => {
  afterEach(() => act(() => useSettingsDialog.getState().close()));

  it("opens on General from the gear, and closes", async () => {
    render(<StatusBar updatedAt={Date.now()} />);
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("dialog").textContent).toContain("general");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(useSettingsDialog.getState().section).toBeNull();
  });

  it("opens on the topic a request names, even over another topic", async () => {
    render(<StatusBar updatedAt={Date.now()} />);
    await settle();
    act(() => useSettingsDialog.getState().open());
    expect(screen.getByRole("dialog").textContent).toContain("general");
    act(() => useSettingsDialog.getState().open("jira"));
    expect(screen.getByRole("dialog").textContent).toContain("jira");
  });
});
