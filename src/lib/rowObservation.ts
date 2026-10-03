import type { RowObservation } from "@/types/pr";

export interface ObservationStatus {
  label: "Confirmed" | "Last known" | "Pending" | "Unconfirmed";
  explanation: string;
}

export function observationStatus(row: { observation?: RowObservation | null }): ObservationStatus | null {
  const observation = row.observation;
  if (!observation) return null;
  if (observation.confirmed_review?.confirmed_by_read) return observation.state === "retained"
    ? { label: "Last known", explanation: "Your review was confirmed — last known list membership" }
    : { label: "Confirmed", explanation: "Your review is confirmed" };
  if (observation.confirmed_review?.unresolved) return { label: "Unconfirmed", explanation: "Review confirmation is unresolved — check GitHub before reviewing again" };
  if (observation.confirmed_review) return { label: "Pending", explanation: "Your submitted review is awaiting confirmation in the list" };
  if (observation.state === "retained" || observation.retained_fields.length > 0) {
    return { label: "Last known", explanation: "Last known — not confirmed by latest refresh" };
  }
  return observation.unknown_fields.length > 0
    ? { label: "Unconfirmed", explanation: "Readiness could not be confirmed" }
    : null;
}

export function observationLabel(row: { observation?: RowObservation | null }): string | null {
  return observationStatus(row)?.explanation ?? null;
}
