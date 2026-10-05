import { describe, expect, it } from "vitest";
import { observationStatus } from "./rowObservation";
import type { RowObservation } from "@/types/pr";
const observed: RowObservation = { state: "observed", last_observed_at: null, unknown_fields: [], retained_fields: [] };
describe("Ready evidence dimensions", () => {
  it("qualifies only the historical date when present readiness was observed", () => {
    expect(observationStatus({ observation: { ...observed, ready_at_state: "retained" } })).toEqual({ label: "Ready date last known", explanation: "Ready-since date is last known; current readiness was observed" });
  });
  it("names retained readiness and separately unknown fields", () => {
    const value = observationStatus({ observation: { ...observed, retained_fields: ["ci", "queue"], unknown_fields: ["review"] } });
    expect(value?.label).toBe("Readiness last known");
    expect(value?.explanation).toContain("Last known: checks, merge queue");
    expect(value?.explanation).toContain("Not confirmed: review decision");
  });
  it("identifies membership without inventing field measurement ages", () => {
    const value = observationStatus({ observation: { ...observed, state: "retained", last_observed_at: "2026-10-01T12:00:00Z" } });
    expect(value?.label).toBe("Membership unconfirmed");
    expect(value?.explanation).toContain("List membership is last known");
    expect(value?.explanation).toContain("List last observed");
  });
  it("does not hide unknown readiness behind a retained date", () => {
    const value = observationStatus({ observation: { ...observed, unknown_fields: ["ci"], ready_at_state: "retained" } });
    expect(value?.label).toBe("Unconfirmed");
    expect(value?.explanation).toContain("Not confirmed: checks");
    expect(value?.explanation).toContain("Ready-since date is last known");
  });
});
