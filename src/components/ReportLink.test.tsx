import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiagnosticBundle } from "../types/report";

const lookups = vi.hoisted(() => ({
  lookupVersion: vi.fn<() => Promise<string | null>>(),
  lookupBundle: vi.fn<() => Promise<DiagnosticBundle | null>>(),
}));
vi.mock("../lib/reportError", () => lookups);

import { ReportLink } from "./ReportLink";
import { ErrorBoundary } from "./ErrorBoundary";

const bundle: DiagnosticBundle = {
  appVersion: "7.9.2",
  os: "linux",
  arch: "x86_64",
  osVersion: "Linux 6.1",
  install: { method: "Linux AppImage", basis: "the AppImage runtime is set" },
  ghVersion: "gh version 2.80.0",
  ghNote: null,
  poll: {
    operation: "the open pull request search (GraphQL)",
    fetchTimeoutSecs: 30,
    tickTimeoutSecs: 45,
    focusedIntervalSecs: 60,
    lastWaitSecs: 60,
    ticksRecorded: 3,
    recent: [],
    failuresInRecent: 0,
    lastSuccessSecsAgo: null,
  },
  graphqlRemaining: 100,
  restRemaining: 200,
  diagnosticsOn: false,
  logTail: "[diag] poll tick start",
  logNote: null,
};

beforeEach(() => {
  lookups.lookupVersion.mockReset().mockResolvedValue("7.9.2");
  lookups.lookupBundle.mockReset().mockResolvedValue(bundle);
});

const openDialog = async () => {
  fireEvent.click(screen.getByRole("button", { name: "Report this" }));
  return screen.findByRole("dialog");
};

const githubHref = () =>
  new URL(screen.getByRole("link", { name: "Open on GitHub" }).getAttribute("href") ?? "");

describe("ReportLink", () => {
  it("renders at once, before any lookup, and asks nothing until opened", () => {
    lookups.lookupBundle.mockReturnValue(new Promise(() => {}));
    render(<ReportLink error="boom" />);
    expect(screen.getByRole("button", { name: "Report this" })).toBeTruthy();
    expect(lookups.lookupBundle).not.toHaveBeenCalled();
  });

  it("previews the report and opens the prefilled form, never submitting", async () => {
    render(<ReportLink error="GitHub request timed out after 30s" view="My PRs" diagnostics />);
    const dialog = await openDialog();
    await waitFor(() => expect(githubHref().searchParams.get("os")).toBe("Linux 6.1 x86_64"));
    const p = githubHref().searchParams;
    expect(githubHref().pathname).toBe("/StormKiln/headstate/issues/new");
    expect(p.get("template")).toBe("bug_report.yml");
    expect(p.get("install")).toBe("Linux AppImage");
    expect(p.get("what-happened")).toContain("View: My PRs");
    // The caller's answer wins over the desktop's.
    expect(p.get("what-happened")).toContain("Diagnostic logging: on");
    // Every section is shown for editing.
    expect(within(dialog).getByRole("textbox", { name: "Diagnostic log" })).toBeTruthy();
    // No submit control of any kind.
    expect(within(dialog).queryByRole("button", { name: /submit/i })).toBeNull();
  });

  it("lets the user leave a section out, and the URL loses it", async () => {
    render(<ReportLink error="boom" />);
    const dialog = await openDialog();
    await waitFor(() => expect(githubHref().searchParams.get("log")).toContain("poll tick start"));
    const log = within(dialog).getByRole("group", { name: "Diagnostic log" });
    fireEvent.click(within(log).getByRole("checkbox", { name: "Leave this out" }));
    expect(githubHref().searchParams.get("log")).toBeNull();
    expect(within(log).getByRole("textbox")).toHaveProperty("disabled", true);
  });

  it("scrubs what the user types before it reaches the URL, and says so", async () => {
    render(<ReportLink error="boom" />);
    const dialog = await openDialog();
    const steps = within(dialog).getByRole("textbox", { name: "How to get there" });
    fireEvent.change(steps, { target: { value: "opened acme/widget from /Users/alice" } });
    const sent = githubHref().searchParams.get("steps") ?? "";
    expect(sent).not.toContain("acme/widget");
    expect(sent).not.toContain("alice");
    expect(within(dialog).getByText(/Will also be removed when sent: 1 file path and 1 repository name/)).toBeTruthy();
  });

  it("notes what automatic scrubbing removed, as counts", async () => {
    render(<ReportLink error="failed for acme/widget" />);
    const dialog = await openDialog();
    expect(within(dialog).getByText(/Removed automatically: 1 repository name\./)).toBeTruthy();
    expect(within(dialog).queryByText(/acme\/widget/)).toBeNull();
  });

  it("states unknowns when the desktop cannot answer", async () => {
    lookups.lookupVersion.mockResolvedValue(null);
    lookups.lookupBundle.mockResolvedValue(null);
    render(<ReportLink error="boom" />);
    const dialog = await openDialog();
    await waitFor(() => expect(within(dialog).getByText(/Some details could not be read/)).toBeTruthy());
    expect(githubHref().searchParams.get("version")).toContain("unknown");
    expect(githubHref().searchParams.get("gh-version")).toContain("unknown");
  });
});

describe("inside the crash panel", () => {
  /// `ErrorBoundary` sits ABOVE `QueryClientProvider`, so there is no
  /// QueryClient here. A query hook anywhere in the report would throw
  /// while rendering the crash screen.
  it("opens the report with no QueryClient anywhere", async () => {
    const Boom = () => {
      throw new Error("render failed");
    };
    vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    const dialog = await openDialog();
    expect(within(dialog).getByText("Report this problem")).toBeTruthy();
    await waitFor(() =>
      expect(githubHref().searchParams.get("what-happened")).toContain("render failed"),
    );
    // Keyboard: Escape closes it and the crash panel is still there.
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByText("Something went wrong")).toBeTruthy();
  });
});
