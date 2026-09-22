import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ArgvPreview } from "./ArgvPreview";

/// The preview that runs off the right edge of its dialog (#1303), and
/// the boundary it must not blur while fixing that.
///
/// # What these tests can and cannot prove
///
/// jsdom applies no stylesheets and performs no layout: `getBoundingClientRect`
/// is zero everywhere, `scrollWidth` is zero everywhere, and no assertion
/// here can say "it fits on screen". So these assert the STRUCTURE that
/// makes overflow impossible -- the wrapping and overflow classes are on
/// the element that holds the text, and each argv word is still its own
/// element -- and the PR records what a real browser showed.
///
/// The classes are asserted on the `<code>` element the text is IN, found
/// by its text content. Not on an ancestor, and not by walking up from
/// one: an ancestor walk that stops one node early passes against a
/// deliberately broken layout, which is a test that cannot fail. Each
/// assertion below was run against the pre-fix class list and observed to
/// fail.

/// A brief of the shape that actually overflowed: a long absolute path,
/// no spaces to break at, and newlines that must survive.
const LONG_WORD =
  "cd '/Users/somebody/code/a-rather-long-repository-name/.worktrees/fix-1303-preview-overflow' && claude '## Finding\n\nThe file `src/components/SomeDeeplyNestedThing.tsx` does a thing.\n\nPlease fix it.'";

/// The `<code>` box whose OWN text is exactly this word.
///
/// Deliberately not `getByText`, and deliberately not a walk up from one:
/// `getByText` normalises whitespace, which would silently make a
/// multi-line brief match a single-line rendering of it -- and an ancestor
/// walk that stops one node early is how a structural test comes to pass
/// against a broken layout. This matches the boxes themselves, exactly.
function boxFor(word: string): HTMLElement {
  const boxes = Array.from(document.querySelectorAll("code")).filter(
    (c) => c.textContent === word,
  );
  expect(boxes.length).toBe(1);
  return boxes[0];
}

describe("the argv preview wraps instead of overflowing", () => {
  it("wraps a long word inside its own box, with no horizontal scroll", () => {
    render(<ArgvPreview program="bash" args={["-lc", LONG_WORD]} />);
    const box = boxFor(LONG_WORD);
    // Newlines in the brief are real newlines, not collapsed spaces.
    expect(box.className).toContain("whitespace-pre-wrap");
    // A path with no spaces still breaks rather than pushing the box wide.
    expect(box.className).toContain("break-words");
    // Vertical scrolling is fine and horizontal is the bug, so the box is
    // bounded in height and explicitly not scrollable sideways.
    expect(box.className).toMatch(/\bmax-h-\d+\b/);
    expect(box.className).toContain("overflow-auto");
    expect(box.className).toContain("overflow-x-hidden");
  });

  it("gives every word the same treatment, not just the long one", () => {
    // The bug was one component handling overflow and another not. A
    // preview where only *some* boxes wrap is the same defect one level
    // down, so every box is asserted rather than the interesting one.
    render(<ArgvPreview program="bash" args={["-lc", LONG_WORD, "--model", "opus"]} />);
    for (const word of ["bash", "-lc", LONG_WORD, "--model", "opus"]) {
      const box = boxFor(word);
      expect(box.className).toContain("whitespace-pre-wrap");
      expect(box.className).toContain("break-words");
    }
  });
});

describe("wrapping does not blur the argument boundaries", () => {
  /// The whole safety claim of this preview: "`;` inside a box stays
  /// inside it". A wrap that merged two words, or that let one box's text
  /// flow into the next, would make the preview lie about how many
  /// commands are being run.
  it("keeps each argv word as its own separate element", () => {
    render(<ArgvPreview program="/usr/bin/claude" args={["--resume", "x'; rm -rf ~", ""]} />);
    // Four words in, four boxes out. The dangerous one is a SINGLE box:
    // if it had been split at the `;` the preview would be showing two
    // commands where argv has one argument.
    const boxes = Array.from(document.querySelectorAll("code"));
    expect(boxes.length).toBe(4);
    expect(boxes[0].textContent).toBe("/usr/bin/claude");
    expect(boxes[1].textContent).toBe("--resume");
    expect(boxes[2].textContent).toBe("x'; rm -rf ~");
    // An empty argument is a real argv slot and still gets its own box,
    // labelled, rather than vanishing into the gap between its neighbours.
    expect(boxes[3].textContent).toBe("(empty)");
  });

  it("stacks the boxes in a column so a wrapped word cannot read as several", () => {
    // `block` on the box plus `flex-col` on the container is what makes a
    // word that wraps onto four lines unmistakably four lines of ONE box.
    // Laid out as inline chips in a wrapping row -- which is what
    // `LaunchTermsPicker` did before #1303 -- a wrapped word's second line
    // starts mid-row beside a different word's box, which is exactly the
    // ambiguity the boxes exist to remove.
    const { container } = render(<ArgvPreview program="bash" args={[LONG_WORD]} />);
    const box = boxFor(LONG_WORD);
    expect(box.className).toContain("block");
    const row = container.firstElementChild;
    expect(row?.className).toContain("flex-col");
    expect(row?.className).not.toContain("flex-wrap");
  });

  it("renders the container as the element the caller asked for", () => {
    // `ClaudifyAction` renders this inside an inline `<span>` chain, where
    // a `<div>` is invalid HTML that React will not repair.
    const { container } = render(<ArgvPreview as="span" program="bash" args={[]} />);
    expect(container.firstElementChild?.tagName).toBe("SPAN");
    // And the boxes inside it are `<code>`, which is inline-valid too.
    expect(container.querySelectorAll("div").length).toBe(0);
  });
});

describe("both preview sites share this component", () => {
  /// #1303's actual cause: `LaunchTermsPicker` and `ClaudifyAction` each
  /// had their own `ArgvWord`, and only one of them handled overflow. A
  /// third copy would reintroduce it silently, so the absence of a second
  /// definition is asserted rather than assumed.
  it("leaves no second ArgvWord definition behind to drift", async () => {
    for (const mod of [
      await import("./LaunchTermsPicker.tsx?raw"),
      await import("./ClaudifyAction.tsx?raw"),
    ]) {
      const src = mod.default as string;
      expect(src).toContain("ArgvPreview");
      // A local `function ArgvWord` is the exact shape that drifted.
      expect(src).not.toMatch(/function ArgvWord\b/);
      // And neither may hand-roll the box's classes again.
      expect(src).not.toContain("bg-[#0d1117] px-1 py-0.5 font-mono");
    }
  });
});
