import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeMcpInventory, ClaudeMcpServer } from "@/api/tauri";

const state = vi.hoisted(() => ({
  data: undefined as ClaudeMcpInventory | undefined,
  isError: false,
  repo: undefined as string | undefined,
}));

vi.mock("@/store/filters", () => ({
  useActiveFilters: () => ({ repo: state.repo }),
}));

vi.mock("../api/hooks", () => ({
  useClaudeMcpServers: () => ({
    data: state.data,
    isLoading: false,
    isError: state.isError,
    error: state.isError ? new Error("no home directory is set") : null,
    refetch: () => {},
  }),
}));

const { McpSection, mcpInForce } = await import("./ClaudePluginsPage");

/// #1216: the configuration blind spot the app could not name.
describe("the MCP section", () => {
  it("lists a user-scope server with its transport", () => {
    state.data = {
      servers: [
        {
          name: "codegraph",
          transport: { kind: "stdio", command: "npx -y codegraph-mcp" },
          origin: "user",
          scopeDetail: null,
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: 160_000,
    };
    render(<McpSection />);
    expect(screen.getByText("codegraph")).toBeTruthy();
    expect(screen.getByText("npx -y codegraph-mcp")).toBeTruthy();
    // `getAllByText`: the section's own description names the file too,
    // so this asserts the row carries the scope rather than asserting
    // the string appears somewhere on screen.
    expect(screen.getAllByText("~/.claude.json").length).toBeGreaterThan(0);
  });

  /// The load-bearing column. A server defined for one project must be
  /// visibly attributed to THAT project -- a scope without the project
  /// name has told the reader half the answer.
  it("attributes a project-scope server to its project", () => {
    state.data = {
      servers: [
        {
          name: "atlassian",
          transport: { kind: "url", url: "https://mcp.atlassian.com" },
          origin: "project",
          scopeDetail: "/Users/x/code/one",
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: 160_000,
    };
    render(<McpSection />);
    expect(screen.getByText(/this project only/)).toBeTruthy();
    expect(screen.getByText("/Users/x/code/one")).toBeTruthy();
  });

  /// The whole point of the ticket. A configuration that could not be
  /// read must NOT render as "no MCP servers are configured".
  it("reports a refusal rather than zero servers", () => {
    state.data = {
      servers: [],
      unreadable: [
        {
          origin: "user",
          path: "/h/.claude.json",
          detail:
            "/h/.claude.json did not parse as JSON: EOF while parsing an object at line 3 column 8.",
        },
      ],
      truncated: false,
      sizeBytes: 160_000,
    };
    render(<McpSection />);
    expect(screen.getByText(/did not parse as JSON/)).toBeTruthy();
    expect(screen.getByText(/not the same as having none configured/)).toBeTruthy();
    // The sentence that would have been the defect.
    expect(screen.queryByText("No MCP servers are configured.")).toBeNull();
  });

  /// A measured zero is a real answer and says so plainly -- and must
  /// be distinguishable from the refusal above.
  it("says none are configured when the read succeeded and found none", () => {
    state.data = { servers: [], unreadable: [], truncated: false, sizeBytes: 1024 };
    render(<McpSection />);
    expect(screen.getByText("No MCP servers are configured.")).toBeTruthy();
    expect(screen.queryByText(/could not be read/)).toBeNull();
  });

  /// A query that failed outright is still not an empty inventory.
  it("reports a failed read rather than an empty list", () => {
    state.data = undefined;
    state.isError = true;
    render(<McpSection />);
    expect(screen.getByText(/could not be read/)).toBeTruthy();
    expect(screen.queryByText("No MCP servers are configured.")).toBeNull();
    state.isError = false;
  });

  /// A plugin-shipped server names the plugin, which is what
  /// `Origin::Plugin` exists to carry.
  it("names the plugin that ships a server", () => {
    state.data = {
      servers: [
        {
          name: "github",
          transport: { kind: "stdio", command: "gh-mcp" },
          origin: "plugin",
          scopeDetail: "github-plugin",
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: null,
    };
    render(<McpSection />);
    expect(screen.getByText(/a plugin's .mcp.json/)).toBeTruthy();
    expect(screen.getByText("github-plugin")).toBeTruthy();
  });

  /// An entry whose transport we do not recognise is LISTED and named,
  /// not dropped and not rendered blank.
  it("names an unrecognised transport rather than showing nothing", () => {
    state.data = {
      servers: [
        {
          name: "future",
          transport: { kind: "unknown" },
          origin: "user",
          scopeDetail: null,
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: null,
    };
    render(<McpSection />);
    expect(screen.getByText("future")).toBeTruthy();
    expect(screen.getByText(/transport not recognised/)).toBeTruthy();
  });

  /// The per-repository half of #1216. A project-scope server belonging
  /// to ANOTHER project is marked as not applying here -- which is the
  /// confusion the feature exists to remove.
  it("marks a server from another project as not in force here", () => {
    state.repo = "/Users/x/code/two";
    state.data = {
      servers: [
        {
          name: "atlassian",
          transport: { kind: "url", url: "https://a" },
          origin: "project",
          scopeDetail: "/Users/x/code/one",
        },
        {
          name: "codegraph",
          transport: { kind: "stdio", command: "npx" },
          origin: "user",
          scopeDetail: null,
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: null,
    };
    render(<McpSection />);
    // Exactly one row is marked, and it is the other project's.
    expect(screen.getAllByText(/not in this repository/).length).toBe(1);
    state.repo = undefined;
  });

  /// With no repository selected the question has no subject, so no row
  /// is marked -- rather than every project row being marked absent for
  /// a repository nobody named.
  it("marks nothing when no repository is selected", () => {
    state.repo = undefined;
    state.data = {
      servers: [
        {
          name: "atlassian",
          transport: { kind: "url", url: "https://a" },
          origin: "project",
          scopeDetail: "/Users/x/code/one",
        },
      ],
      unreadable: [],
      truncated: false,
      sizeBytes: null,
    };
    render(<McpSection />);
    expect(screen.queryByText(/not in this repository/)).toBeNull();
  });
});

/// The rule itself, mirroring `claude::mcp::Inventory::in_force`.
describe("mcpInForce", () => {
  const project = (path: string): ClaudeMcpServer => ({
    name: "s",
    transport: { kind: "stdio", command: "c" },
    origin: "project",
    scopeDetail: path,
  });

  it("applies a user-scope server everywhere", () => {
    const s: ClaudeMcpServer = {
      name: "s",
      transport: { kind: "stdio", command: "c" },
      origin: "user",
      scopeDetail: null,
    };
    expect(mcpInForce(s, "/any/repo")).toBe(true);
  });

  it("applies a plugin-scope server everywhere", () => {
    const s: ClaudeMcpServer = {
      name: "s",
      transport: { kind: "stdio", command: "c" },
      origin: "plugin",
      scopeDetail: "some-plugin",
    };
    expect(mcpInForce(s, "/any/repo")).toBe(true);
  });

  it("confines a project-scope server to its own project", () => {
    expect(mcpInForce(project("/a/b"), "/a/b")).toBe(true);
    expect(mcpInForce(project("/a/b"), "/a/c")).toBe(false);
  });

  /// A trailing separator must not hide a project's servers -- the same
  /// normalisation the Rust side applies.
  it("ignores a trailing separator", () => {
    expect(mcpInForce(project("/a/b/"), "/a/b")).toBe(true);
    expect(mcpInForce(project("/a/b"), "/a/b/")).toBe(true);
  });

  /// A Windows trailing separator must normalise too, matching
  /// `claude::mcp::normalise`. Trimming only `/` was correct on Unix
  /// and wrong on Windows -- the same project reported twice, the
  /// second copy showing none of its servers.
  it("ignores a trailing backslash", () => {
    expect(mcpInForce(project("C:\\code\\one\\"), "C:\\code\\one")).toBe(true);
    expect(mcpInForce(project("C:\\code\\one"), "C:\\code\\one\\")).toBe(true);
  });

  /// No repository means no answer, not a false one.
  it("answers null when no repository is selected", () => {
    expect(mcpInForce(project("/a/b"), undefined)).toBeNull();
  });
});
