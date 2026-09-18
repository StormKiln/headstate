import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeDefinitions } from "@/api/tauri";

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

/// #1129: what the plugins table could not say.
describe("the definitions section", () => {
  it("lists a skill by name", () => {
    state.data = {
      definitions: [
        {
          kind: "skill",
          name: "shipping",
          named_in_frontmatter: true,
          description: "how we ship",
          path: "/h/.claude/skills/deploy/SKILL.md",
        },
      ],
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
        {
          kind: "agent",
          name: "reviewer",
          named_in_frontmatter: false,
          description: null,
          path: "/h/.claude/agents/reviewer.md",
        },
      ],
      unreadable: [],
    };
    render(<DefinitionsSection />);
    expect(screen.getByText(/from filename/)).toBeTruthy();
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
    state.data = { definitions: [], unreadable: [] };
    render(<DefinitionsSection />);
    expect(screen.getByText(/No skills, agents or commands are defined here/)).toBeTruthy();
  });

  /// An unreadable directory hides an unknown number, so the list is
  /// qualified rather than presented as complete.
  it("qualifies the list when a directory could not be read", () => {
    state.data = {
      definitions: [
        {
          kind: "command",
          name: "sync",
          named_in_frontmatter: true,
          description: null,
          path: "/h/.claude/commands/git/sync.md",
        },
      ],
      unreadable: ["/h/.claude/agents: permission denied"],
    };
    render(<DefinitionsSection />);
    expect(screen.getByText(/may not be all of them/)).toBeTruthy();
  });
});
