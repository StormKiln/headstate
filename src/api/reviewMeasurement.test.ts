import { describe, expect, it, vi } from "vitest";
import { readyMeasurementCounts } from "./reviewMeasurement";
import { PR_FIXTURES } from "../fixtures/prs";

describe("bounded Ready measurement traversal", () => {
  it("never calls the presentation reader when unavailable or above the cap", () => {
    const reader = vi.fn(() => true);
    const rows = Array.from({ length: 4097 }, () => PR_FIXTURES[0]);
    expect(readyMeasurementCounts(rows, rows, rows, false, reader)).toEqual({});
    expect(readyMeasurementCounts(rows, rows, rows, true, reader)).toEqual({ inventory_count: 4097, eligible_count: 4097, visible_count: 4097 });
    expect(reader).not.toHaveBeenCalled();
  });
  it("measures overlapping qualifiers without converting absent legacy evidence into fresh zero", () => {
    const legacy = { ...PR_FIXTURES[0], observation: undefined };
    const retained = { ...legacy, observation: { state: "retained" as const, last_observed_at: null,
      retained_fields: ["queue" as const], unknown_fields: ["ci" as const] } };
    expect(readyMeasurementCounts([legacy, retained], [legacy, retained], [legacy, retained], true, () => true)).toMatchObject({
      visible_retained_count: 1, visible_retained_readiness_count: 1, visible_readiness_unknown_count: 2, visible_last_known_count: 2,
    });
  });
  it("records a synthetic 4096-row count-pass cost without asserting device performance", () => {
    const rows = Array.from({ length: 4096 }, () => PR_FIXTURES[0]);
    const start = performance.now();
    for (let n = 0; n < 1000; n++) readyMeasurementCounts(rows, rows, rows, true, () => false);
    console.info(JSON.stringify({ benchmark: "synthetic-ready-count-pass", rows: 4096, iterations: 1000, elapsed_ms: performance.now() - start }));
  });
});
