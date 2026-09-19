import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const reportUrlFn = vi.hoisted(() => vi.fn());
vi.mock("../lib/reportError", async (orig) => {
  const actual = await orig<typeof import("../lib/reportError")>();
  return { ...actual, reportUrl: reportUrlFn };
});

import { ReportLink } from "./ReportLink";

const href = () =>
  decodeURIComponent(screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "");

beforeEach(() => {
  reportUrlFn.mockReset();
});

describe("ReportLink", () => {
  it("renders a usable link immediately, before the environment answers", () => {
    // It used to render NOTHING until the lookups resolved, so an IPC
    // call that never answered left the link permanently absent. That
    // is what "Report this does nothing" actually was.
    reportUrlFn.mockReturnValue(new Promise(() => {}));
    render(<ReportLink error="boom" />);
    expect(href()).toContain("boom");
  });

  it("carries the caller's context in that immediate link", () => {
    // The environment needs a lookup; the component stack does not --
    // it is already in hand. A link that waited for one to carry the
    // other would drop the stack whenever the lookups timed out.
    reportUrlFn.mockReturnValue(new Promise(() => {}));
    render(<ReportLink error="boom" componentStack="at DockerPage" view="Docker images" />);
    expect(href()).toContain("at DockerPage");
    expect(href()).toContain("On the Docker images view");
  });

  it("upgrades to the richer URL once it resolves", async () => {
    reportUrlFn.mockResolvedValue("https://example.invalid/rich");
    render(<ReportLink error="boom" />);
    await waitFor(() => expect(href()).toBe("https://example.invalid/rich"));
  });

  it("keeps a usable link when the lookups reject", async () => {
    // A missing platform line is worth far less than a report that
    // never opens.
    reportUrlFn.mockRejectedValue(new Error("ipc died"));
    render(<ReportLink error="boom" />);
    await waitFor(() => expect(href()).toContain("boom"));
  });

  it("never adopts a resolution for a superseded error", async () => {
    // A slow lookup for the FIRST error must not write over the
    // SECOND's link -- that would file a report about the wrong
    // failure. The effect's cleanup flips `live`, which is what stops
    // it.
    //
    // The `await` is load-bearing: a promise settled in this tick runs
    // its handler in a microtask, so an assertion made synchronously
    // after `settleFirst` passes whether or not the guard is there.
    // Without the wait this test passed against a deliberately
    // unguarded component.
    let settleFirst: (v: string) => void = () => {};
    reportUrlFn.mockImplementationOnce(() => new Promise<string>((res) => (settleFirst = res)));
    reportUrlFn.mockImplementationOnce(() => new Promise(() => {}));

    const { rerender } = render(<ReportLink error="FIRST" />);
    rerender(<ReportLink error="SECOND" />);
    settleFirst("https://example.invalid/for-the-first-error");
    await new Promise((r) => setTimeout(r, 0));

    expect(href()).toContain("SECOND");
    expect(href()).not.toContain("for-the-first-error");
  });
});
