import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { stubViewport } from "@/test-utils";
import { useTranscriptRenderer } from "./renderer";

afterEach(() => {
  stubViewport(null);
  vi.unstubAllEnvs();
});

/// The renderer is a LAYOUT choice (`src/CLAUDE.md`): the phone build
/// always gets the phone renderer, and any build at a phone-narrow
/// viewport does too; a desktop-wide desktop build gets the desktop's.
describe("useTranscriptRenderer", () => {
  it("is the desktop's on a wide desktop build", () => {
    stubViewport(1280);
    expect(renderHook(() => useTranscriptRenderer()).result.current).toBe("desktop");
  });

  it("is the phone's on a narrow viewport of any build", () => {
    stubViewport(390);
    expect(renderHook(() => useTranscriptRenderer()).result.current).toBe("phone");
  });

  it("is the phone's on the mobile build, whatever the viewport says", () => {
    vi.stubEnv("VITE_TARGET", "mobile");
    stubViewport(1280);
    expect(renderHook(() => useTranscriptRenderer()).result.current).toBe("phone");
  });

  it("follows the viewport when it changes", () => {
    const vp = stubViewport(1280);
    const { result, rerender } = renderHook(() => useTranscriptRenderer());
    expect(result.current).toBe("desktop");
    vp.resize(600);
    rerender();
    expect(result.current).toBe("phone");
  });
});
