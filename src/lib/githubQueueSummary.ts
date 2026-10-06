import type { ClientMeasurement } from "../types/measurement";
import type { SourceRefreshSnapshot } from "../api/sourceRefresh";

export interface GitHubQueueStatus { list: "authored" | "reviewing"; receipt: SourceRefreshSnapshot }
/** Accepted list evidence and its acquisition time; never query-cache activity. */
export function githubQueueSummary({ list, receipt }: GitHubQueueStatus) {
  const subject = list === "reviewing" ? "Review requests" : "Your PRs";
  const object = list === "reviewing" ? "review requests" : "your PRs";
  const rows = receipt.prs;
  const notes: string[] = [`Covers ${object} across all repositories`];
  const membership = rows?.filter(row => row.observation?.state === "retained").length ?? 0;
  const readiness = rows?.filter(row => !row.observation || row.observation.retained_fields.length || row.observation.unknown_fields.length).length ?? 0;
  const dates = rows?.filter(row => row.observation?.ready_at_state === "retained").length ?? 0;
  if (membership) notes.push(`${membership} rows have last-known list membership`);
  if (readiness) notes.push(`${readiness} rows have unconfirmed or last-known readiness`);
  if (dates) notes.push(`${dates} ready-since dates are last known`);
  const saved = receipt.fetchedAt !== undefined;
  if (saved) notes.push("Showing a saved inventory");
  let text: string;
  let measurement: Extract<ClientMeasurement, { kind: "mounted_review" }>["footer"];
  let warning = true;
  if (receipt.error || receipt.phase === "failed" || receipt.phase === "not_asked") {
    measurement = rows === undefined ? "load_failed" : "refresh_failed";
    text = `Could not ${rows === undefined ? "load" : "refresh"} ${object}`;
    if (receipt.error) notes.unshift(receipt.error);
  } else if (receipt.phase === "retrying") { text = `Retrying ${object}…`; measurement = "retrying"; }
  else if (receipt.phase === "fetching") { text = `Checking ${object}…`; measurement = "checking"; }
  else if (rows === undefined) { text = `${subject} not checked`; measurement = "not_checked"; }
  else if (typeof receipt.coverage === "object" && receipt.coverage !== null) { text = `${subject} partly checked`; measurement = "partly_checked"; }
  else if (receipt.coverage !== "complete") { text = `${subject} coverage unknown`; measurement = "coverage_unknown"; }
  else if (saved || receipt.staleSecs || membership || readiness) { text = `${subject} need checking`; measurement = "needs_checking"; }
  else { text = `${subject} checked`; warning = false; measurement = "checked"; }
  const timestamp = receipt.lastReceivedAt;
  const updatedAt = timestamp && Number.isFinite(Date.parse(timestamp)) ? Date.parse(timestamp) : undefined;
  return { measurement, text, warning, explanation: [text, ...notes].join(". "), updatedAt };
}
