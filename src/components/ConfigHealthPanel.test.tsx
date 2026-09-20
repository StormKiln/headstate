import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeConfigHealth } from "@/api/tauri";

const state = vi.hoisted(() => ({
  data: undefined as ClaudeConfigHealth | undefined,
}));

vi.mock("@/api/hooks", () => ({
  useClaudeConfigHealth: () => ({
    data: state.data,
    error: null,
    isFetching: false,
    refetch: () => Promise.resolve(),
  }),
}));

const { ConfigHealthPanel } = await import("./ConfigHealthPanel");

const open = () => {
  render(<ConfigHealthPanel enabled />);
  fireEvent.click(screen.getByRole("button", { name: /configuration health/i }));
};

const empty: ClaudeConfigHealth = { repos: [], unreadableRoots: [], userFindings: [] };

/// #1217: silently-broken agent configuration across every repository.
describe("the configuration health panel", () => {
  /// The load-bearing rule. The proof is serde's own message with its
  /// line and column, and it is rendered VERBATIM -- a page that
  /// paraphrased it would discard the only actionable thing in the
  /// finding and turn a check into an opinion.
  it("states the parse error's own location rather than a summary", () => {
    state.data = {
      ...empty,
      repos: [
        {
          name: "widget",
          path: "/code/widget",
          verdict: "problem",
          findings: [
            {
              check: "settingsParse",
              severity: "problem",
              path: "/code/widget/.claude/settings.json",
              scope: "project",
              proof:
                "/code/widget/.claude/settings.json is not valid JSON (expected value at line 4 column 12).",
              undecidableKeys: [],
            },
          ],
        },
      ],
    };
    open();
    expect(screen.getByText(/line 4 column 12/)).toBeTruthy();
  });

  /// A repository whose local scope alone refused is not simply
  /// "unhealthy": the remedy is one specific file, and the keys that
  /// went dark are the consequence the reader actually feels.
  it("names the failing scope and the keys it put in doubt", () => {
    state.data = {
      ...empty,
      repos: [
        {
          name: "widget",
          path: "/code/widget",
          verdict: "problem",
          findings: [
            {
              check: "settingsParse",
              severity: "problem",
              path: "/code/widget/.claude/settings.local.json",
              scope: "local",
              proof: "not valid JSON at line 1 column 3",
              undecidableKeys: ["model", "permissions"],
            },
          ],
        },
      ],
    };
    open();
    expect(screen.getByText(".claude/settings.local.json")).toBeTruthy();
    expect(screen.getByText(/model, permissions cannot be resolved/)).toBeTruthy();
  });

  /// #1042 on this surface. "Could not check" gets its own word and its
  /// own count -- never folded into the passes, where it would read as
  /// coverage the sweep never established.
  it("counts a repository it could not check apart from the passes", () => {
    state.data = {
      ...empty,
      repos: [
        {
          name: "walled",
          path: "/code/walled",
          verdict: "unknown",
          findings: [
            {
              check: "unreadable",
              severity: "unknown",
              path: "/code/walled/.claude",
              scope: null,
              proof: "/code/walled/.claude (Permission denied (os error 13))",
              undecidableKeys: [],
            },
          ],
        },
        { name: "clean", path: "/code/clean", verdict: "pass", findings: [] },
      ],
    };
    open();
    expect(screen.getByText("1 could not be checked")).toBeTruthy();
    expect(screen.getByText("1 checked and clean")).toBeTruthy();
    expect(screen.getByText("0 broken")).toBeTruthy();
    // And the verdict is named in TEXT, not only in colour.
    expect(screen.getByText("[could not check]")).toBeTruthy();
  });

  /// The shortfall in the CENSUS. Without it, "N clean" is a claim about
  /// a machine that was only half looked at.
  it("says when the repository walk itself came back short", () => {
    state.data = {
      ...empty,
      repos: [{ name: "clean", path: "/code/clean", verdict: "pass", findings: [] }],
      unreadableRoots: ["/code/locked: permission denied"],
    };
    open();
    expect(screen.getByText(/may be missing repositories entirely/)).toBeTruthy();
    expect(screen.getByText("/code/locked: permission denied")).toBeTruthy();
  });

  /// A machine-wide finding is not attributed to any one repository.
  it("reports ~/.claude findings apart from the repositories", () => {
    state.data = {
      ...empty,
      userFindings: [
        {
          check: "definition",
          severity: "unknown",
          path: "/home/me/.claude/skills",
          scope: null,
          proof: "/home/me/.claude/skills: Permission denied (os error 13)",
          undecidableKeys: [],
        },
      ],
    };
    open();
    expect(screen.getByText(/loaded into every session on this machine/)).toBeTruthy();
  });

  /// "Clean" is only claimed when repositories were actually checked.
  /// An empty sweep says so rather than reading as an all-clear.
  it("does not call an empty sweep clean", () => {
    state.data = empty;
    open();
    expect(screen.getByText(/No repositories were scanned/)).toBeTruthy();
    expect(screen.queryByText(/Every settings file parsed/)).toBeNull();
  });

  /// Gated on the integration, like the panel it sits beneath.
  it("is absent when the integration is off", () => {
    render(<ConfigHealthPanel enabled={false} />);
    expect(screen.queryByRole("button", { name: /configuration health/i })).toBeNull();
  });
});
