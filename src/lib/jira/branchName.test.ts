import { describe, expect, it } from "vitest";
import { branchName, slugify, templateHasKey } from "./branchName";

describe("branchName", () => {
  it("proposes the key plus a slug of the summary by default", () => {
    expect(branchName("", "ABC-1", "Fix login redirect")).toBe("abc-1-fix-login-redirect");
  });

  it("fills a template", () => {
    expect(branchName("feature/{key}-{summary}", "ABC-1", "Fix login")).toBe("feature/abc-1-fix-login");
    expect(branchName("{KEY}_{summary}", "XY-2", "Retry")).toBe("XY-2_retry");
  });

  it("trims what an empty summary leaves dangling", () => {
    expect(branchName("", "ABC-1", "!!!")).toBe("abc-1");
    expect(branchName("feature/{summary}-{key}", "ABC-1", "")).toBe("feature/abc-1");
  });

  it("slugs to ASCII words cut at a boundary", () => {
    expect(slugify("Café: “quoted” — dash/slash")).toBe("cafe-quoted-dash-slash");
    expect(slugify("one two three", 9)).toBe("one-two");
    expect(slugify("abcdefghijkl", 5)).toBe("abcde");
  });

  it("knows whether a template can link to its ticket", () => {
    expect(templateHasKey("")).toBe(true);
    expect(templateHasKey("feature/{KEY}")).toBe(true);
    expect(templateHasKey("feature/{summary}")).toBe(false);
  });
});
