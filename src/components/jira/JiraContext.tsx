import { useMemo, type ReactNode } from "react";
import type { UiPrefs } from "@/api/tauri";
import { useJiraConnection, useJiraLinks } from "@/api/jira";
import { linkMaps } from "@/lib/jira/links";
import { JiraContext, type JiraState } from "./jiraState";

/// The app-wide sign-in check is slow-polled: Settings › Jira polls briskly
/// while open, and this only has to notice a sign-in eventually.
const APP_POLL_MS = 5 * 60_000;

/// Decides `JiraState` once for the whole shell. See `jiraState.ts`.
///
/// Keys are found and kept by the desktop (`jira/index.rs`), which applies
/// the project list and Settings' exclusions; this only reads the result.
export function JiraProvider({ prefs, children }: { prefs: UiPrefs | undefined; children: ReactNode }) {
  const enabled = prefs?.jira_enabled ?? false;
  const connection = useJiraConnection(enabled, APP_POLL_MS);
  const signedIn = connection.data?.state === "logged_in";
  const links = useJiraLinks(enabled && signedIn);
  const site = connection.data?.state === "logged_in" ? connection.data.site : null;
  const value = useMemo<JiraState | null>(() => {
    if (!enabled || !signedIn || !links.data || links.data.error !== null) return null;
    return { projects: new Set(links.data.projects), site, links: linkMaps(links.data.links) };
  }, [enabled, signedIn, links.data, site]);
  return <JiraContext.Provider value={value}>{children}</JiraContext.Provider>;
}
