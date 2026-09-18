import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeEffectiveSettings } from "@/api/tauri";

const state = vi.hoisted(() => ({
  data: undefined as ClaudeEffectiveSettings | undefined,
  repo: "/code/widget" as string | undefined,
}));

vi.mock("@/api/hooks", () => ({
  useClaudeEffectiveSettings: () => ({ data: state.data, error: null }),
}));
vi.mock("@/store/filters", () => ({
  useActiveFilters: () => ({ repo: state.repo }),
}));

const { EffectiveSettingsPanel } = await import("./EffectiveSettingsPanel");

const open = () => {
  render(<EffectiveSettingsPanel enabled />);
  fireEvent.click(screen.getByRole("button", { name: /settings Claude Code reads/i }));
};

/// #1130: the rule behind a denial, which the page could show the
/// effect of but not the cause.
describe("the effective settings panel", () => {
  it("names the file each value came from", () => {
    state.data = {
      keys: [
        {
          key: "model",
          winner: { origin: "local", value: "opus" },
          contributions: [
            { origin: "user", value: "sonnet" },
            { origin: "local", value: "opus" },
          ],
          undecidable: false,
        },
      ],
      unreadable: [],
    };
    open();
    expect(screen.getByText("~/.claude/settings.json")).toBeTruthy();
    expect(screen.getByText(".claude/settings.local.json")).toBeTruthy();
  });

  /// The winner is marked in TEXT, not only by colour: colour alone is
  /// not an answer for a reader who cannot see it.
  it("marks which value applies and which is overridden", () => {
    state.data = {
      keys: [
        {
          key: "model",
          winner: { origin: "local", value: "opus" },
          contributions: [
            { origin: "user", value: "sonnet" },
            { origin: "local", value: "opus" },
          ],
          undecidable: false,
        },
      ],
      unreadable: [],
    };
    open();
    expect(screen.getByText("[applies]")).toBeTruthy();
    expect(screen.getByText("[overridden]")).toBeTruthy();
  });

  /// The load-bearing case, and #1042's shape. A scope outranking the
  /// best readable one could not be parsed, so the merged value is
  /// UNKNOWN -- printing the loser as the winner would be a confident
  /// wrong answer.
  it("refuses to name a winner when a higher scope could not be read", () => {
    state.data = {
      keys: [
        {
          key: "model",
          winner: null,
          contributions: [{ origin: "user", value: "sonnet" }],
          undecidable: true,
        },
      ],
      unreadable: [
        { origin: "local", path: "/code/widget/.claude/settings.local.json", detail: "bad json" },
      ],
    };
    open();
    expect(screen.getByText(/Cannot say which value wins/)).toBeTruthy();
    expect(screen.queryByText("[applies]")).toBeNull();
  });

  /// A file Claude Code cannot parse is one it ignores entirely, so
  /// every rule in it is already doing nothing. That outranks any value
  /// below it.
  it("shows a refusal above the values", () => {
    state.data = {
      keys: [],
      unreadable: [
        { origin: "project", path: "/code/widget/.claude/settings.json", detail: "bad json here" },
      ],
    };
    open();
    expect(screen.getByText("bad json here")).toBeTruthy();
  });

  /// Two of the three scopes belong to a repository, so without one
  /// there is no question to answer. Rendering the user scope alone and
  /// calling it effective would be the wrong answer.
  it("asks for a repository rather than showing a partial merge", () => {
    state.repo = undefined;
    state.data = undefined;
    open();
    expect(screen.getByText(/Pick a repository/)).toBeTruthy();
    state.repo = "/code/widget";
  });

  /// Gated on the integration, so a user who has not enabled it is not
  /// offered a view of files the app is not otherwise reading.
  it("is absent when the integration is off", () => {
    render(<EffectiveSettingsPanel enabled={false} />);
    expect(screen.queryByRole("button", { name: /settings Claude Code reads/i })).toBeNull();
  });
});
