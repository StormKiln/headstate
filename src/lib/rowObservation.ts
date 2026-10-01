import type { RowObservation } from "@/types/pr";

export function observationLabel(row: { observation?: RowObservation | null }): string | null {
  const observation = row.observation;
  if (!observation) return null;
  if (observation.confirmed_review) return "Your submitted review is awaiting confirmation in the list";
  if (observation.state === "retained" || observation.retained_fields.length > 0) {
    return "Last known — not confirmed by latest refresh";
  }
  return observation.unknown_fields.length > 0 ? "Readiness could not be confirmed" : null;
}
