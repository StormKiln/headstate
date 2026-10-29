import { useFollowJiraLinks } from "@/api/jira";
import { useJira } from "./jiraState";

/// Reads the Jira link index again when a page's scan finishes: the
/// desktop indexes keys as it scans (#2035). Renders nothing, and mounts
/// its hook only while Jira is on, so a page without Jira asks nothing.
export function FollowJiraLinks({ updatedAt }: { updatedAt: number }) {
  return useJira() ? <Follow updatedAt={updatedAt} /> : null;
}

function Follow({ updatedAt }: { updatedAt: number }) {
  useFollowJiraLinks(true, updatedAt);
  return null;
}
