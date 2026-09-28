/// #1491: what a send does to the scroll position.

import { describe, expect, it, vi } from "vitest";
import { sendScroll, type SendScrollDeps } from "./sendScroll";

function deps(over: Partial<SendScrollDeps> = {}): SendScrollDeps {
  return {
    atTail: true,
    reducedMotion: false,
    scrollToEnd: vi.fn(),
    rewindToTail: vi.fn(),
    anchor: vi.fn(),
    ...over,
  };
}

describe("sendScroll", () => {
  it("anchors the new message and re-engages follow at the live edge", () => {
    const d = deps();
    sendScroll("c1", d);
    expect(d.anchor).toHaveBeenCalledWith("c1");
    expect(d.scrollToEnd).toHaveBeenCalledWith({ behavior: "smooth" });
    expect(d.rewindToTail).not.toHaveBeenCalled();
  });

  it("scrolls instantly when the reader asked for less motion", () => {
    const d = deps({ reducedMotion: true });
    sendScroll("c1", d);
    expect(d.scrollToEnd).toHaveBeenCalledWith({ behavior: "auto" });
  });

  it("re-windows to the tail first when the reader had scrolled back past the window", () => {
    const d = deps({ atTail: false });
    sendScroll("c1", d);
    expect(d.anchor).toHaveBeenCalledWith("c1");
    expect(d.rewindToTail).toHaveBeenCalledOnce();
    // The rows to scroll to are not mounted yet; the viewer scrolls once
    // they are.
    expect(d.scrollToEnd).not.toHaveBeenCalled();
  });
});
