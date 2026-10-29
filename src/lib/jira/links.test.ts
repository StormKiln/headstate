import { describe, expect, it } from "vitest";
import type { JiraLink } from "@/types/jira";
import { linkMaps, rowId } from "./links";

const source = { provider: "github" as const, host: "github.com" };
const pr = (key: string, found_in: "title" | "branch", over: Partial<Extract<JiraLink["target"], { kind: "pr" }>> = {}, current = true): JiraLink => ({
  key,
  found_in,
  current,
  target: {
    kind: "pr",
    source,
    role: "authored",
    repo: "octocat/hello-world",
    number: 7,
    title: "ABC-1: fix",
    url: "https://github.com/octocat/hello-world/pull/7",
    head_ref: "xy-2-follow-up",
    is_draft: false,
    state: null,
    author: null,
    ...over,
  },
});

describe("linkMaps", () => {
  it("files links by row, title keys first, and by key", () => {
    const wt: JiraLink = { key: "ABC-1", found_in: "branch", current: true, target: { kind: "worktree", repo: "hello-world", path: "/w/abc-1", branch: "abc-1-fix" } };
    const { byRow, byKey } = linkMaps([pr("XY-2", "branch"), pr("ABC-1", "title"), wt]);
    expect(byRow.get(rowId({ prUrl: "https://github.com/octocat/hello-world/pull/7" }))).toEqual([
      { key: "ABC-1", source: "title" },
      { key: "XY-2", source: "branch" },
    ]);
    expect(byRow.get(rowId({ worktreePath: "/w/abc-1" }))).toEqual([{ key: "ABC-1", source: "branch" }]);
    expect([...byKey.keys()].sort()).toEqual(["ABC-1", "XY-2"]);
    expect(byKey.get("ABC-1")?.worktrees).toEqual([{ repo: "hello-world", path: "/w/abc-1", branch: "abc-1-fix" }]);
    expect(byKey.get("ABC-1")?.prs[0]).toMatchObject({ identity: { source, repo: "octocat/hello-world", number: 7 }, current: true });
  });

  /// Review requests get chips, but My Jira issues is the user's own work.
  it("lists only authored pull requests by key", () => {
    const { byRow, byKey } = linkMaps([pr("ABC-1", "title", { role: "reviewing" })]);
    expect(byRow.size).toBe(1);
    expect(byKey.size).toBe(0);
  });

  it("keeps a pull request that left the open list, marked as such", () => {
    const { byKey } = linkMaps([pr("ABC-1", "title", {}, false)]);
    expect(byKey.get("ABC-1")?.prs[0].current).toBe(false);
  });

  it("files a local branch by repository and name", () => {
    const branch: JiraLink = { key: "ABC-3", found_in: "branch", current: true, target: { kind: "branch", repo_path: "/r", branch: "abc-3" } };
    expect(linkMaps([branch]).byRow.get(rowId({ repoPath: "/r", branch: "abc-3" }))).toEqual([{ key: "ABC-3", source: "branch" }]);
  });
});
