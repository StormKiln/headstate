import { evidenceEnabled, preserveMeasurement, nodeVersion } from "../../scripts/measurement-evidence.mjs";
import { describe, expect, it, vi } from "vitest";
import { readyMeasurementCounts, readyMeasurementLastKnown } from "./reviewMeasurement";
import { display, type Evidence } from "./advisoryEvidence";
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
  it("uses the real presentation predicate for retained readiness, date, stack and pusher evidence", () => {
    const row = { ...PR_FIXTURES[0], ready_at: "2026-10-01T00:00:00Z", observation: { state: "observed" as const, last_observed_at: null, retained_fields: [], unknown_fields: [] } };
    const now = new Date("2026-10-06T00:00:00Z");
    expect(readyMeasurementLastKnown(row, undefined, undefined, now)).toBe(false);
    expect(readyMeasurementLastKnown({ ...row, observation: { ...row.observation, retained_fields: ["queue"] } }, undefined, undefined, now)).toBe(true);
    expect(readyMeasurementLastKnown({ ...row, observation: { ...row.observation, ready_at_state: "retained" } }, undefined, undefined, now)).toBe(true);
    expect(readyMeasurementLastKnown(row, { freshness: "retained", value: { kind: "stacked" } }, undefined, now)).toBe(true);
    expect(readyMeasurementLastKnown(row, undefined, { freshness: "retained", value: { pusher: { state: "viewer" } } }, now)).toBe(true);
    expect(readyMeasurementLastKnown(row, { freshness: "fresh", value: { kind: "stacked" } }, { freshness: "fresh", value: { pusher: { state: "viewer" } } }, now)).toBe(false);
  });
  it.skipIf(!evidenceEnabled)("preserves actual qualifier cost samples for synthetic Ready populations", async () => {
    const results = [];
    const now = new Date("2026-10-06T00:00:00Z");
    for (const size of [130, 236, 4096, 4097]) {
      const rows = Array.from({ length: size }, (_, i) => ({ ...PR_FIXTURES[0], number: i,
        ready_at: "2026-10-01T00:00:00Z", observation: { state: "observed" as const, last_observed_at: null,
          retained_fields: i % 5 === 0 ? ["queue" as const] : [], unknown_fields: [], ready_at_state: i % 5 === 1 ? "retained" as const : "observed" as const } }));
      const stack = new Map<number, Evidence<{kind:string}>>();
      const pusher = new Map<number, Evidence<{pusher:{state:string}}>>();
      for (const row of rows) {
        stack.set(row.number, { value: { kind: row.number % 5 === 2 ? "stacked" : "single" }, expiresAt: 0, observedAt: 0 });
        pusher.set(row.number, { value: { pusher: { state: row.number % 5 === 3 ? "viewer" : "other" } }, expiresAt: 0, observedAt: 0 });
      }
      let calls = 0;
      const reader = (row: (typeof PR_FIXTURES)[number]) => { calls++; return readyMeasurementLastKnown(row, display(stack.get(row.number)), display(pusher.get(row.number)), now); };
      for (const enabled of [false, true]) {
        const samples = [];
        const before = calls;
        for (let sample = 0; sample < 20; sample++) {
          const start = performance.now();
          for (let i = 0; i < 20; i++) readyMeasurementCounts(rows, rows, rows, enabled, reader);
          samples.push((performance.now() - start) / 20);
        }
        results.push({ size, enabled, iterations: 400, milliseconds_per_pass: samples, predicate_calls: calls - before });
      }
    }
    preserveMeasurement("ready-cost", {
      method: "Actual mounted qualifier and production display expiry; synthetic in-memory advisory maps. Excludes full advisory-hook assembly, React rendering and provider work. Samples are 20-pass batch means, not individual latency percentiles.", node: nodeVersion, results,
    });
    for (const r of results) expect(r.predicate_calls).toBe(r.enabled && r.size <= 4096 ? r.size * 400 : 0);
  });
});
