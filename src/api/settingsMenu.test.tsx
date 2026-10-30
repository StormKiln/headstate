import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => {
  const listeners: Array<{ event: string; handler: (event: { payload: unknown }) => void }> = [];
  return { listeners, unlisten: vi.fn(), desktop: true };
});
vi.mock("../lib/target", () => ({ get IS_DESKTOP_BUILD() { return ipc.desktop; } }));
vi.mock("./transport", () => ({
  listen: (event: string, handler: (event: { payload: unknown }) => void) => {
    ipc.listeners.push({ event, handler });
    return Promise.resolve(ipc.unlisten);
  },
}));

import { useSettingsMenu } from "./settingsMenu";
import { useSettingsDialog } from "../store/settingsDialog";

afterEach(() => {
  ipc.listeners.length = 0;
  ipc.unlisten.mockReset();
  ipc.desktop = true;
  useSettingsDialog.getState().close();
});

describe("the menu bar's Settings… (#2084)", () => {
  it("opens Settings, and stops listening on unmount", async () => {
    const { unmount } = renderHook(() => useSettingsMenu());
    await act(() => Promise.resolve());
    expect(ipc.listeners.map(({ event }) => event)).toEqual(["open-settings"]);
    act(() => ipc.listeners[0].handler({ payload: null }));
    expect(useSettingsDialog.getState().section).toBe("general");
    unmount();
    expect(ipc.unlisten).toHaveBeenCalledOnce();
  });

  it("does not listen on the phone, which has no menu bar", () => {
    ipc.desktop = false;
    renderHook(() => useSettingsMenu());
    expect(ipc.listeners).toHaveLength(0);
  });
});
