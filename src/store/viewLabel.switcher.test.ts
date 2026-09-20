import { describe, expect, it } from "vitest";
import { ALL_VIEWS, viewLabel } from "./filters";
import { VIEWS } from "../components/ViewSwitcher";

/// The header label table against the switcher's.
///
/// `viewLabel` is what the header, the view error boundary and now the
/// menu all name a page with. #794's rule: a page named something other
/// than the menu item that opened it is how a user doubts they are
/// where they meant to be.
///
/// Four of twelve used to differ (#1185). They no longer can: `VIEWS`
/// DERIVES its label from `viewLabel` rather than declaring its own, so
/// there is one table and nothing to drift. These tests assert that the
/// derivation is actually in place -- a future edit that reintroduces a
/// hand-written `label:` would pass a type check and fail here.
describe("viewLabel against the switcher", () => {
  it("has a label for every registered view", () => {
    // Totality is a compile error via `Record<View, string>`, but that
    // only proves a key EXISTS. This proves none is empty -- a `""`
    // satisfies the type and renders a page with no heading at all.
    for (const view of ALL_VIEWS) {
      expect(viewLabel(view), view).toBeTruthy();
    }
  });

  it("names every view the switcher offers", () => {
    // The switcher is the route in. A view listed there with no label
    // would be a menu entry leading to an unheaded page -- and since
    // the boundary uses the same table, an error panel that could not
    // say which page had failed.
    for (const { id } of VIEWS) {
      expect(viewLabel(id), id).toBeTruthy();
    }
  });

  it("matches the switcher for EVERY view, with no exceptions", () => {
    // The whole point of #1185, and the test that replaced a
    // `KNOWN_DIFFERENT` table of four accepted mismatches. There is
    // nothing to except now: a divergence means someone reintroduced a
    // second source of truth.
    const differing = VIEWS.filter((v) => viewLabel(v.id) !== v.label).map((v) => v.id);
    expect(differing.sort()).toEqual([]);
  });

  it("gives the two vaguest pages their specific names", () => {
    // Pinned by value, not just by agreement: before #1185 the page
    // specifically about your own pull requests was headed "Pull
    // requests", which was also the generic fallback -- so the two
    // agreeing on the WRONG word would satisfy the test above.
    expect(viewLabel("my-prs")).toBe("My pull requests");
    expect(viewLabel("to-review")).toBe("To review");
  });
});
