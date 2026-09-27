import { vi } from "vitest";
const build = vi.hoisted(() => ({ mobile: true }));
vi.mock("../lib/target", () => ({
  get IS_MOBILE_BUILD() {
    return build.mobile;
  },
  get IS_DESKTOP_BUILD() {
    return !build.mobile;
  },
}));
import { render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { TranscriptMarkdown } from "./TranscriptMarkdown";

/// A module worker that cannot load its script: the constructor
/// succeeds and the failure arrives as an `error` event, which is how a
/// worker whose URL the webview's scheme cannot serve fails.
class BrokenWorker extends EventTarget {
  postMessage() {
    queueMicrotask(() => this.dispatchEvent(new Event("error")));
  }
  terminate() {}
}

beforeEach(() => {
  vi.stubGlobal("Worker", BrokenWorker);
  // Mounted blocks are highlighted at once without an observer.
  vi.stubGlobal("IntersectionObserver", undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  build.mobile = true;
});

const fence = "```ts\nconst a: number = 1;\n```";

/// #1481: highlighting on the phone runs in a module worker loaded from
/// the app's custom scheme, not yet verified on a device. If it fails
/// there every block is plain, and the phone says so -- the only way
/// the failure gets noticed.
describe("TranscriptMarkdown on the phone build", () => {
  it("says when highlighting did not load, and keeps the code whole", async () => {
    const { container } = render(<TranscriptMarkdown>{fence}</TranscriptMarkdown>);
    await waitFor(() =>
      expect(container.textContent).toContain("highlighting did not load on this device"),
    );
    expect(container.querySelector("pre")?.textContent).toBe("const a: number = 1;");
  });

  it("says nothing of it on the desktop build, where the worker is verified", async () => {
    build.mobile = false;
    const { container } = render(<TranscriptMarkdown>{fence}</TranscriptMarkdown>);
    await new Promise((r) => setTimeout(r, 20));
    expect(container.textContent).not.toContain("did not load");
  });
});
