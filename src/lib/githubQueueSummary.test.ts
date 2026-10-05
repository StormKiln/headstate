import { describe, expect, it } from "vitest";
import { githubQueueSummary } from "./githubQueueSummary";
import type { SourceRefreshSnapshot } from "../api/sourceRefresh";
import { PR_FIXTURES } from "../fixtures/prs";
const receipt: SourceRefreshSnapshot = { modern: true, prs: [], error: null, coverage: "complete" };
const summary = (patch: Partial<SourceRefreshSnapshot>) => githubQueueSummary({ list: "reviewing", receipt: { ...receipt, ...patch } });
const row = { ...PR_FIXTURES[0], observation: { state: "observed" as const, last_observed_at: null, unknown_fields: [], retained_fields: [] } };
describe("GitHub inventory summary", () => {
  it.each([
    [{ phase: "retrying" }, "Retrying review requests…"],
    [{ phase: "fetching" }, "Checking review requests…"],
    [{ prs: undefined }, "Review requests not checked"],
    [{ coverage: "unknown" }, "Review requests coverage unknown"],
    [{ coverage: { partial: { total: null } } }, "Review requests partly checked"],
    [{ error: "offline" }, "Could not refresh review requests"],
    [{ error: "offline", prs: undefined }, "Could not load review requests"],
    [{ fetchedAt: "2026-10-01T12:00:00Z" }, "Review requests need checking"],
  ] as [Partial<SourceRefreshSnapshot>, string][])("qualifies %j", (patch, text) => {
    expect(summary(patch)).toMatchObject({ text, warning: true });
  });
  it("separates ready date history from current readiness", () => {
    const result = summary({ prs: [{ ...row, observation: { ...row.observation, ready_at_state: "retained" } }] });
    expect(result).toMatchObject({ text: "Review requests checked", warning: false });
    expect(result.explanation).toContain("1 ready-since dates are last known");
  });
  it.each(["retained", "unknown"] as const)("qualifies %s readiness even with complete coverage", kind => {
    expect(summary({ prs: [{ ...row, observation: { ...row.observation, [`${kind}_fields`]: ["ci"] } }] }).warning).toBe(true);
  });
  it("does not assume legacy rows have field-level evidence", () => {
    expect(summary({ prs: [{ ...row, observation: undefined }] }).text).toBe("Review requests need checking");
  });
  it("ignores optional advisory lifetime and rejects invalid timestamps", () => {
    expect(summary({ prs: [row], lastReceivedAt: "invalid" })).toMatchObject({ text: "Review requests checked", updatedAt: undefined });
  });
});
