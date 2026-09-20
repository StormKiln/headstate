import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeDefinition, ClaudeDefinitions } from "@/api/tauri";

const state = vi.hoisted(() => ({
  data: undefined as ClaudeDefinitions | undefined,
  isError: false,
}));

vi.mock("../api/hooks", () => ({
  useClaudeDefinitions: () => ({
    data: state.data,
    isLoading: false,
    isError: state.isError,
    error: state.isError ? new Error("permission denied") : null,
    refetch: () => {},
  }),
}));

const { DefinitionsSection } = await import("./ClaudePluginsPage");

/// A definition with the boring fields filled in, so each test states
/// only the thing it is about.
function def(over: Partial<ClaudeDefinition> = {}): ClaudeDefinition {
  return {
    kind: "skill",
    name: "shipping",
    namedInFrontmatter: true,
    description: null,
    path: "/h/.claude/skills/deploy/SKILL.md",
    source: { scope: "user" },
    ...over,
  };
}

/// #1129: what the plugins table could not say.
describe("the definitions section", () => {
  it("lists a skill by name", () => {
    state.data = {
      definitions: [def({ description: "how we ship" })],
      collisions: [],
      unreadable: [],
    };
    render(<DefinitionsSection />);
    expect(screen.getByText("shipping")).toBeTruthy();
  });

  /// A name taken from the filename is real -- a skill directory IS
  /// addressed by its name -- but a reader has to be able to tell it
  /// from one the author wrote.
  it("marks a name that came from the filename", () => {
    state.data = {
      definitions: [
        def({
          kind: "agent",
          name: "reviewer",
          namedInFrontmatter: false,
          path: "/h/.claude/agents/reviewer.md",
        }),
      ],
      collisions: [],
      unreadable: [],
    };
    render(<DefinitionsSection />);
    expect(screen.getByText(/from filename/)).toBeTruthy();
  });

  /// And a name the author DID write is not marked. Without this the
  /// test above passes on a component that marks every row, which is
  /// the bug the `named_in_frontmatter` / `namedInFrontmatter` casing
  /// mismatch actually shipped.
  it("does not mark a name that came from frontmatter", () => {
    state.data = { definitions: [def()], collisions: [], unreadable: [] };
    render(<DefinitionsSection />);
    expect(screen.queryByText(/from filename/)).toBeNull();
  });

  /// The load-bearing case. "You have none" and "we could not look" are
  /// different answers, and this page's whole argument is that the
  /// second must never render as the first.
  it("reports a read failure rather than an empty list", () => {
    state.data = undefined;
    state.isError = true;
    render(<DefinitionsSection />);
    expect(screen.getByText(/could not be read/)).toBeTruthy();
    expect(screen.queryByText(/No skills, agents or commands/)).toBeNull();
    state.isError = false;
  });

  /// A measured zero is a real answer and says so plainly.
  it("says none are defined when the scan found none", () => {
    state.data = { definitions: [], collisions: [], unreadable: [] };
    render(<DefinitionsSection />);
    expect(screen.getByText(/No skills, agents or commands are defined here/)).toBeTruthy();
  });

  /// An unreadable scope hides an unknown number, so the list is
  /// qualified rather than presented as complete -- and the scope is
  /// NAMED, because a project behind a permission wall must not hide
  /// inside a successful user scan (#1215).
  it("qualifies the list when a scope could not be read, and names it", () => {
    state.data = {
      definitions: [
        def({ kind: "command", name: "sync", path: "/h/.claude/commands/git/sync.md" }),
      ],
      collisions: [],
      unreadable: [
        {
          source: { scope: "project", path: "/code/headstate" },
          detail: "/code/headstate/.claude/agents: Permission denied (os error 13)",
        },
      ],
    };
    render(<DefinitionsSection />);
    expect(screen.getByText(/may not be all of them/)).toBeTruthy();
    expect(screen.getByText(/\/code\/headstate\/\.claude\/agents/)).toBeTruthy();
  });

  // ---- #1215: scopes and collisions ---------------------------------

  /// Every row says where it came from. Across ~38 repositories a list
  /// of bare names is not something a user can act on.
  it("names the scope each definition came from", () => {
    state.data = {
      definitions: [
        def({ name: "user-one" }),
        def({
          name: "project-one",
          path: "/code/headstate/.claude/skills/x/SKILL.md",
          source: { scope: "project", path: "/code/headstate" },
        }),
        def({
          name: "plugin-one",
          path: "/h/.claude/plugins/cache/superpowers/skills/y/SKILL.md",
          source: {
            scope: "plugin",
            name: "superpowers",
            path: "/h/.claude/plugins/cache/superpowers",
          },
        }),
      ],
      collisions: [],
      unreadable: [],
    };
    render(<DefinitionsSection />);
    // By `title`, which is the badge's full path: the intro prose also
    // says `~/.claude`, and matching that would pass on a component
    // that drew no badges at all.
    const badge = (t: string) => document.querySelector(`span[title="${t}"]`)?.textContent;
    expect(badge("~/.claude")).toBe("~/.claude");
    expect(badge("/code/headstate")).toBe("headstate");
    expect(badge("/h/.claude/plugins/cache/superpowers")).toBe("superpowers");
  });

  /// THE test of #1215.
  ///
  /// Two scopes claiming one name are TWO rows, both marked, with both
  /// sources named. Deduping to one -- by any precedence -- would be a
  /// second source of truth for Claude Code's shadowing rule, and
  /// unlike a wrong merged value it would delete a definition the user
  /// can see on disk.
  it("shows both sides of a collision rather than picking a winner", () => {
    state.data = {
      definitions: [
        def({ name: "review", path: "/h/.claude/skills/review/SKILL.md" }),
        def({
          name: "review",
          path: "/code/headstate/.claude/skills/review/SKILL.md",
          source: { scope: "project", path: "/code/headstate" },
        }),
      ],
      collisions: [{ kind: "skill", name: "review", members: [0, 1] }],
      unreadable: [],
    };
    render(<DefinitionsSection />);

    // Both survive as ROWS. Counted in the list rather than by every
    // occurrence of the text, because the collision summary above names
    // it too -- and a count that included the summary would pass on a
    // component that deduped the list to one row.
    const rows = [...document.querySelectorAll("li")].filter((li) =>
      li.querySelector("span[title]"),
    );
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.textContent?.includes("review"))).toBe(true);
    // Both are marked as colliding.
    expect(screen.getAllByText(/name collision/)).toHaveLength(2);
    // And the summary names BOTH sources, which is what lets the user
    // resolve it against the tool that actually decides.
    expect(screen.getByText(/~\/\.claude and headstate/)).toBeTruthy();
    // No winner is claimed anywhere.
    expect(screen.queryByText(/wins|overrid|shadow/i)).toBeNull();
  });

  /// A name nothing else claims is not marked. Without this the test
  /// above passes on a component that marks every row, and the mark
  /// becomes noise the reader learns to ignore.
  it("does not mark a definition that collides with nothing", () => {
    state.data = {
      definitions: [def({ name: "solo" })],
      collisions: [],
      unreadable: [],
    };
    render(<DefinitionsSection />);
    expect(screen.queryByText(/name collision/)).toBeNull();
  });
});
