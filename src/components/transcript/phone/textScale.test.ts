import { afterEach, describe, expect, it, vi } from "vitest";
import { readTextScale, scaleStyle } from "./textScale";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/// WebKit resolves `-apple-system-body` to the reader's text size; jsdom
/// does not, so the engine is stood in for: `CSS.supports` says yes and
/// the computed size is what that setting would give.
function asWebKit(bodyPx: number) {
  vi.stubGlobal("CSS", { supports: (p: string, v: string) => p === "font" && v === "-apple-system-body" });
  vi.spyOn(window, "getComputedStyle").mockReturnValue({ fontSize: `${bodyPx}px` } as CSSStyleDeclaration);
}

describe("readTextScale", () => {
  it("is 1 where Dynamic Type cannot be read", () => {
    expect(readTextScale()).toBe(1);
  });

  it("is the body size over the default 17 px", () => {
    asWebKit(17);
    expect(readTextScale()).toBe(1);
    asWebKit(23.8);
    expect(readTextScale()).toBeCloseTo(1.4);
  });

  it("is bounded at the accessibility sizes' ends", () => {
    asWebKit(200);
    expect(readTextScale()).toBe(3.25);
    asWebKit(4);
    expect(readTextScale()).toBe(0.75);
  });

  it("leaves no probe behind", () => {
    asWebKit(20);
    readTextScale();
    expect(document.body.children.length).toBe(0);
  });
});

describe("scaleStyle", () => {
  it("adds nothing at the default size", () => {
    expect(scaleStyle(1)).toBeUndefined();
    expect(scaleStyle(1.5)).toEqual({ zoom: 1.5 });
  });
});
