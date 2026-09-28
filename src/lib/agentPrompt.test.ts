import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import {
  type AgentContext,
  agentPrompt,
  shellWord,
  toAgentContext,
  worktreeName,
} from "@/lib/agentPrompt";

const ctx = (over: Partial<AgentContext> = {}): AgentContext => ({
  repo: "octocat/hello-world",
  number: 42,
  title: "Add retry to the client",
  url: "https://github.com/octocat/hello-world/pull/42",
  head_ref: "feature/retry-client",
  base_ref: "main",
  merge_status: "clean",
  unresolved_threads: 0,
  checks: [],
  ...over,
});

describe("agentPrompt", () => {
  // The lead line is what an agent acts on, so it must name the task
  // rather than restate the state.
  it("leads with conflicts, which block everything else", () => {
    const out = agentPrompt(
      ctx({
        merge_status: "dirty",
        checks: [{ name: "build", state: "failure", url: "u" }],
        unresolved_threads: 3,
      }),
    );
    expect(out.split("\n")[0]).toMatch(/^Resolve the merge conflicts on octocat\/hello-world#42/);
  });

  it("leads with CI when there are no conflicts", () => {
    const out = agentPrompt(
      ctx({ checks: [{ name: "build", state: "failure", url: "u" }], unresolved_threads: 3 }),
    );
    expect(out.split("\n")[0]).toMatch(/^Fix the failing CI/);
  });

  it("leads with review feedback when CI is green", () => {
    expect(agentPrompt(ctx({ unresolved_threads: 2 })).split("\n")[0]).toMatch(
      /^Address the review feedback/,
    );
  });

  it("always names the branch pair, so the agent knows what to check out", () => {
    expect(agentPrompt(ctx())).toContain("(feature/retry-client → main)");
  });

  // skipped and pending are not failures. Treating anything non-success as
  // broken would send an agent chasing checks that never ran.
  it("lists only genuinely failed checks", () => {
    const out = agentPrompt(
      ctx({
        checks: [
          { name: "build", state: "failure", url: "https://ci/build" },
          { name: "lint", state: "error", url: "https://ci/lint" },
          { name: "skipped-job", state: "skipped", url: "https://ci/skip" },
          { name: "running", state: "pending", url: "https://ci/run" },
          { name: "tests", state: "success", url: "https://ci/tests" },
        ],
      }),
    );
    expect(out).toContain("build: https://ci/build");
    expect(out).toContain("lint: https://ci/lint");
    for (const absent of ["skipped-job", "running", "tests"]) {
      expect(out).not.toContain(absent);
    }
  });

  // A list row has no per-check detail. Silence would read as "CI is
  // fine", which is a lie the agent would act on.
  it("says checks were not loaded rather than implying CI is clean", () => {
    const out = agentPrompt(ctx({ checks: undefined }));
    expect(out).toMatch(/Failing checks: not loaded/);
  });

  it("stays silent about checks when they loaded and none failed", () => {
    expect(agentPrompt(ctx({ checks: [{ name: "build", state: "success", url: "u" }] }))).not.toMatch(
      /Failing checks/,
    );
  });

  it("mentions unresolved conversations only when there are some", () => {
    expect(agentPrompt(ctx({ unresolved_threads: 3 }))).toContain(
      "Unresolved review conversations: 3",
    );
    expect(agentPrompt(ctx())).not.toContain("Unresolved review");
  });
});

/// #1455: "Review X" alone left the agent to invent a standard.
describe("review criteria", () => {
  const CRITERIA = [
    /Correctness and edge cases/,
    /Security/,
    /Performance/,
    /Tests: is the CHANGED behaviour covered/,
    /Documentation and comments/,
    /Backwards compatibility and migrations/,
    /Error handling/,
  ];

  it("proposes every criterion when the task is a review", () => {
    const out = agentPrompt(ctx());
    expect(out.split("\n")[0]).toMatch(/^Review /);
    for (const c of CRITERIA) expect(out).toMatch(c);
    // Named against the actual base branch.
    expect(out).toContain("relying on main's");
    // Before the setup, which stays last.
    expect(out.indexOf("Error handling")).toBeLessThan(out.indexOf("git worktree add"));
  });

  /// The lead lines keep their concrete job; a checklist would bury it.
  it.each([
    ["conflicts", { merge_status: "dirty" }],
    ["failing CI", { checks: [{ name: "build", state: "failure", url: "u" }] }],
    ["feedback", { unresolved_threads: 2 }],
    ["behind", { merge_status: "behind" }],
  ])("leaves the criteria out for %s", (_name, over) => {
    const out = agentPrompt(ctx(over as Partial<AgentContext>));
    expect(out).not.toMatch(/Review it against these criteria/);
  });

  /// Adapted where the data is real, and silent where it is absent: a
  /// list row has no size, so none is stated (absent is not zero).
  it("adapts to size, description and pending checks only when known", () => {
    const rich = agentPrompt(
      ctx({
        size: { additions: 120, deletions: 4, changed_files: 1 },
        has_description: false,
        checks: [
          { name: "build", state: "pending", url: "u" },
          { name: "lint", state: "success", url: "u" },
        ],
      }),
    );
    expect(rich).toContain("The change is +120/-4 across 1 file.");
    expect(rich).toContain("The PR has no description");
    expect(rich).toContain("1 check is still running");

    const bare = agentPrompt(ctx({ checks: undefined }));
    expect(bare).not.toContain("The change is");
    expect(bare).not.toContain("no description");
    expect(bare).not.toContain("still running");
  });
});

describe("toAgentContext", () => {
  // The absence of `checks` on a row is load-bearing -- it drives the
  // "not loaded" line -- so it must not be defaulted to [].
  it("preserves a list row's missing checks rather than defaulting them", () => {
    expect(toAgentContext(PR_FIXTURES[0]).checks).toBeUndefined();
    // And its missing size and body, for the same reason.
    expect(toAgentContext(PR_FIXTURES[0]).size).toBeUndefined();
    expect(toAgentContext(PR_FIXTURES[0]).has_description).toBeUndefined();
  });

  it("carries the checkout the caller resolved", () => {
    expect(toAgentContext(PR_FIXTURES[0], "/code/r").checkout).toBe("/code/r");
    expect(toAgentContext(PR_FIXTURES[0]).checkout).toBeUndefined();
  });

  it("carries a detail's checks through", () => {
    const detail = {
      ...PR_FIXTURES[0],
      checks: [{ name: "build", state: "failure", url: "u" }],
    } as unknown as Parameters<typeof toAgentContext>[0];
    expect(toAgentContext(detail).checks).toHaveLength(1);
  });
});

/// #426: the prompt named what was wrong and never said where to work.
describe("where the agent should work", () => {
  it("tells the agent to use a worktree, not the checkout itself", () => {
    const out = agentPrompt(ctx());
    expect(out).toContain("git worktree add");
    expect(out).toMatch(/not in the checkout itself/i);
  });

  /// #1455: `../pr-{n}` was relative to wherever the agent started, so
  /// from inside another worktree it landed beside THAT one. The hint
  /// now says to start from the main checkout, and names it when known.
  it("anchors the worktree on the main checkout, by path when one is known", () => {
    const known = agentPrompt(ctx({ checkout: "/code/hello-world" }));
    expect(known).toContain("created from the main checkout at /code/hello-world");
    const cdAt = known.indexOf("  cd /code/hello-world\n");
    expect(cdAt).toBeGreaterThan(-1);
    // The cd comes before the fetch, so the relative add is relative to it.
    expect(cdAt).toBeLessThan(known.indexOf("git fetch origin"));
    expect(known).toContain("git worktree add ../hello-world-pr-42 feature/retry-client");
    expect(known.trimEnd().endsWith("cd ../hello-world-pr-42")).toBe(true);

    const unknown = agentPrompt(ctx());
    expect(unknown).toMatch(/from the repository's main checkout \(not from another worktree\)/);
    expect(unknown).not.toMatch(/^ {2}cd \//m);
  });

  /// The setup lines are commands an agent runs, and a branch name is
  /// chosen by whoever pushed it. Quoted the way `shell_quote` does.
  it("shell-quotes a branch or checkout that needs it", () => {
    const out = agentPrompt(ctx({ head_ref: "x';touch pwned;'", checkout: "/my code/repo" }));
    expect(out).toContain(`git fetch origin 'x'\\'';touch pwned;'\\'''`);
    expect(out).toContain("  cd '/my code/repo'");
    expect(shellWord("feature/retry-client")).toBe("feature/retry-client");
    expect(shellWord("$(whoami)")).toBe("'$(whoami)'");
  });

  /// Only the name after the slash, and only a plain one, may prefix the
  /// directory: a repository name must not add a path segment.
  it("falls back to pr-{n} when the repository name is not a plain name", () => {
    expect(worktreeName({ repo: "octocat/hello-world", number: 7 })).toBe("hello-world-pr-7");
    expect(worktreeName({ repo: "octocat/..", number: 7 })).toBe("pr-7");
    expect(worktreeName({ repo: "", number: 7 })).toBe("pr-7");
  });

  it("fetches the PR's branch before adding the worktree", () => {
    const out = agentPrompt(ctx({ head_ref: "feature/retry-client" }));
    const fetchAt = out.indexOf("git fetch origin feature/retry-client");
    const addAt = out.indexOf("git worktree add");
    expect(fetchAt).toBeGreaterThan(-1);
    expect(addAt).toBeGreaterThan(fetchAt);
  });

  /// A branch name can contain slashes, which would nest the worktree
  /// somewhere the user did not expect. The PR number cannot.
  it("names the worktree from the number, so a slashed branch cannot nest it", () => {
    const out = agentPrompt(ctx({ number: 42, head_ref: "feature/deep/nested" }));
    expect(out).toContain("../hello-world-pr-42");
    expect(out).not.toContain("../feature/deep/nested");
  });

  /// The instruction is last: it is what the agent does FIRST, and a
  /// reader scanning back finds it at the end.
  it("puts the setup after the problem description", () => {
    const out = agentPrompt(ctx({ merge_status: "dirty" }));
    expect(out.indexOf("merge conflicts")).toBeLessThan(out.indexOf("git worktree add"));
  });
});
