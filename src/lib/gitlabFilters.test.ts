import { prKey } from "./prIdentity";
import { describe, expect, it } from "vitest";
import { filterGitLab, gitlabParents } from "./gitlabFilters";
import type { MergeRequest } from "../types/gitlab";

const row = { title: "Fix a bug", repo: "team/app", number: 7, is_draft: false, ci: null, review: null,
  unresolved_threads: null, detailed_merge_status: null, labels: [{ name: "bug", color: "" }], updated_at: "2026-01-01T00:00:00Z" } as MergeRequest;
describe("GitLab queue filters", () => {
  it("does not turn unmeasured evidence into matching success, approval or unresolved threads", () => {
    for (const filter of [{ ci: "success" as const }, { review: "approved" as const }, { unresolvedOnly: true }]) {
      expect(filterGitLab([row], filter)).toEqual([]);
    }
    expect(filterGitLab([row], {})).toEqual([row]);
  });
  it("supports search, labels, drafts, age and measured attention", () => {
    expect(filterGitLab([row], { query: "!7", includeLabels: ["bug"], readyOnly: true, staleOnly: true }, Date.parse("2026-02-01"))).toEqual([row]);
    expect(filterGitLab([row], { excludeLabels: ["bug"] })).toEqual([]);
    expect(filterGitLab([row], { draftsOnly: true })).toEqual([]);
    expect(filterGitLab([{ ...row, ci: "failure" }], { needsAttentionOnly: true })).toHaveLength(1);
    expect(filterGitLab([{ ...row, detailed_merge_status: "conflict" }], { needsAttentionOnly: true })).toHaveLength(1);
  });
});

it("matches GitLab parent branches only within the same host and full project path", () => {
  const parent = { ...row, source: { provider: "gitlab" as const, host: "gitlab.com" }, head_ref: "topic", base_ref: "main" };
  const child = { ...parent, number: 8, head_ref: "next", base_ref: "topic" };
  expect(gitlabParents([parent, child]).get(prKey(child))).toBe(7);
  expect(gitlabParents([parent, { ...child, source: { ...child.source, host: "elsewhere.example" } }]).size).toBe(0);
});
