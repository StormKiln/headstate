import { relativeTime } from "@/lib/time";
import type { JiraIssue } from "@/types/jira";

/// How old a Jira answer is, in words: "read 12 minutes ago" (#2036).
///
/// `null` when the desktop did not say when it was read, or said something
/// unreadable. Absent is not zero: an age that is not known is not shown,
/// never rendered as "just now".
export function readAge(readAt: string | null | undefined, now: Date): string | null {
  if (!readAt || Number.isNaN(Date.parse(readAt))) return null;
  return `read ${relativeTime(readAt, now)}`;
}

/// What a reader is told about an issue's freshness, for a tooltip or a
/// detail line: its age, and whether it is an old answer served because a
/// newer read failed. `failed` is the reason that read failed, when this
/// side knows it. `null` when there is nothing to say.
export function freshnessNote(issue: JiraIssue, now: Date, failed?: string | null): string | null {
  const age = readAge(issue.read_at, now);
  const stale = issue.stale === true || failed != null;
  if (!stale) return age ? `Status ${age}.` : null;
  const as = age ? `as ${age}` : "from an earlier read";
  return `Shown ${as}: the latest read failed${failed ? ` (${failed})` : ""}.`;
}
