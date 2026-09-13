import { fireEvent, screen } from "@testing-library/react";
// ViewSwitcher reads `useUiPrefs`, so it needs a QueryClient.
import { renderWithQuery as render } from "@/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import { useFilters } from "../store/filters";
import { VIEWS, ViewSwitcher } from "./ViewSwitcher";

const EMPTY = { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {} } as const;

describe("ViewSwitcher", () => {
  /// PR Stats leads the menu (#823).
  ///
  /// The v5.13.0 tracker asked for this and it was the one scope item that
  /// did not land -- the rebuild shipped the org/member sidebar, the
  /// Mine/Others views and the load gating, and left the entry ninth of
  /// ten. Nothing asserted the position, so nothing noticed.
  ///
  /// Asserts the FIRST entry rather than "contains PR Stats": the latter
  /// passed throughout the period the item was outstanding, which is the
  /// difference between a test and a guard. `VIEWS` is the array the menu
  /// renders, so this is the order the user sees -- `ALL_VIEWS` in
  /// `store/filters.ts` is kept in step for readers, but only derives a
  /// type.
  /// The capability gate, and specifically the case the escape hatches
  /// would otherwise defeat (#916).
  ///
  /// `ViewSwitcher` honours `hidden_views` loosely on purpose: a user
  /// sitting on a view they hid keeps it, via the `id === view` hatch. That
  /// is right for a preference and wrong for a capability -- a switched-off
  /// integration has no page behind the entry, so being the current view
  /// must not make it offerable.
  ///
  /// Asserted with `view` set TO the gated id, because that is the only
  /// configuration where the two rules disagree. A test that left `view`
  /// elsewhere would pass against a gate placed after the hatch, which is
  /// the bug this is written to catch.
  it("does not offer the Claude Code view while the capability is off", () => {
    useFilters.setState({ filtersByView: EMPTY, view: "claude-code" } as never);
    render(<ViewSwitcher counts={{ "to-review": 0 }} />);
    fireEvent.click(screen.getByRole("button", { name: /claude code|my pull requests/i }));
    // Absent from the MENU, not merely from a collapsed control.
    expect(screen.queryByRole("menuitem", { name: /claude code/i })).toBeNull();
    // A view that is NOT gated proves the menu rendered at all, rather
    // than the assertion above passing because nothing is on screen.
    expect(screen.getByRole("menuitem", { name: /worktrees/i })).toBeTruthy();
  });

  /// Absent prefs read as OFF, not as on.
  ///
  /// The hook returns `undefined` before the first fetch resolves, and a
  /// capability that defaults to "available" during that window would flash
  /// a view the user has not enabled. Absent is not enabled -- the same
  /// direction every other unknown in this codebase fails in.
  it("treats unknown prefs as the capability being off", () => {
    useFilters.setState({ filtersByView: EMPTY, view: "my-prs" } as never);
    render(<ViewSwitcher counts={{ "to-review": 0 }} />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    expect(screen.queryByRole("menuitem", { name: /claude code/i })).toBeNull();
  });

  it("offers PR Stats first", () => {
    expect(VIEWS[0].id).toBe("pr-stats");
    expect(VIEWS[0].label).toBe("PR Stats");
    // And the non-pull-request view stays last, which is the other half of
    // the ordering rule both lists record.
    expect(VIEWS[VIEWS.length - 1].id).toBe("system-health");
  });

  beforeEach(() =>
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "my-prs" }),
  );

  it("names the current view when collapsed", () => {
    render(<ViewSwitcher />);
    expect(screen.getByRole("button", { name: /my pull requests/i })).toBeTruthy();
    // The others are not visible until expanded.
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  // Names rather than a count: a bare length assertion has to be edited
  // every time a view is added and says nothing about which are missing.
  it("lists every view when expanded", () => {
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    for (const label of [/my pull requests/i, /to review/i, /worktrees/i, /docker/i]) {
      expect(screen.getByRole("menuitem", { name: label })).toBeTruthy();
    }
  });

  it("switches view and closes", () => {
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /worktrees/i }));
    expect(useFilters.getState().view).toBe("worktrees");
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("marks the current view so the menu is not ambiguous", () => {
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" });
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /to review/i }));
    const current = screen.getByRole("menuitem", { name: /to review/i });
    expect(current.getAttribute("aria-current")).toBe("true");
  });

  it("badges a count when one is supplied", () => {
    render(<ViewSwitcher counts={{ "to-review": 4 }} />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    expect(screen.getByText("4")).toBeTruthy();
  });

  // A menu that survives Escape or an outside click stays open behind
  // whatever the user does next.
  it("closes on Escape", () => {
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("closes on a click outside", () => {
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  // Switching views must not carry one view's repo selection into
  // another, which has an entirely different repo list.
  it("does not leak filters between views", () => {
    useFilters.setState({
      filtersByView: { "my-prs": { repo: "octocat/hello-world" }, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {} },
      view: "my-prs",
    });
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /to review/i }));
    const s = useFilters.getState();
    expect(s.filtersByView[s.view].repo).toBeUndefined();
  });
});
