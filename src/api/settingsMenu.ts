import { useEffect } from "react";
import { listen } from "./transport";
import { safeUnlisten } from "./unlisten";
import { IS_DESKTOP_BUILD } from "../lib/target";
import { useSettingsDialog } from "../store/settingsDialog";

/** The menu bar's Settings… (#2084). The phone has no menu bar. */
export function useSettingsMenu() {
  useEffect(() => {
    if (!IS_DESKTOP_BUILD) return;
    let active = true;
    let stop: (() => void) | undefined;
    void listen("open-settings", () => { if (active) useSettingsDialog.getState().open(); }).then(
      unlisten => { if (active) stop = unlisten; else safeUnlisten(unlisten); },
      (error: unknown) => console.error("Could not listen for the Settings menu item", error),
    );
    return () => { active = false; if (stop) safeUnlisten(stop); };
  }, []);
}
