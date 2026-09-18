import { describe, expect, it } from "vitest";
import type { Branch } from "@/types/pr";
import { scopeEffect, scopeLabel, scopesFor, targetsFor } from "./branchDelete";

const b = (over: Partial<Branch> = {}): Branch => ({
  name: "feature",
  location: "local",
  upstream: null,
  ahead: 0,
  behind: 0,
  committed: "2026-09-01T00:00:00Z",
  author: "octocat",
  tip: "abc1234",
  deletable: { kind: "merged", how: "squash" },
  ...over,
});

const tracked = (name: string) =>
  b({ name, location: "tracked", upstream: `origin/${name}` });
const remoteOnly = (name: string) => b({ name: `origin/${name}`, location: "remote" });
/// A branch whose upstream is configured but no longer on the remote
/// (#1139) -- the ordinary state after a PR merges. It keeps its
/// `upstream` string, because that is the name of the ref that is gone.
const gone = (name: string) => b({ name, location: "gone", upstream: `origin/${name}` });

describe("scopesFor", () => {
  it("offers only local when nothing selected has a remote side", () => {
    expect(scopesFor([b({ name: "x" })])).toEqual(["local"]);
  });

  it("offers only remote for a remote-only selection", () => {
    expect(scopesFor([remoteOnly("x")])).toEqual(["remote"]);
  });

  /// The #473 case: a tracked branch exists in both places, so all
  /// three scopes are real choices.
  it("offers both for a tracked branch", () => {
    expect(scopesFor([tracked("x")])).toEqual(["local", "remote", "both"]);
  });

  /// A tracked branch with no upstream has no remote side to delete,
  /// whatever its location says.
  it("does not offer a remote scope for a tracked branch with no upstream", () => {
    const orphan = b({ name: "x", location: "tracked", upstream: null });
    expect(scopesFor([orphan])).toEqual(["local"]);
  });

  it("offers both scopes for a mixed selection, without offering both-at-once", () => {
    expect(scopesFor([b({ name: "x" }), remoteOnly("y")])).toEqual(["local", "remote"]);
  });
});

describe("targetsFor", () => {
  /// THE bug. Deleting a tracked branch locally must not silently
  /// leave the remote branch alive.
  it("sends a tracked branch to both calls when the scope is both", () => {
    expect(targetsFor([tracked("shipped")], "both")).toEqual({
      local: ["shipped"],
      remote: ["origin/shipped"],
    });
  });

  /// The remote call needs the UPSTREAM name, not the local one.
  /// Passing "shipped" where "origin/shipped" is expected deletes the
  /// wrong thing or nothing at all.
  it("uses the upstream name for the remote half of a tracked branch", () => {
    expect(targetsFor([tracked("shipped")], "remote")).toEqual({
      local: [],
      remote: ["origin/shipped"],
    });
  });

  it("passes a remote-only branch through whole", () => {
    expect(targetsFor([remoteOnly("gone")], "remote")).toEqual({
      local: [],
      remote: ["origin/gone"],
    });
  });

  it("never sends a remote-only branch to the local call", () => {
    expect(targetsFor([remoteOnly("gone")], "both").local).toEqual([]);
  });

  it("never sends a local-only branch to the remote call", () => {
    expect(targetsFor([b({ name: "x" })], "both").remote).toEqual([]);
  });

  it("splits a mixed selection correctly", () => {
    const sel = [b({ name: "loc" }), tracked("tr"), remoteOnly("rem")];
    expect(targetsFor(sel, "both")).toEqual({
      local: ["loc", "tr"],
      remote: ["origin/tr", "origin/rem"],
    });
  });
});

describe("scopeLabel", () => {
  /// Remote deletion pushes to shared state that no reflog can undo.
  /// With one button instead of two, the words carry that warning.
  it("says remote deletion is on the remote", () => {
    const t = { local: [], remote: ["origin/a"] };
    expect(scopeLabel("remote", t)).toMatch(/on the remote/);
  });

  it("counts what will actually be deleted", () => {
    expect(scopeLabel("local", { local: ["a", "b"], remote: [] })).toMatch(/2 branches/);
    expect(scopeLabel("local", { local: ["a"], remote: [] })).toMatch(/1 branch\b/);
  });
});

describe("scopeEffect", () => {
  /// The sentence two surfaces now share (#845).
  ///
  /// `BranchesPage`'s scope questionnaire and the PR detail view's
  /// single-branch confirmation both render it. They used to be one local
  /// closure and one missing dialog; a copy in each would let them
  /// disagree about the one operation a reflog cannot undo, which is the
  /// same argument `scopeLabel` above exists for.
  it("says a remote deletion cannot be undone by a reflog", () => {
    expect(scopeEffect("remote")).toMatch(/no local reflog can undo/);
  });

  /// The local sentence is the CONTRAST that makes the remote one mean
  /// something. Asserted here so a future edit cannot quietly make both
  /// scopes sound equally final, which would flatten the distinction
  /// `BranchesPage` defaults its scope choice on ("Local deletion is
  /// recoverable from the reflog; a remote deletion is not").
  it("says a local deletion is recoverable, unlike the remote one", () => {
    expect(scopeEffect("local")).toMatch(/recoverable from the reflog/i);
    expect(scopeEffect("local")).not.toMatch(/cannot be undone/i);
  });

  it("says the remote half of a both-scope deletion is the final one", () => {
    expect(scopeEffect("both")).toMatch(/remote half cannot be undone/);
  });

  /// Every scope answers, because the PR detail view reads one of them
  /// straight into a dialog -- an undefined there would render an empty
  /// warning under a red title, which is worse than no dialog.
  it("answers for every scope", () => {
    for (const s of ["local", "remote", "both"] as const) {
      expect(scopeEffect(s).length).toBeGreaterThan(0);
    }
  });
});


/// #1139: a gone branch has a local side and NO remote side.
///
/// The dangerous version of that change types it `tracked`, which would
/// offer a push against a ref that is not there -- and `targetsFor`
/// would hand `delete_remote` an `origin/foo` that the remote deleted
/// weeks ago. These pin the safe shape.
describe("a branch whose upstream is gone", () => {
  it("is offered for local deletion", () => {
    expect(scopesFor([gone("feature")])).toEqual(["local"]);
  });

  it("is never offered a remote or both scope", () => {
    const scopes = scopesFor([gone("feature")]);
    expect(scopes).not.toContain("remote");
    expect(scopes).not.toContain("both");
  });

  it("is a local target, by its local name", () => {
    expect(targetsFor([gone("feature")], "local")).toEqual({
      local: ["feature"],
      remote: [],
    });
  });

  /// The load-bearing one. Even when a selection mixes a gone branch
  /// with a tracked one -- so the remote scope IS on offer -- the gone
  /// branch must not be pushed against.
  it("is excluded from a remote scope it did not ask for", () => {
    const mixed = [gone("old"), tracked("live")];
    expect(scopesFor(mixed)).toContain("remote");
    expect(targetsFor(mixed, "remote")).toEqual({
      local: [],
      remote: ["origin/live"],
    });
  });

  it("is still deleted locally under a both scope", () => {
    const mixed = [gone("old"), tracked("live")];
    expect(targetsFor(mixed, "both")).toEqual({
      local: ["old", "live"],
      remote: ["origin/live"],
    });
  });
});
