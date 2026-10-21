import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Venv } from "@/types/pr";
import { useFilters } from "@/store/filters";

/// #1975: `scan_venvs` stored every result and nothing read it back, so
/// the virtualenv page started every launch from "Looking for Poetry
/// virtualenvs…" with a full snapshot sitting in SQLite. These run the
/// REAL hooks against a mocked transport (`WorktreesColdStart`'s shape):
/// the saved scan answers at once, the live scan is held open, and then
/// lands.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));
// Their own test files cover them; here they would only add commands.
vi.mock("./DiskUsagePanel", () => ({ DiskUsagePanel: () => null }));
vi.mock("./CleanupLog", () => ({ CleanupLog: () => null }));

const bridge = vi.hoisted(() => ({
  saved: null as unknown,
  scans: [] as ((v: unknown) => void)[],
  artifacts: [] as ((v: unknown) => void)[],
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { kind?: string }) => {
    if (cmd === "read_cached_scan") return Promise.resolve(args?.kind === "venvs" ? bridge.saved : null);
    if (cmd === "get_worktree_dirs") return Promise.resolve(["/code"]);
    if (cmd === "get_discovery_exclusions") return Promise.resolve([]);
    if (cmd === "scan_venvs") return new Promise((resolve) => bridge.scans.push(resolve));
    if (cmd === "scan_artifacts_report") return new Promise((resolve) => bridge.artifacts.push(resolve));
    // Sizes, prefs, progress -- outstanding: these tests are about which
    // LIST is on screen.
    return new Promise(() => {});
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { ArtifactsPage } from "./ArtifactsPage";
import { VENV_GROUP } from "./ArtifactSidebar";

const venv = (project: string, state: Venv["state"] = "orphaned"): Venv => ({
  path: `/venvs/${project}-py3.12`,
  project,
  state,
  source: state === "orphaned" ? null : `/code/${project}`,
  size_bytes: null,
  idle_secs: null,
});

function mount(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  useFilters.getState().setView("artifacts");
  useFilters.getState().setFilter("repo", VENV_GROUP);
});

afterEach(() => {
  bridge.saved = null;
  bridge.scans = [];
  bridge.artifacts = [];
  useFilters.getState().setFilter("repo", undefined);
});

describe("cold start paints the saved virtualenv scan (#1975)", () => {
  it("saved rows with their age while the scan runs, read-only, replaced when it lands", async () => {
    bridge.saved = { payload: JSON.stringify([venv("saved-project")]), age_secs: 2 * 3600, stale: false };
    mount(<ArtifactsPage />);

    expect(await screen.findByText(/Showing the last scan, from 2 hours ago/)).toBeTruthy();
    const saved = screen.getByRole("list", { name: "Virtualenvs from the last scan" });
    expect(saved.textContent).toContain("saved-project");
    // Read-only and verdict-free: a saved orphan verdict authorises nothing.
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /Remove/ })).toBeNull();
    expect(screen.queryByText(/orphaned/)).toBeNull();
    // Pending is not empty: no "none found" while the scan has not answered.
    expect(screen.queryByText(/No .* found/)).toBeNull();

    await act(async () => bridge.scans.forEach((resolve) => resolve([venv("live-project")])));

    await waitFor(() => expect(screen.queryByText(/Showing the last scan/)).toBeNull());
    expect(screen.queryByRole("list", { name: "Virtualenvs from the last scan" })).toBeNull();
    expect(screen.getByText("live-project")).toBeTruthy();
    expect(screen.queryByText("saved-project")).toBeNull();
  });

  it("no saved scan: the scanning sentence, not an empty page", async () => {
    mount(<ArtifactsPage />);
    expect(await screen.findByText(/Looking for Poetry virtualenvs/)).toBeTruthy();
    expect(screen.queryByText(/Showing the last scan/)).toBeNull();
  });
});
