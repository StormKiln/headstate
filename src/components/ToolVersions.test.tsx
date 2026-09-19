import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ToolReport } from "@/api/tauri";

const state = vi.hoisted(() => ({
  data: undefined as ToolReport[] | undefined,
  isError: false,
}));

vi.mock("@/api/hooks", () => ({
  useToolVersions: () => ({ data: state.data, isError: state.isError }),
}));

const { ToolVersions } = await import("./ToolVersions");

const tool = (over: Partial<ToolReport> = {}): ToolReport => ({
  name: "git",
  path: "/usr/bin/git",
  version: { state: "ok", found: "2.50.1" },
  matters: "Every worktree and branch view.",
  ...over,
});

/// #1154: which version, not merely whether it exists.
describe("the tool versions panel", () => {
  it("shows a current tool's version plainly", () => {
    state.data = [tool()];
    render(<ToolVersions />);
    expect(screen.getByText("2.50.1")).toBeTruthy();
  });

  /// BOTH numbers. "2.30 is older than the 2.41 this needs" is
  /// actionable; "your git is too old" is not.
  it("names both versions when a tool is too old", () => {
    state.data = [
      tool({ version: { state: "tooOld", found: "2.30.0", required: "2.41.0" } }),
    ];
    render(<ToolVersions />);
    expect(screen.getByText(/2\.30\.0 — older than the 2\.41\.0/)).toBeTruthy();
  });

  /// The load-bearing distinction (`install.rs:341`): "could not tell"
  /// must not render as "too old", because the remedies differ and one
  /// of them sends a user to upgrade something that may be current.
  it("renders cannot-tell distinctly from too-old", () => {
    state.data = [
      tool({ version: { state: "cannotTell", detail: "unrecognised output" } }),
    ];
    render(<ToolVersions />);
    expect(screen.getByText(/could not tell/)).toBeTruthy();
    expect(screen.queryByText(/older than/)).toBeNull();
  });

  /// And missing is its own state again -- three remedies, three
  /// sentences.
  it("renders not-found distinctly from both", () => {
    state.data = [tool({ version: { state: "notFound" }, path: null })];
    render(<ToolVersions />);
    expect(screen.getByText("not found")).toBeTruthy();
    expect(screen.queryByText(/could not tell/)).toBeNull();
    expect(screen.queryByText(/older than/)).toBeNull();
  });

  /// What stops working is shown only when something is wrong: a
  /// working tool needs no explanation, and showing one for all four
  /// would bury the one that matters.
  it("says what stops working only when a tool is not ok", () => {
    state.data = [tool(), tool({ name: "gh", version: { state: "notFound" }, matters: "Your GitHub token." })];
    render(<ToolVersions />);
    expect(screen.getByText(/Your GitHub token/)).toBeTruthy();
    expect(screen.queryByText(/Every worktree and branch view/)).toBeNull();
  });

  /// A failed probe renders NOTHING rather than "not found" for
  /// everything -- that would be four confident wrong answers at once.
  it("renders nothing when the probe failed", () => {
    state.data = undefined;
    state.isError = true;
    const { container } = render(<ToolVersions />);
    expect(container.textContent).toBe("");
    state.isError = false;
  });
});
