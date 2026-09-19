import { describe, expect, it } from "vitest";
import { ALL_VIEWS, viewLabel } from "./filters";
import { VIEWS } from "../components/ViewSwitcher";

/// The header label table against the switcher's.
///
/// `viewLabel` is what the header and the view error boundary both name
/// a page with. The switcher is what the user clicked to get there, and
/// a page named something other than the menu item that opened it is
/// how a user doubts they are where they meant to be (#794).
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
    // here would be a menu entry leading to an unheaded page -- and
    // since the boundary uses the same table, an error panel that
    // could not say which page had failed.
    for (const { id } of VIEWS) {
      expect(viewLabel(id), id).toBeTruthy();
    }
  });

  /// The four pairs that are KNOWN to differ, quoted in full.
  ///
  /// Not an excuse list: it is the record of a real mismatch found
  /// while extracting the header's ternary chain (#1146), pinned so it
  /// cannot silently grow. Fixing any entry means deleting it from
  /// here, and the test below fails if a fifth appears.
  const KNOWN_DIFFERENT: Record<string, { header: string; switcher: string }> = {
    "my-prs": { header: "Pull requests", switcher: "My pull requests" },
    "to-review": { header: "Pull requests to review", switcher: "To review" },
    docker: { header: "Docker images", switcher: "Docker" },
    artifacts: { header: "Build artifacts", switcher: "Artifacts" },
  };

  it("matches the switcher everywhere except the four known pairs", () => {
    const differing: string[] = [];
    for (const { id, label } of VIEWS) {
      if (viewLabel(id) !== label) differing.push(id);
    }
    // Sorted compare, so the failure names WHICH view drifted rather
    // than just a count -- a count tells you something broke and not
    // what.
    expect(differing.sort()).toEqual(Object.keys(KNOWN_DIFFERENT).sort());
  });

  it("records the known pairs with their real current text", () => {
    // Pins both sides. Without this, someone could "fix" a mismatch by
    // editing the entry above rather than the label, and the test
    // would keep passing while the user still saw two names.
    for (const [id, { header, switcher }] of Object.entries(KNOWN_DIFFERENT)) {
      const entry = VIEWS.find((v) => v.id === id);
      expect(entry, id).toBeDefined();
      expect(entry?.label, id).toBe(switcher);
      expect(viewLabel(id as (typeof ALL_VIEWS)[number]), id).toBe(header);
    }
  });
});
