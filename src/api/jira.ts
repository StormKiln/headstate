import { useQuery } from "@tanstack/react-query";
import { jiraConnection, jiraProjects } from "./tauri";

/// --- Jira (epic #2031) --------------------------------------------
///
/// Kept out of `hooks.ts`: every Jira read is gated on the capability
/// switch, and a disabled query here must not issue anything. `enabled`
/// is the switch itself, so turning Jira off stops the calls rather than
/// asking the desktop to refuse them.

/// Whether `acli` is installed and signed in. Slow-polled: signing in
/// happens in a terminal, and the panel should notice without a reload.
export function useJiraConnection(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-connection"],
    queryFn: () => jiraConnection(),
    enabled,
    refetchInterval: enabled ? 30_000 : false,
    staleTime: 10_000,
  });
}

/// The projects the signed-in user can see. Changes on human time.
export function useJiraProjects(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-projects"],
    queryFn: () => jiraProjects(),
    enabled,
    staleTime: 10 * 60_000,
  });
}
