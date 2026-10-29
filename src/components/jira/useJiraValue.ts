import { useMemo } from "react";
import type { UiPrefs } from "@/api/tauri";
import { useJiraConnection, useJiraLinks } from "@/api/jira";
import { DEFAULT_MERGE_GRACE_MINS, type ExpectationMap } from "@/lib/jira/drift";
import { linkMaps } from "@/lib/jira/links";
import { noTicketIndex, type NoTicketIndex } from "@/lib/jira/noTicket";
import type { JiraState } from "./jiraState";

/// The app-wide sign-in check is slow-polled: Settings › Jira polls briskly
/// while open, and this only has to notice a sign-in eventually.
const APP_POLL_MS = 5 * 60_000;

const NO_EXPECTATIONS: ExpectationMap = {};

/// `JiraState` from the preferences and the desktop's index; see
/// `jiraState.ts`. Keys are found and kept by the desktop (`jira/index.rs`),
/// which applies the project list and Settings' exclusions; this only reads
/// the result.
export function useJiraValue(prefs: UiPrefs | undefined): JiraState | null {
  const enabled = prefs?.jira_enabled ?? false;
  const connection = useJiraConnection(enabled, APP_POLL_MS);
  const signedIn = connection.data?.state === "logged_in";
  const links = useJiraLinks(enabled && signedIn);
  const site = connection.data?.state === "logged_in" ? connection.data.site : null;
  const expectations = prefs?.jira_status_expectations ?? NO_EXPECTATIONS;
  const chipsOnPrRows = prefs?.jira_chips_on_prs ?? true;
  const branchTemplate = prefs?.jira_branch_template ?? "";
  const mergeGraceMins = prefs?.jira_merge_grace_mins ?? DEFAULT_MERGE_GRACE_MINS;
  const noTicketOn = prefs?.jira_no_ticket_marker ?? false;
  return useMemo<JiraState | null>(() => {
    if (!enabled || !signedIn || !links.data || links.data.error !== null) return null;
    return {
      projects: new Set(links.data.projects),
      site,
      links: linkMaps(links.data.links),
      expectations,
      chipsOnPrRows,
      branchTemplate,
      mergeGraceMins,
      noTicket: noTicketOn ? noTicketIndex(links.data) : null,
    };
  }, [enabled, signedIn, links.data, site, expectations, chipsOnPrRows, branchTemplate, mergeGraceMins, noTicketOn]);
}

/// The "no ticket" index for a component that renders the provider rather
/// than sitting inside it -- the shell, which filters My PRs by it (#2046).
/// The same queries as the provider's, so no extra request is made.
export function useNoTicketIndex(prefs: UiPrefs | undefined): NoTicketIndex | null {
  return useJiraValue(prefs)?.noTicket ?? null;
}
