import { vi } from "vitest";
const openUrl = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (u: string) => openUrl(u) }));
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";

describe("Markdown", () => {
  it("renders formatting", () => {
    const { container } = render(<Markdown>{"# Title\n\n- one\n- two"}</Markdown>);
    expect(screen.getByText("Title")).toBeTruthy();
    expect(container.querySelectorAll("li")).toHaveLength(2);
  });

  it("renders tables via GFM", () => {
    const { container } = render(
      <Markdown>{"| a | b |\n|---|---|\n| 1 | 2 |"}</Markdown>,
    );
    expect(container.querySelector("table")).toBeTruthy();
  });

  // Bodies and comments are written by other people, and this app holds
  // a token in memory. The sanitiser is the load-bearing part.
  it("strips script tags while keeping the surrounding prose", () => {
    // Separate blocks: raw HTML and its neighbouring text are one node
    // otherwise, so dropping the script drops the sentence with it.
    const { container } = render(
      <Markdown>{"safe text\n\n<script>window.evil=1</script>"}</Markdown>,
    );
    expect(container.querySelector("script")).toBeNull();
    expect(container.innerHTML).not.toContain("window.evil");
    expect(container.textContent).toContain("safe text");
  });

  it("strips inline event handlers", () => {
    const { container } = render(
      <Markdown>{'<img src="x" onerror="window.evil=1" alt="a">'}</Markdown>,
    );
    expect(container.innerHTML).not.toContain("onerror");
  });

  it("strips iframes", () => {
    const { container } = render(<Markdown>{'<iframe src="https://x"></iframe>'}</Markdown>);
    expect(container.querySelector("iframe")).toBeNull();
  });

  // A link must never navigate the app webview itself.
  // The intent is unchanged -- a link must open the user's browser, not
  // navigate the app -- but `target="_blank"` was never the mechanism
  // that achieved it. In a packaged Tauri window there is no browser
  // context to open a tab in, so the attribute did nothing; it only
  // appeared to work in `tauri dev`, where the webview IS a browser.
  it("opens links in the system browser, not the app", () => {
    const { container } = render(<Markdown>{"[click](https://example.com)"}</Markdown>);
    const a = container.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://example.com");
    fireEvent.click(a as Element);
    expect(openUrl).toHaveBeenCalledWith("https://example.com");
  });

  it("survives an empty body", () => {
    expect(() => render(<Markdown>{""}</Markdown>)).not.toThrow();
  });
});

/// #438: paragraphs and tables rendered squished together, and blank
/// lines looked ignored.
///
/// They were parsed correctly all along -- `prose-headstate` was applied
/// to the wrapper and never DEFINED anywhere, so every block element
/// fell back to the CSS reset, which strips margins.
describe("vertical rhythm", () => {
  /// Asserting on the CLASS rather than on computed style: jsdom
  /// computes no layout, so a margin assertion would pass either way.
  it("gives paragraphs a margin, so blank lines are visible", () => {
    const { container } = render(<Markdown>{"First para.\n\nSecond para."}</Markdown>);
    const ps = container.querySelectorAll("p");
    expect(ps).toHaveLength(2);
    for (const p of ps) expect(p.className).toMatch(/my-\d/);
  });

  it("spaces tables and collapses their borders", () => {
    const md = "| a | b |\n| - | - |\n| 1 | 2 |";
    const { container } = render(<Markdown>{md}</Markdown>);
    const table = container.querySelector("table");
    expect(table?.className).toMatch(/border-collapse/);
    expect(table?.parentElement?.className).toMatch(/my-\d/);
  });

  it("spaces lists and their items", () => {
    const { container } = render(<Markdown>{"- one\n- two"}</Markdown>);
    const ul = container.querySelector("ul");
    expect(ul?.className).toMatch(/my-\d/);
    expect(ul?.className).toMatch(/space-y-\d/);
  });

  /// A heading belongs to what FOLLOWS it, so it needs more space above
  /// than below -- equal margins make it float between two sections.
  it("gives headings more space above than below", () => {
    const { container } = render(<Markdown>{"## Heading\n\nBody."}</Markdown>);
    const h2 = container.querySelector("h2");
    expect(h2?.className).toMatch(/mt-5/);
    expect(h2?.className).toMatch(/mb-2/);
  });

  it("spaces block quotes and code blocks", () => {
    const { container } = render(
      <Markdown>{"> quoted\n\n```\ncode\n```"}</Markdown>,
    );
    expect(container.querySelector("blockquote")?.className).toMatch(/my-\d/);
    expect(container.querySelector("pre")?.className).toMatch(/my-\d/);
  });
});

/// #1279: every fenced code block rendered with its first line indented
/// one space.
///
/// The `code` override applied the inline chip's `px-1 py-0.5`
/// unconditionally. On an inline `<code>` that is right; inside a
/// `<pre>` the element is still inline, so the horizontal padding lands
/// at the start of the first line and after the last rather than around
/// the box -- exactly one space of first-line indent, with the
/// background and rounding doubled against the `<pre>`'s own.
describe("code", () => {
  /// The distinguishing assertion: the SAME class is right on one and
  /// wrong on the other, so each direction has to be checked.
  it("gives inline code the chip padding but a block none", () => {
    const { container } = render(
      <Markdown>{"Inline `bit` here.\n\n```\nblock line one\nblock line two\n```"}</Markdown>,
    );
    const [inline, block] = [...container.querySelectorAll("code")];
    expect(inline?.className).toMatch(/px-1/);
    expect(inline?.className).toMatch(/py-0\.5/);
    expect(block?.className ?? "").not.toMatch(/px-1/);
    expect(block?.className ?? "").not.toMatch(/py-0\.5/);
  });

  /// A fence with NO language tag is the case that rules out branching
  /// on `language-*`: react-markdown passes no className at all there,
  /// so it is indistinguishable from inline code by the props alone.
  it("strips the chip from an untagged fence too", () => {
    const { container } = render(<Markdown>{"```\nplain\n```"}</Markdown>);
    const block = container.querySelector("pre code");
    expect(block).toBeTruthy();
    expect(block?.className ?? "").not.toMatch(/px-1/);
  });

  /// The block's own chrome must not double up either: the `<pre>`
  /// already supplies the background, border and rounding.
  it("leaves the block's background and rounding to the pre", () => {
    const { container } = render(<Markdown>{"```\nplain\n```"}</Markdown>);
    const cls = container.querySelector("pre code")?.className ?? "";
    expect(cls).not.toMatch(/bg-/);
    expect(cls).not.toMatch(/rounded/);
    expect(container.querySelector("pre")?.className).toMatch(/bg-/);
  });

  /// Dropping the chip must not drop the language with it -- a
  /// highlighter downstream reads that class.
  it("keeps the fence's language class", () => {
    const { container } = render(<Markdown>{"```js\nconst x = 1;\n```"}</Markdown>);
    expect(container.querySelector("pre code")?.className).toContain("language-js");
  });

  /// A fenced block inside a blockquote goes through the same `pre`,
  /// so it must be stripped there too.
  it("strips the chip from a quoted block", () => {
    const { container } = render(<Markdown>{"> quoted\n>\n> ```\n> qcode\n> ```"}</Markdown>);
    const cls = container.querySelector("blockquote pre code")?.className ?? "";
    expect(cls).not.toMatch(/px-1/);
  });
});

/// Every component override spread react-markdown's props straight onto
/// an intrinsic element. Those props include `node`, the hast AST node.
/// React 19 does not recognise it, so it stringified it onto the DOM:
/// every element this component styles carried a literal
/// `node="[object Object]"`.
describe("AST leak", () => {
  it("does not emit the hast node as a DOM attribute", () => {
    const md = "# H\n\npara\n\n- one\n\n> quote\n\n---\n\n| a |\n| - |\n| 1 |\n\n```\nc\n```";
    const { container } = render(<Markdown>{md}</Markdown>);
    expect(container.querySelectorAll("[node]")).toHaveLength(0);
    expect(container.innerHTML).not.toContain("[object Object]");
  });
});

/// A task item is a checkbox, not a bullet. GFM renders the marker as
/// the `<input>` itself, so the list's `list-disc` gave it both.
describe("task lists", () => {
  it("suppresses the bullet on a task item", () => {
    const { container } = render(<Markdown>{"- [ ] todo\n- [x] done"}</Markdown>);
    const items = [...container.querySelectorAll("li")];
    expect(items).toHaveLength(2);
    expect(container.querySelectorAll("input[type=checkbox]")).toHaveLength(2);
    for (const li of items) expect(li.className).toMatch(/list-none/);
  });

  /// GFM marks the LIST `contains-task-list` when even ONE item is a
  /// task, so suppressing the marker list-wide would strip the bullet
  /// from the ordinary items beside it.
  it("keeps the bullet on a plain item in a mixed list", () => {
    const { container } = render(<Markdown>{"- plain item\n- [ ] a task"}</Markdown>);
    const [plain, task] = [...container.querySelectorAll("li")];
    expect(plain?.className ?? "").not.toMatch(/list-none/);
    expect(task?.className).toMatch(/list-none/);
  });
});

/// A nested list sits inside its parent's `li`, so the outer list's
/// `my-2` stacks on that item's own spacing and opens a gap in the
/// middle of what should read as one block.
describe("nested lists", () => {
  it("drops the outer margin on a nested list", () => {
    const { container } = render(<Markdown>{"- top\n  - nested"}</Markdown>);
    const [outer, inner] = [...container.querySelectorAll("ul")];
    expect(outer?.className).toMatch(/my-2/);
    expect(inner?.className ?? "").not.toMatch(/my-2/);
    expect(inner?.className).toMatch(/mt-1/);
  });

  it("still marks an ordered list with numbers", () => {
    const { container } = render(<Markdown>{"1. one\n2. two"}</Markdown>);
    expect(container.querySelector("ol")?.className).toMatch(/list-decimal/);
  });

  /// An `<ol>` that starts at `3.` carries a `start` attribute. The
  /// list override computes its own className, so it must pass the
  /// rest through rather than swallowing it and renumbering from 1.
  it("preserves an ordered list's starting number", () => {
    const { container } = render(<Markdown>{"3. three\n4. four"}</Markdown>);
    expect(container.querySelector("ol")?.getAttribute("start")).toBe("3");
  });
});

/// A comment body starts at whatever heading level its author felt
/// like. Without an override, h4-h6 fall through to the CSS reset and
/// render at body size and body weight.
describe("deep headings", () => {
  it("styles h4, h5 and h6 distinguishably from body text", () => {
    const { container } = render(<Markdown>{"#### Four\n\n##### Five\n\n###### Six"}</Markdown>);
    for (const tag of ["h4", "h5", "h6"]) {
      const el = container.querySelector(tag);
      expect(el, tag).toBeTruthy();
      expect(el?.className, tag).toMatch(/font-semibold/);
      expect(el?.className, tag).toMatch(/mt-\d/);
    }
  });
});

/// #1456: CI comments nest large reports in `<details>`, and without
/// raw-HTML parsing the sanitiser dropped the wrappers, so every level
/// rendered as one always-open block.
describe("details", () => {
  const NESTED = [
    "<details>",
    "<summary>Level one</summary>",
    "",
    "Body one.",
    "",
    "<details>",
    "<summary>Level two</summary>",
    "",
    "Body two.",
    "",
    "<details open>",
    "<summary>Level three</summary>",
    "",
    "Body three.",
    "",
    "</details>",
    "</details>",
    "</details>",
  ].join("\n");

  function levels(container: HTMLElement): HTMLDetailsElement[] {
    return [...container.querySelectorAll("details")];
  }

  it("renders three nested levels as real details elements", () => {
    const { container } = render(<Markdown>{NESTED}</Markdown>);
    const all = levels(container);
    expect(all).toHaveLength(3);
    // Each nested inside the previous, not flattened into siblings.
    expect(all[1]?.parentElement?.closest("details")).toBe(all[0]);
    expect(all[2]?.parentElement?.closest("details")).toBe(all[1]);
    const summaries = [...container.querySelectorAll("summary")].map((s) => s.textContent);
    expect(summaries).toEqual(["Level one", "Level two", "Level three"]);
  });

  /// Level three is written `<details open>`: an author's `open` must
  /// not unfold anything on expand.
  it("starts collapsed at every depth, whatever the author wrote", () => {
    const { container } = render(<Markdown>{NESTED}</Markdown>);
    for (const d of levels(container)) {
      expect(d.open).toBe(false);
      expect(d.hasAttribute("open")).toBe(false);
    }
  });

  it("opens each level on its own", () => {
    const { container } = render(<Markdown>{NESTED}</Markdown>);
    const [one, two, three] = levels(container) as [
      HTMLDetailsElement,
      HTMLDetailsElement,
      HTMLDetailsElement,
    ];
    const state = () => [one.open, two.open, three.open];
    one.open = true;
    expect(state()).toEqual([true, false, false]);
    two.open = true;
    expect(state()).toEqual([true, true, false]);
    three.open = true;
    one.open = false;
    expect(state()).toEqual([false, true, true]);
  });

  it("keeps the summary when it shares a block with its details", () => {
    const { container } = render(
      <Markdown>{"<details><summary>Report</summary>\n\nContent here.\n\n</details>"}</Markdown>,
    );
    expect(container.querySelector("summary")?.textContent).toBe("Report");
    expect(container.querySelector("details")?.textContent).toContain("Content here.");
  });
});

/// Parsing raw HTML is what #1456 needed and exactly what a sanitiser
/// exists for. Each of these would reach the webview if the sanitiser
/// ran before `rehype-raw` rather than after it.
describe("sanitising raw HTML", () => {
  it("strips a script nested inside details", () => {
    const { container } = render(
      <Markdown>{"<details><summary>s</summary><script>window.evil=1</script></details>"}</Markdown>,
    );
    expect(container.querySelector("script")).toBeNull();
    expect(container.innerHTML).not.toContain("window.evil");
  });

  it("strips event handlers from allowed elements", () => {
    const md =
      '<details ontoggle="window.evil=1"><summary onclick="window.evil=2">s</summary></details>' +
      '\n\n<img src="https://example.com/a.png" onerror="window.evil=3">';
    const { container } = render(<Markdown>{md}</Markdown>);
    expect(container.querySelector("details")).toBeTruthy();
    expect(container.innerHTML).not.toMatch(/on(toggle|click|error)/);
    expect(container.innerHTML).not.toContain("window.evil");
  });

  it("refuses javascript: URLs in raw anchors", () => {
    const { container } = render(<Markdown>{'<a href="javascript:window.evil=1">x</a>'}</Markdown>);
    expect(container.innerHTML).not.toContain("javascript:");
  });

  it("strips style elements, contents included, and style attributes", () => {
    const { container } = render(
      <Markdown>{'<style>body{display:none}</style>\n\n<p style="position:fixed">styled</p>'}</Markdown>,
    );
    expect(container.querySelector("style")).toBeNull();
    expect(container.textContent).not.toContain("display:none");
    expect(container.innerHTML).not.toContain("position:fixed");
    expect(container.textContent).toContain("styled");
  });

  it("strips a raw iframe", () => {
    const { container } = render(
      <Markdown>
        {'<details><summary>s</summary><iframe src="https://example.com"></iframe></details>'}
      </Markdown>,
    );
    expect(container.querySelector("iframe")).toBeNull();
  });

  it("does not render HTML comments", () => {
    const { container } = render(<Markdown>{"<!-- bot-marker -->\n\nVisible."}</Markdown>);
    expect(container.innerHTML).not.toContain("bot-marker");
    expect(container.textContent).toContain("Visible.");
  });
});
