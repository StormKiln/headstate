import { create } from "zustand";
import type { SectionId } from "@/components/settingsSections";

/// Which Settings topic is open, or `null` when Settings is closed. Its own
/// store so the status bar's gear, the menu bar's Settings… (#2084) and a
/// page's "Open Jira settings" (#2085) all open the one dialog the status
/// bar mounts, each at the topic it is about.
interface SettingsDialogState {
  section: SectionId | null;
  open: (section?: SectionId) => void;
  close: () => void;
}

export const useSettingsDialog = create<SettingsDialogState>()((set) => ({
  section: null,
  open: (section = "general") => set({ section }),
  close: () => set({ section: null }),
}));
