import { describe, expect, it } from "vitest";
import type { JiraLink, KeyLookup, KeySearch } from "@/types/jira";
import { listed, searchWords, ticketWork } from "./ticketWork";
import type { Links } from "./tree";

const github = { provider: "github" as const, host: "github.com" };
const gitlab = { provider: "gitlab" as const, host: "gitlab.example.test" };

const local: Links = {
  prs: [
    {
      identity: { source: github, repo: "octocat/hello-world", number: 7 },
      title: "ABC-1: fix",
      url: "https://github.com/octocat/hello-world/pull/7",
      is_draft: true,
      branch: "abc-1-fix",
      source: "title",
      current: true,
    },
    {
      identity: { source: github, repo: "octocat/hello-world", number: 6 },
      title: "ABC-1: first go",
      url: "https://github.com/octocat/hello-world/pull/6",
      is_draft: false,
      branch: "abc-1-first",
      source: "title",
      current: false,
    },
  ],
  worktrees: [{ repo: "hello-world", path: "/w/abc-1", branch: "abc-1-fix" }],
};

const searched = (url: string, over: Partial<Extract<JiraLink["target"], { kind: "pr" }>> = {}): JiraLink => ({
  key: "ABC-1",
  found_in: "title",
  current: false,
  target: {
    kind: "pr",
    source: gitlab,
    role: "searched",
    repo: "group/app",
    number: 4,
    title: "ABC-1 tidy",
    url,
    head_ref: "tidy",
    is_draft: false,
    state: "merged",
    author: "dana",
    ...over,
  },
});

const lookup = (links: JiraLink[]): KeyLookup => ({
  key: "ABC-1",
  links,
  search: { state: "not_asked", reason: "already known from your pull requests" },
  gitlab: null,
});

describe("ticketWork", () => {
  it("lists each pull request once, with its state and branch, before worktrees and branches", () => {
    const rows = ticketWork(
      local,
      lookup([
        { key: "ABC-1", found_in: "branch", current: true, target: { kind: "branch", repo_path: "/r/hello", branch: "abc-1-spike" } },
        // Already listed from the index: not repeated.
        { ...searched("https://github.com/octocat/hello-world/pull/7"), target: { ...(searched("x").target as Extract<JiraLink["target"], { kind: "pr" }>), url: "https://github.com/octocat/hello-world/pull/7" } },
        searched("https://gitlab.example.test/group/app/-/merge_requests/4"),
        { key: "ABC-1", found_in: "branch", current: true, target: { kind: "worktree", repo: "hello-world", path: "/w/abc-1", branch: "abc-1-fix" } },
      ]),
    );
    expect(rows.map((r) => (r.kind === "pr" ? `${r.ref} ${r.state} ${r.branch}` : `${r.kind} ${r.branch}`))).toEqual([
      "octocat/hello-world#7 draft abc-1-fix",
      "octocat/hello-world#6 no longer open abc-1-first",
      "group/app!4 merged tidy",
      "worktree abc-1-fix",
      "branch abc-1-spike",
    ]);
    const mr = rows[2];
    expect(mr.kind === "pr" && mr.identity).toBeNull();
    expect(mr.kind === "pr" && mr.gitlab).toBe(true);
  });

  it("opens a listed pull request in Headstate, and a review request too", () => {
    const rows = ticketWork(undefined, lookup([searched("u", { role: "reviewing", source: github, state: null, repo: "o/r", number: 2 })]));
    expect(rows[0]).toMatchObject({ kind: "pr", ref: "o/r#2", identity: { repo: "o/r", number: 2 } });
  });
});

describe("listed", () => {
  it("names up to three, then counts", () => {
    expect(listed(["a"])).toBe("a");
    expect(listed(["a", "b"])).toBe("a and b");
    expect(listed(["a", "b", "c"])).toBe("a, b and c");
    expect(listed(["a", "b", "c", "d", "e"])).toBe("a, b, c and 2 more");
  });
});

describe("searchWords", () => {
  const time = () => "3:04 PM";
  const asked = (over: Partial<Extract<KeySearch, { state: "asked" }>> = {}): KeySearch => ({
    state: "asked",
    at: "2026-10-28T15:04:00Z",
    total: 0,
    read: 0,
    truncated: false,
    scope: ["octocat", "AcmeCorp"],
    unsearched: [],
    ...over,
  });

  it("names what was searched when nothing was found", () => {
    expect(searchWords("GitHub", asked(), "ABC-1", { shown: 0, mine: 0, time })).toBe(
      "No pull requests name ABC-1 in repositories owned by octocat and AcmeCorp on GitHub (searched 3:04 PM).",
    );
    expect(
      searchWords("GitLab", asked({ scope: ["gitlab.example.test/group/app"], unsearched: ["gitlab.example.test/group/web"] }), "ABC-1", {
        shown: 0,
        mine: 0,
        time,
      }),
    ).toBe(
      "No merge requests name ABC-1 in gitlab.example.test/group/app on GitLab (searched 3:04 PM). 1 more project of yours not searched.",
    );
  });

  it("does not claim none when the user already has some", () => {
    expect(searchWords("GitHub", asked(), "ABC-1", { shown: 0, mine: 2, time })).toMatch(/^Nothing else names ABC-1 in/);
  });

  it("qualifies a truncated answer: counted, or only a floor", () => {
    expect(searchWords("GitHub", asked({ total: 37, read: 20, truncated: true }), "ABC-1", { shown: 3, mine: 0, time })).toBe(
      "Searched GitHub 3:04 PM in repositories owned by octocat and AcmeCorp: 37 matched and only the first 20 were read.",
    );
    expect(searchWords("GitLab", asked({ total: 20, read: 20, truncated: true, scope: ["h/g/p"] }), "ABC-1", { shown: 3, mine: 0, time })).toBe(
      "Searched GitLab 3:04 PM in h/g/p: at least 20 matched.",
    );
  });

  /// #1050: not asked is never "none found".
  it("says why it did not search, and never that nothing matched", () => {
    const words = searchWords("GitHub", { state: "not_asked", reason: "not signed in to GitHub" }, "ABC-1", { shown: 0, mine: 0, time });
    expect(words).toBe("GitHub not searched: not signed in to GitHub.");
    expect(words).not.toMatch(/No pull requests/);
    expect(searchWords("GitLab", { state: "failed", error: "GitLab request timed out" }, "ABC-1", { shown: 0, mine: 0, time })).toBe(
      "Searching GitLab failed: GitLab request timed out",
    );
  });
});
