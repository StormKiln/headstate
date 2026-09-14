// `?raw` rather than `node:fs`, for the reason `App.lazy.test.tsx` states
// at length: this project deliberately carries no `@types/node`, so
// `readFileSync` does not typecheck here. Vite's raw import gives the
// file's text with no types needed.
import readme from "../README.md?raw";
import { describe, expect, it } from "vitest";
import { ALL_VIEWS } from "@/store/filters";

/// Every view the app offers is described in the README (#962).
///
/// The Claude Code views shipped across five issues (#910, #917, #918,
/// #919, #921) and were absent from the README's feature list entirely.
/// That mattered more than a normal documentation gap because the view is
/// gated OFF by default: `ViewSwitcher` removes the entry outright while
/// `claude_integrations_enabled` is false, so there is no greyed row, no
/// mention, and no empty state. The absence of the feature is
/// indistinguishable from the feature not existing, and the README was the
/// one place a user could have found out otherwise.
///
/// # Why a test rather than remembering
///
/// The list grew to nine views one at a time, and the README kept up eight
/// times out of nine. A ninth entry added by hand is a ninth chance to
/// forget — and the failure is silent, because nothing renders a README.
/// `ALL_VIEWS` is the canonical list every route and preference is keyed
/// on, so deriving the expectation from it means a tenth view is covered
/// without anyone adding a line here. That is the rule `invariants.rs`
/// states for Rust and the reason `App.lazy.test.tsx` reads its own
/// source: a property about a SET cannot be pinned by naming the members.
///
/// # What this does not assert
///
/// Not the quality of the prose, and not that it is current — a test
/// cannot tell either. It asserts only that each view is mentioned by a
/// name a reader would search for, which is the part that was actually
/// missing. A view whose section says nothing useful will pass; a view
/// with no section at all will not.
describe("the README documents every view", () => {
  /// The heading a reader would look for, per view id.
  ///
  /// Deliberately NOT the view id or the sidebar label: the README names
  /// two of these by what they show rather than by their menu entry --
  /// "Pull request list" for `my-prs`, "Unresolved conversations" for
  /// `to-review` -- and that is the better prose for someone who has not
  /// opened the app yet. The first draft of this test asserted the menu
  /// labels, failed on exactly those two, and the README was right.
  ///
  /// Keyed off `ALL_VIEWS` rather than listed independently: the test
  /// below fails if this map and the canonical list ever disagree, so a
  /// new view cannot be silently exempted by leaving it out here.
  const HEADING: Record<(typeof ALL_VIEWS)[number], string> = {
    "pr-stats": "PR Stats",
    "my-prs": "Pull request list",
    "to-review": "Unresolved conversations",
    worktrees: "Worktrees",
    branches: "Branches",
    docker: "Docker",
    artifacts: "Artifacts",
    packages: "Package updates",
    "claude-md": "CLAUDE.md",
    "claude-code": "Claude Code",
    "system-health": "System health",
  };

  it("covers every entry in ALL_VIEWS, with no view left unmapped", () => {
    // The map and the canonical list must agree in BOTH directions: a new
    // view missing from the map would otherwise never be checked, which is
    // exactly the silent exemption this test exists to prevent.
    expect(Object.keys(HEADING).sort()).toEqual([...ALL_VIEWS].sort());
  });

  it.each([...ALL_VIEWS])("describes %s", (view) => {
    // The HEADING form `**Name.**`, not a bare substring. "Claude Code"
    // appears in the Worktrees section too (the Claudify action), so a
    // substring check passed even with the whole Claude Code section
    // deleted -- caught by sabotaging this very test, which is the one
    // way to find an assertion that cannot fail.
    const heading = `**${HEADING[view]}.**`;
    expect(
      readme.includes(heading),
      `The README does not mention "${heading}". Every view in ALL_VIEWS needs a ` +
        `section a reader can find, because a view absent from the README is a view ` +
        `a user may never learn exists -- and for a capability-gated one like ` +
        `claude-code, there is no entry in the switcher to discover either (#962).`,
    ).toBe(true);
  });

  /// The gated view says it is gated, and where the switch is.
  ///
  /// Naming it is not enough on its own: a reader who finds "Claude Code"
  /// in the README and then cannot find it in the app has been told the
  /// feature exists and not how to reach it, which is a worse outcome than
  /// silence. The default stays `false` — that is deliberate and #962 does
  /// not dispute it — so the README carries what the switcher cannot.
  it("says the Claude Code views are off by default and where to turn them on", () => {
    const section = readme.slice(readme.indexOf("**Claude Code.**"));
    expect(section).toMatch(/off by default/i);
    expect(section).toMatch(/Settings/);
  });
});
