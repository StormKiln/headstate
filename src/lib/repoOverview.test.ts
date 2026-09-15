import { describe, expect, it } from "vitest";
import { currencyRank, repoCurrency, repoOverviewRows, sortByCurrency } from "./repoOverview";
import type { RepoOverviewRow } from "./repoOverview";
import type { Upstream, Worktree, WorktreeRepo } from "@/types/pr";

const wt = (over: Partial<Worktree> = {}): Worktree =>
  ({
    path: "/code/a",
    branch: "main",
    head: "abc",
    size_bytes: null,
    safety: { kind: "main_checkout" },
    is_main: true,
    merged_at: null,
    upstream: { kind: "current" },
    last_commit: null,
    ...over,
  }) as Worktree;

const repo = (over: Partial<WorktreeRepo> = {}): WorktreeRepo => ({
  identity: null,
  name: "a",
  path: "/code/a",
  worktrees: [wt()],
  fetched_at: null,
  ...over,
});

const row = (upstream: Upstream | null): RepoOverviewRow => ({
  name: "a",
  path: "/code/a",
  branch: "main",
  defaultRef: "origin/main",
  upstream,
  fetchedAt: null,
});

describe("repoOverviewRows", () => {
  /// #1015: the load-bearing field is `upstream`, not `safety`.
  it("reads the upstream verdict from the main checkout", () => {
    const rows = repoOverviewRows([
      repo({ worktrees: [wt({ upstream: { kind: "behind", n: 3 } })] }),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].upstream).toEqual({ kind: "behind", n: 3 });
  });

  /// `is_main`, not "the first worktree". An orphaned gitdir's synthetic
  /// entry is deliberately `is_main: false`, and picking by position
  /// would make it the repository's own checkout.
  it("picks the main checkout rather than the first worktree", () => {
    const rows = repoOverviewRows([
      repo({
        worktrees: [
          wt({ path: "/code/a/feature", is_main: false, upstream: { kind: "ahead", n: 9 } }),
          wt({ path: "/code/a", is_main: true, upstream: { kind: "current" } }),
        ],
      }),
    ]);
    expect(rows[0].upstream).toEqual({ kind: "current" });
  });

  /// A repository with no main checkout has nothing to say about
  /// currency -- every cell of its row would be empty.
  it("drops a repository with no main checkout", () => {
    const rows = repoOverviewRows([repo({ worktrees: [wt({ is_main: false })] })]);
    expect(rows).toEqual([]);
  });

  /// #1027: `upstream: null` is "not computed", and must survive as
  /// null so the cell can render a pending skeleton rather than a
  /// verdict.
  it("preserves a null upstream rather than defaulting it", () => {
    const rows = repoOverviewRows([repo({ worktrees: [wt({ upstream: null })] })]);
    expect(rows[0].upstream).toBeNull();
  });

  /// #1026: the ref is carried, never assumed.
  it("carries the repository's own default ref", () => {
    const rows = repoOverviewRows([repo({ default_ref: "origin/master" })]);
    expect(rows[0].defaultRef).toBe("origin/master");
  });

  /// The hardcode this issue exists to prevent. An absent ref stays
  /// absent -- it does not become `origin/main`.
  it("reports an unresolved default ref as null, never origin/main", () => {
    const rows = repoOverviewRows([repo({ default_ref: null })]);
    expect(rows[0].defaultRef).toBeNull();
  });

  it("normalises a detached checkout's empty branch to null", () => {
    const rows = repoOverviewRows([repo({ worktrees: [wt({ branch: "" })] })]);
    expect(rows[0].branch).toBeNull();
  });

  it("keeps the scan's order", () => {
    const rows = repoOverviewRows([
      repo({ name: "z", path: "/code/z" }),
      repo({ name: "a", path: "/code/a" }),
    ]);
    expect(rows.map((r) => r.name)).toEqual(["z", "a"]);
  });
});

describe("repoCurrency", () => {
  /// #1027's core claim: `Untracked` is not "up to date", and it is not
  /// in the denominator either. A ratio over a set including repositories
  /// that were never compared is a number that cannot be true.
  it("excludes untracked repositories from both halves of the ratio", () => {
    const c = repoCurrency([
      row({ kind: "current" }),
      row({ kind: "untracked" }),
      row({ kind: "untracked" }),
    ]);
    expect(c).toEqual({ current: 1, compared: 1, total: 3 });
  });

  /// The states where a remote ref WAS read are all in the denominator,
  /// whatever the answer was.
  it("counts every compared state in the denominator", () => {
    const c = repoCurrency([
      row({ kind: "current" }),
      row({ kind: "ahead", n: 2 }),
      row({ kind: "behind", n: 1 }),
      row({ kind: "diverged", n: [1, 1] }),
    ]);
    expect(c).toEqual({ current: 1, compared: 4, total: 4 });
  });

  /// `null` is pending, not current. A table that counted it would
  /// claim every repository was up to date before anything was measured.
  it("counts a pending row as neither current nor compared", () => {
    const c = repoCurrency([row(null), row(null)]);
    expect(c).toEqual({ current: 0, compared: 0, total: 2 });
  });

  /// #967's shape: git was asked and could not answer. That is not
  /// evidence of currency in either direction.
  it("counts an unknown verdict as neither current nor compared", () => {
    const c = repoCurrency([row({ kind: "unknown", n: "git failed" }), row({ kind: "detached" })]);
    expect(c).toEqual({ current: 0, compared: 0, total: 2 });
  });

  /// Paired with the shortfall tests above: when every repository IS
  /// compared and current, the ratio says so plainly and invents no
  /// caveat.
  it("reports a clean set with no shortfall", () => {
    const c = repoCurrency([row({ kind: "current" }), row({ kind: "current" })]);
    expect(c).toEqual({ current: 2, compared: 2, total: 2 });
    expect(c.total - c.compared).toBe(0);
  });

  it("reports zeroes for an empty set rather than throwing", () => {
    expect(repoCurrency([])).toEqual({ current: 0, compared: 0, total: 0 });
  });
});

describe("currencyRank", () => {
  /// #1027: "the states with no comparison have no position on that
  /// axis; they go in their own group rather than being given an implied
  /// zero." An implied zero would sort the local-only repositories in
  /// among the up-to-date ones -- the position that reads as "nothing to
  /// do here".
  it("groups every uncompared state together, after the compared ones", () => {
    const uncompared: (Upstream | null)[] = [
      { kind: "untracked" },
      { kind: "detached" },
      { kind: "unknown", n: "boom" },
      null,
    ];
    for (const u of uncompared) {
      expect(currencyRank(u)).toBe(2);
      expect(currencyRank(u)).toBeGreaterThan(currencyRank({ kind: "current" }));
    }
  });

  it("ranks the states needing attention ahead of the settled ones", () => {
    expect(currencyRank({ kind: "behind", n: 1 })).toBeLessThan(currencyRank({ kind: "current" }));
    expect(currencyRank({ kind: "diverged", n: [1, 1] })).toBeLessThan(
      currencyRank({ kind: "ahead", n: 1 }),
    );
  });

  /// An untracked repository must never sort as though it were current.
  it("does not rank untracked alongside current", () => {
    expect(currencyRank({ kind: "untracked" })).not.toBe(currencyRank({ kind: "current" }));
  });
});

describe("sortByCurrency", () => {
  /// The band order, end to end: needs attention, then settled, then the
  /// rows where no comparison happened at all.
  it("puts the uncompared rows last rather than among the current ones", () => {
    const rows = [
      { ...row({ kind: "untracked" }), name: "local" },
      { ...row({ kind: "current" }), name: "fine" },
      { ...row({ kind: "behind", n: 2 }), name: "behind" },
    ];
    expect(sortByCurrency(rows).map((r) => r.name)).toEqual(["behind", "fine", "local"]);
  });

  /// `useMemo` hands back the array React has cached, so sorting in place
  /// would mutate what every other reader of the query sees.
  it("does not mutate the array it was given", () => {
    const rows = [row({ kind: "untracked" }), row({ kind: "behind", n: 1 })];
    const before = [...rows];
    sortByCurrency(rows);
    expect(rows).toEqual(before);
  });
});
