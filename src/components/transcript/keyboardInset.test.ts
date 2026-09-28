/// #1491: how much of the layout viewport the on-screen keyboard covers.

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { keyboardInset, useKeyboardInset } from "./keyboardInset";

/// A stand-in for iOS's `visualViewport`: an event target with a height
/// and an offset the test moves.
class FakeVisualViewport extends EventTarget {
  height = 800;
  offsetTop = 0;
  move(height: number, offsetTop = 0) {
    this.height = height;
    this.offsetTop = offsetTop;
    this.dispatchEvent(new Event("resize"));
  }
}

const saved = Object.getOwnPropertyDescriptor(window, "visualViewport");
const savedInner = window.innerHeight;
function installViewport(): FakeVisualViewport {
  const vv = new FakeVisualViewport();
  Object.defineProperty(window, "visualViewport", { configurable: true, value: vv });
  Object.defineProperty(window, "innerHeight", { configurable: true, value: 800 });
  return vv;
}
afterEach(() => {
  if (saved) Object.defineProperty(window, "visualViewport", saved);
  else delete (window as { visualViewport?: unknown }).visualViewport;
  Object.defineProperty(window, "innerHeight", { configurable: true, value: savedInner });
});

describe("keyboardInset", () => {
  it("is the part of the layout viewport below the visual one", () => {
    expect(keyboardInset({ height: 800, offsetTop: 0 }, 800)).toBe(0);
    expect(keyboardInset({ height: 500, offsetTop: 0 }, 800)).toBe(300);
    // iOS scrolled the visual viewport down by 100: the keyboard covers
    // 200 of the layout viewport, not 300.
    expect(keyboardInset({ height: 500, offsetTop: 100 }, 800)).toBe(200);
  });

  it("is never negative", () => {
    expect(keyboardInset({ height: 900, offsetTop: 0 }, 800)).toBe(0);
  });
});

describe("useKeyboardInset", () => {
  it("follows the keyboard opening and closing", () => {
    const vv = installViewport();
    const { result } = renderHook(() => useKeyboardInset(true));
    expect(result.current).toBe(0);
    act(() => vv.move(460));
    expect(result.current).toBe(340);
    act(() => vv.move(800));
    expect(result.current).toBe(0);
  });

  it("is 0 when not enabled, whatever the keyboard does", () => {
    const vv = installViewport();
    const { result } = renderHook(() => useKeyboardInset(false));
    act(() => vv.move(460));
    expect(result.current).toBe(0);
  });

  it("is 0 where there is no visual viewport", () => {
    Object.defineProperty(window, "visualViewport", { configurable: true, value: null });
    const { result } = renderHook(() => useKeyboardInset(true));
    expect(result.current).toBe(0);
  });
});
