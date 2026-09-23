import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeMdAdviceResult } from "@/types/pr";

/// The advice table on the phone build (#1344).
///
/// `IS_MOBILE_BUILD` is a build-time constant, so the mobile build is
/// reached by mocking `@/lib/target`, as `ClaudeCodePage.mobile.test.tsx`
/// does. A terminal IS configured here, to prove the column's absence is
/// the capability deciding and not the missing setting.
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));
vi.mock("../api/hooks", () => ({
  useUiPrefs: () => ({ prefs: { terminal_command: "open -a Terminal {command}" } }),
  useClaudeMdAdvice: (_repo: string, _enabled: boolean, mode = "cached") =>
    mode === "fresh"
      ? { data: undefined, isError: false, error: undefined, isFetching: false, refetch: vi.fn() }
      : { data: result, isError: false, error: undefined, isFetching: false, refetch: vi.fn() },
}));
vi.mock("../api/tauri", () => ({
  claudeMdAdviceLaunch: vi.fn(),
  claudeMdAdviceLaunchPreview: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { ClaudeMdAdvicePanel } from "./ClaudeMdAdvicePanel";
import { useFilters } from "@/store/filters";

const REPO = "/home/octocat/hello-world";

const result: ClaudeMdAdviceResult = {
  report: {
    repo: REPO,
    findings: [
      {
        check: "imports",
        severity: "problem",
        subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
        evidence: [],
        finding: "a finding",
        brief: "## a finding\n",
      },
    ],
    checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    brief: "# advice\n",
  },
  freshness: { state: "fresh", recomputed: true },
  computedAt: "2026-01-01T00:00:00Z",
  build: "7.4.0",
};

beforeEach(() => {
  useFilters.setState({ filtersByView: {} as never, view: "claude-md" });
});

describe("ClaudeMdAdvicePanel on the phone build", () => {
  it("drops the Claudify column and keeps Copy brief", () => {
    render(<ClaudeMdAdvicePanel repo={REPO} activePath={undefined} onSelectFile={vi.fn()} />);
    const table = screen.getByRole("table");
    const headers = within(table)
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(headers).toEqual(["Severity", "Finding", "Where", "Copy brief"]);
    expect(within(table).getByRole("button", { name: "Copy brief" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Claudify" })).toBeNull();
    // Nor a sentence about a terminal the phone could never use.
    expect(screen.queryByText(/no terminal is configured/i)).toBeNull();
  });
});
