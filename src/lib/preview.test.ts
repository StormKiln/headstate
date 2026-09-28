import { describe, expect, it } from "vitest";
import { commentPreview } from "./preview";

describe("commentPreview", () => {
  it("uses the body when it is a single plain line", () => {
    expect(commentPreview("LGTM once CI passes")).toBe("LGTM once CI passes");
  });

  // The three shapes `body.slice(0, n)` gets wrong. Each one is a real
  // comment opening, and each previews as punctuation without this.
  it("skips a leading blank line", () => {
    expect(commentPreview("\n\nThis will break on Windows")).toBe(
      "This will break on Windows",
    );
  });

  it("takes the text of a heading rather than the hashes", () => {
    expect(commentPreview("## Summary\n\nPin the version")).toBe("Summary");
  });

  it("skips a fence and previews the code's first line", () => {
    expect(commentPreview("```ts\nconst x = 1;\n```")).toBe("const x = 1;");
  });

  it("reads bullets and task boxes as their text", () => {
    expect(commentPreview("- [ ] rebase onto main")).toBe("rebase onto main");
    expect(commentPreview("1. first step")).toBe("first step");
  });

  // The preview is placed as PLAIN text in a one-line row, so markup that
  // survives here would render literally rather than as formatting.
  it("renders inline markup as the text a person reads", () => {
    expect(commentPreview("**Please** pin `tauri` to [2.x](http://a.b)")).toBe(
      "Please pin tauri to 2.x",
    );
  });

  it("keeps an image's alt text without leaving the bang", () => {
    expect(commentPreview("![a screenshot](http://a.b/c.png)")).toBe("a screenshot");
  });

  it("collapses whitespace runs", () => {
    expect(commentPreview("a     b\tc")).toBe("a b c");
  });

  it("truncates long lines and marks that it did", () => {
    const out = commentPreview("x".repeat(200), 80);
    expect(out).toHaveLength(81);
    expect(out.endsWith("…")).toBe(true);
  });

  // An unconditional ellipsis makes a complete short comment look like it
  // continues, which is the opposite of what the row is for.
  it("does not mark a short comment as truncated", () => {
    expect(commentPreview("short").endsWith("…")).toBe(false);
  });

  // No characters at all: nothing to label, so no preview.
  it("returns empty for a blank body", () => {
    expect(commentPreview("\n\n   \n")).toBe("");
  });

  // Characters, but none a reader would see: a neutral label rather than a
  // title row that looks like the preview failed to load (#1458).
  it("labels a body with content but no visible text", () => {
    expect(commentPreview("```\n```")).toBe("(no text)");
  });
});

/// #1458: bots open their bodies with an HTML comment marker, and the
/// collapsed title row read `<!-- ai-review -->`.
describe("commentPreview on HTML", () => {
  it("skips a leading marker comment", () => {
    expect(commentPreview("<!-- bot-marker -->\nThree checks failed")).toBe(
      "Three checks failed",
    );
  });

  it("skips a multi-line comment", () => {
    const body = "<!--\nmarker: bot-review\nversion: 2\n-->\n\nLooks good overall";
    expect(commentPreview(body)).toBe("Looks good overall");
  });

  it("takes the summary text from a details opener", () => {
    expect(commentPreview("<details>\n<summary>CI report</summary>\n\nbody")).toBe("CI report");
    expect(commentPreview("<details><summary><b>CI</b>: 3 failed</summary>")).toBe(
      "CI: 3 failed",
    );
  });

  it("labels a body that is only a marker", () => {
    expect(commentPreview("<!-- bot-marker -->")).toBe("(no text)");
    expect(commentPreview("<!-- bot-marker -->\n\n<details>\n</details>")).toBe("(no text)");
  });

  it("hides everything after an unterminated comment, as rendering does", () => {
    expect(commentPreview("<!-- never closed\nhidden text")).toBe("(no text)");
  });

  // Prose about code is not markup: only real HTML element names go.
  it("keeps angle brackets that are not HTML tags", () => {
    expect(commentPreview("Return Option<T> instead of Vec<String>")).toBe(
      "Return Option<T> instead of Vec<String>",
    );
  });

  it("drops an inline comment from the chosen line", () => {
    expect(commentPreview("Ready <!-- ignore --> to merge")).toBe("Ready to merge");
  });
});
