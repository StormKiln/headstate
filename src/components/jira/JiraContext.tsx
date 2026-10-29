import type { ReactNode } from "react";
import type { UiPrefs } from "@/api/tauri";
import { JiraContext } from "./jiraState";
import { useJiraValue } from "./useJiraValue";

/// Decides `JiraState` once for the whole shell. See `jiraState.ts`.
///
/// Keys are found and kept by the desktop (`jira/index.rs`), which applies
/// the project list and Settings' exclusions; this only reads the result.
export function JiraProvider({ prefs, children }: { prefs: UiPrefs | undefined; children: ReactNode }) {
  const value = useJiraValue(prefs);
  return <JiraContext.Provider value={value}>{children}</JiraContext.Provider>;
}
