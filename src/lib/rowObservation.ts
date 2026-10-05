import type { RowObservation } from "@/types/pr";

export interface ObservationStatus {
  label: "Confirmed" | "Membership unconfirmed" | "Readiness last known" | "Ready date last known" | "Pending" | "Unconfirmed";
  explanation: string;
}
const fieldNames = { head: "commit", draft: "draft status", ci: "checks", merge: "mergeability", review: "review decision", queue: "merge queue" };

export function observationStatus(row: { observation?: RowObservation | null }): ObservationStatus | null {
  const observation = row.observation;
  if (!observation) return null;
  const reasons: string[] = [];
  if (observation.state === "retained") reasons.push("List membership is last known; not confirmed by the latest refresh");
  if (observation.retained_fields.length) reasons.push(`Last known: ${observation.retained_fields.map(field => fieldNames[field]).join(", ")}`);
  if (observation.unknown_fields.length) reasons.push(`Not confirmed: ${observation.unknown_fields.map(field => fieldNames[field]).join(", ")}`);
  if (observation.ready_at_state === "retained") reasons.push("Ready-since date is last known");
  const effect = observation.confirmed_review;
  let label: ObservationStatus["label"];
  if (effect?.confirmed_by_read) {
    label = observation.state === "retained" ? "Membership unconfirmed" : "Confirmed";
    reasons.unshift("Your review is confirmed");
  } else if (effect?.unresolved) {
    label = "Unconfirmed";
    reasons.unshift("Review confirmation is unresolved — check GitHub before reviewing again");
  } else if (effect) {
    label = "Pending";
    reasons.unshift("Your submitted review is awaiting confirmation in the list");
  } else if (observation.state === "retained") label = "Membership unconfirmed";
  else if (observation.retained_fields.length) label = "Readiness last known";
  else if (observation.unknown_fields.length) label = "Unconfirmed";
  else if (observation.ready_at_state === "retained") {
    label = "Ready date last known";
    reasons[0] += "; current readiness was observed";
  } else return null;
  // This is a list observation time, not the acquisition time of a retained
  // individual field or the ready-since event. Never imply those were re-read.
  if (observation.last_observed_at && Number.isFinite(Date.parse(observation.last_observed_at))) {
    reasons.push(`List last observed ${new Date(observation.last_observed_at).toLocaleString()}`);
  }
  return { label, explanation: reasons.join(". ") };
}

export function observationLabel(row: { observation?: RowObservation | null }): string | null {
  return observationStatus(row)?.explanation ?? null;
}
