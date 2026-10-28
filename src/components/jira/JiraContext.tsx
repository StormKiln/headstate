import { useMemo, type ReactNode } from "react";
import type { UiPrefs } from "@/api/tauri";
import { useJiraConnection, useJiraProjects } from "@/api/jira";
import { JiraContext, type JiraState } from "./jiraState";

/// The app-wide sign-in check is slow-polled: Settings › Jira polls briskly
/// while open, and this only has to notice a sign-in eventually.
const APP_POLL_MS = 5 * 60_000;

/// Decides `JiraState` once for the whole shell. See `jiraState.ts`.
export function JiraProvider({ prefs, children }: { prefs: UiPrefs | undefined; children: ReactNode }) {
  const enabled = prefs?.jira_enabled ?? false;
  const connection = useJiraConnection(enabled, APP_POLL_MS);
  const signedIn = connection.data?.state === "logged_in";
  const projects = useJiraProjects(enabled && signedIn);
  const site = connection.data?.state === "logged_in" ? connection.data.site : null;
  const excluded = prefs?.jira_excluded_projects;
  const value = useMemo<JiraState | null>(() => {
    if (!enabled || !signedIn || !projects.data || projects.data.error !== null) return null;
    const skip = new Set(excluded ?? []);
    return {
      projects: new Set(projects.data.projects.map((p) => p.key).filter((k) => !skip.has(k))),
      site,
    };
  }, [enabled, signedIn, projects.data, excluded, site]);
  return <JiraContext.Provider value={value}>{children}</JiraContext.Provider>;
}
