import { fireEvent, screen, within } from "@testing-library/react";
// ViewSwitcher reads `useUiPrefs`, so it needs a QueryClient.
import { renderWithQuery as render, stubViewport } from "@/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Filters } from "../lib/derive";
import { MOBILE_BREAKPOINT } from "../lib/useIsMobile";
import { ALL_VIEWS, type View, useFilters } from "../store/filters";
import { GROUPS, VIEWS, ViewSwitcher } from "./ViewSwitcher";

/// What `useUiPrefs` answers, per test.
///
/// `undefined` is the real pre-fetch shape and the default here, so the
/// capability tests below keep asserting against it exactly as they did
/// before this mock existed. The grouping tests set it to exercise
/// `hidden_views`, which is the route an ordinary user takes.
let uiPrefs: { hidden_views: string[]; claude_integrations_enabled: boolean } | undefined;

vi.mock("../api/hooks", () => ({
  useUiPrefs: () => ({ prefs: uiPrefs, set: () => Promise.resolve() }),
}));

/// An empty filter set for every view, DERIVED from `ALL_VIEWS`.
///
/// This was two hand-written object literals -- here and in the
/// filter-leak test below -- each cast `as never`, which is precisely
/// what made them useless as a check: `as never` silences the assignment,
/// so a view added to `ALL_VIEWS` and missing from these fixtures was
/// caught by nothing and the tests kept passing against a store that no
/// longer matched the type (#1024).
///
/// Built from `ALL_VIEWS` and typed `Record<View, Filters>`, it cannot
/// fall behind: a new view is in it the moment it is in the type, and no
/// cast is needed because the value genuinely has the shape.
const EMPTY: Record<View, Filters> = Object.fromEntries(
  ALL_VIEWS.map((id) => [id, {}]),
) as Record<View, Filters>;

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
    useFilters.setState({ filtersByView: EMPTY, view: "claude-code" });
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
    useFilters.setState({ filtersByView: EMPTY, view: "my-prs" });
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

  beforeEach(() => {
    // The pre-fetch shape, which is what this block was written against.
    uiPrefs = undefined;
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "my-prs" });
  });

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
      filtersByView: { ...EMPTY, "my-prs": { repo: "octocat/hello-world" } },
      view: "my-prs",
    });
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /to review/i }));
    const s = useFilters.getState();
    expect(s.filtersByView[s.view].repo).toBeUndefined();
  });
});

/// The grouping itself (#1017), its totality (#1024), the empty-group
/// rule (#1018), the phone (#1020) and the ARIA shape (#1022).
describe("ViewSwitcher grouping", () => {
  afterEach(() => stubViewport(null));

  beforeEach(() => {
    stubViewport(1400);
    // Every group populated unless a test says otherwise.
    uiPrefs = { hidden_views: [], claude_integrations_enabled: true };
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "my-prs" });
  });

  /// Every view reaches exactly one group, derived from `ALL_VIEWS`.
  ///
  /// The type makes `group` required, so a `VIEWS` entry cannot omit it.
  /// What the type cannot see is whether `VIEWS` covers `ALL_VIEWS` at
  /// all -- an array type does not force totality over a union, so the
  /// array can be short and still compile. That is #675's gap, and until
  /// now nothing closed it here.
  ///
  /// Derived in both directions rather than naming the twelve ids: a
  /// hardcoded second list is one that gets edited to match whatever the
  /// code does and stops checking anything.
  it("assigns every view in ALL_VIEWS to exactly one group", () => {
    for (const id of ALL_VIEWS) {
      const entries = VIEWS.filter((v) => v.id === id);
      expect(entries).toHaveLength(1);
      expect(GROUPS.map((g) => g.id)).toContain(entries[0].group);
    }
    // And no `VIEWS` entry names a view that is not in `ALL_VIEWS`.
    expect(VIEWS).toHaveLength(ALL_VIEWS.length);
  });

  /// A group with a label and no possible member is a heading that
  /// renders on no machine in no configuration -- the kind of leftover a
  /// refactor produces.
  it("declares no group that no view can ever fill", () => {
    for (const g of GROUPS) {
      expect(VIEWS.filter((v) => v.group === g.id).length).toBeGreaterThan(0);
    }
  });

  it("renders a heading for each group, labelling its items", () => {
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    const groups = screen.getAllByRole("group");
    // `role="group"` with an accessible NAME, which is the whole point of
    // the `aria-labelledby` -- a bare `<h3>` in `role="menu"` is outside
    // its content model and takes the label with it when it is ignored.
    expect(groups.map((g) => g.getAttribute("aria-label") ?? g.textContent)).toBeTruthy();
    expect(screen.getByRole("group", { name: /pull requests/i })).toBeTruthy();
    expect(screen.getByRole("group", { name: /^system$/i })).toBeTruthy();
    // The items live INSIDE their group, not as siblings of it.
    const prGroup = screen.getByRole("group", { name: /pull requests/i });
    expect(
      within(prGroup).getByRole("menuitem", { name: /my pull requests/i }),
    ).toBeTruthy();
  });

  /// #1018, the `hidden_views` route.
  ///
  /// Deliberately NOT the Claude capability path, which is the one a test
  /// reaches for first and is also the one an ordinary user cannot reach.
  /// Repositories, Builds and System are all fully hideable -- none of
  /// their members is in `ALWAYS_OFFERED` -- so this is a state any user
  /// produces in three clicks in Settings, and it must render NOTHING
  /// rather than a heading over empty space.
  it("renders no heading for a group whose every member is hidden", () => {
    // EVERY member, which is three since #1023 added Repositories to this
    // group. Hiding two of the three would leave the heading standing
    // correctly, so a list that fell behind the group's membership would
    // make this test assert the opposite of its own name -- which is why
    // it is spelled out rather than kept at the two it used to be.
    uiPrefs = {
      hidden_views: ["worktrees", "branches", "repositories"],
      claude_integrations_enabled: true,
    };
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    // The group is gone, not merely empty.
    //
    // `^repos$` rather than `^repositories$`: the group is labelled
    // "Repos" as of #1023, because it now CONTAINS a view called
    // Repositories and a heading indistinguishable by name from one of
    // its own children is unresolvable to a screen reader querying by
    // accessible name. The epic's grouping spells it that way for the
    // same reason.
    expect(screen.queryByRole("group", { name: /^repos$/i })).toBeNull();
    expect(screen.queryByText(/^repos$/i)).toBeNull();
    // Its members are gone too, and an unaffected group still renders --
    // so this is not passing because the menu failed to open.
    expect(screen.queryByRole("menuitem", { name: /worktrees/i })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: /^repositories$/i })).toBeNull();
    expect(screen.getByRole("group", { name: /^builds$/i })).toBeTruthy();
  });

  /// The same rule via the capability, and the AI group specifically --
  /// the heading the issue is named for.
  it("renders no AI heading when the capability is off and CLAUDE.md is hidden", () => {
    uiPrefs = { hidden_views: ["claude-md"], claude_integrations_enabled: false };
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    expect(screen.queryByRole("group", { name: /^ai$/i })).toBeNull();
  });

  /// The property that makes "render nothing" safe (#1018).
  ///
  /// The current view is always offered, so the group the user is
  /// standing in always has at least that member and can never be the
  /// one that is dropped. Asserted with the view set to a member of an
  /// otherwise fully hidden group, which is the only configuration where
  /// the rules could disagree.
  it("never drops the group holding the current view, even when it is hidden", () => {
    // All three members hidden since #1023, so the group survives ONLY
    // through the current-view hatch. With a member left visible it would
    // survive for an ordinary reason and this would stop testing the
    // hatch at all.
    uiPrefs = {
      hidden_views: ["worktrees", "branches", "repositories"],
      claude_integrations_enabled: true,
    };
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "worktrees" });
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /worktrees/i }));
    // "Repos" -- see the rename's reasoning above.
    const repos = screen.getByRole("group", { name: /^repos$/i });
    expect(within(repos).getByRole("menuitem", { name: /worktrees/i })).toBeTruthy();
    // The hidden siblings stay hidden -- the hatch is for the current
    // view only, not for its whole group.
    expect(screen.queryByRole("menuitem", { name: /branches/i })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: /^repositories$/i })).toBeNull();
  });

  /// No group may share a label with a view (#1023).
  ///
  /// The collision this pins is not hypothetical: #1017 labelled the
  /// repos group "Repositories" while it held two views neither of which
  /// was called that, and #1023's third member IS called that -- so the
  /// menu rendered a "Repositories" heading with a "Repositories" item
  /// beneath it. A reader cannot tell which is which, and a screen reader
  /// querying by accessible name genuinely cannot resolve it: the group
  /// and one of its own children answer to the same string.
  ///
  /// Asserted over both SETS rather than on the one pair that collided,
  /// for the reason this codebase states about every such guard: a
  /// property about a set cannot be pinned by naming the members, and the
  /// next label added is the one nobody checks.
  ///
  /// Case-insensitive, because the ambiguity is about what a person reads
  /// and hears rather than about bytes.
  it("gives no group the same label as a view", () => {
    const groupLabels = GROUPS.map((g) => g.label.toLowerCase());
    for (const v of VIEWS) {
      expect(
        groupLabels,
        `The view "${v.label}" shares its label with a group heading. A heading ` +
          `indistinguishable by name from one of its own children is ambiguous to ` +
          `a reader and unresolvable to a screen reader querying by accessible ` +
          `name (#1023). Rename the GROUP: a view's name is user-facing and has to ` +
          `agree with its header, its README section and its Settings checkbox.`,
      ).not.toContain(v.label.toLowerCase());
    }
    // Guards the guard: both lists are non-empty, or the assertion above
    // is vacuously true.
    expect(groupLabels.length).toBeGreaterThan(1);
    expect(VIEWS.length).toBeGreaterThan(1);
  });

  /// #1022: the attribute's ABSENCE is how "not current" is spelled.
  ///
  /// The defect this replaces was `aria-current={id === view}`, which
  /// React serialises to the literal string "false" on every other item.
  /// Asserting `toBeNull()` on a non-current item is what distinguishes
  /// the fix from the bug -- `"false"` is truthy as a string and passes
  /// any assertion that merely checks the current one.
  it("omits aria-current entirely on the items that are not current", () => {
    useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" });
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /to review/i }));
    expect(
      screen.getByRole("menuitem", { name: /to review/i }).getAttribute("aria-current"),
    ).toBe("true");
    for (const name of [/worktrees/i, /docker/i, /system health/i]) {
      expect(screen.getByRole("menuitem", { name }).getAttribute("aria-current")).toBeNull();
    }
  });

  /// #1403: the headings ARE painted on the phone. #1020 hid them on the
  /// premise that the sheet "has 288px" of vertical space -- but `w-72`
  /// is the sheet's WIDTH; a left sheet spans the full height.
  ///
  /// The viewport is stubbed at 390px and ASSERTED to be narrow, because
  /// a test that mocks the build flag without `matchMedia` runs at
  /// desktop width while claiming to be a phone -- and would pass against
  /// a component that ignored the width entirely.
  it("shows every group heading at phone width, each labelling its group", () => {
    const viewport = stubViewport(390);
    expect(window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`).matches).toBe(true);
    expect(viewport).toBeTruthy();
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    const groups = screen.getAllByRole("group");
    expect(groups.length).toBeGreaterThan(1);
    for (const group of groups) {
      const heading = document.getElementById(group.getAttribute("aria-labelledby") ?? "");
      expect(heading).not.toBeNull();
      expect(heading?.hasAttribute("hidden")).toBe(false);
      expect(heading?.textContent?.trim()).not.toBe("");
    }
    // The group keeps its accessible name, and the entries are still there.
    expect(screen.getByRole("group", { name: /pull requests/i })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /my pull requests/i })).toBeTruthy();
  });

  it("shows the group headings at desktop width", () => {
    stubViewport(1400);
    render(<ViewSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /my pull requests/i }));
    expect(document.getElementById("view-group-pull-requests")?.hasAttribute("hidden")).toBe(
      false,
    );
  });
});
