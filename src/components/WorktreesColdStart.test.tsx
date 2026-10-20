import { act, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Worktree, WorktreeRepo, WorktreeScan } from "@/types/pr";

/// #1950: the Worktrees scan was stored on every scan and never read, so
/// every cold start of Worktrees and Repositories was a blank "Scanning…"
/// with a full snapshot sitting in SQLite. These run the REAL hooks
/// against a mocked transport: the saved scan answers at once, discovery
/// is held open, and then lands.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));
vi.mock("./ViewSwitcher", () => ({ ViewSwitcher: () => null }));

const bridge = vi.hoisted(() => ({
  saved: null as unknown,
  scans: [] as ((v: unknown) => void)[],
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === "read_cached_scan") return Promise.resolve(bridge.saved);
    if (cmd === "get_worktree_dirs") return Promise.resolve(["/code"]);
    if (cmd === "get_discovery_exclusions") return Promise.resolve([]);
    if (cmd === "list_worktrees") return new Promise((resolve) => bridge.scans.push(resolve));
    // Everything else -- verdicts, update progress -- stays outstanding:
    // these tests are about which LIST is on screen, not its verdicts.
    return new Promise(() => {});
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { WorktreeSidebar } from "./WorktreeSidebar";
import { AllRepositoriesTable } from "./AllRepositoriesTable";

const wt = (path: string, is_main: boolean): Worktree =>
  ({
    path,
    branch: is_main ? "main" : "feature",
    head: "abc",
    size_bytes: null,
    safety: { kind: "pending" },
    is_main,
    merged_at: null,
    upstream: null,
    last_commit: null,
  }) as unknown as Worktree;

const repo = (name: string): WorktreeRepo => ({
  identity: null,
  name,
  path: `/code/${name}`,
  worktrees: [wt(`/code/${name}`, true), wt(`/code/${name}-wt`, false)],
});

const scan = (...names: string[]): WorktreeScan => ({
  repos: names.map(repo),
  unreadable: [],
  diagnostics: [],
});

function mount(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  bridge.saved = null;
  bridge.scans = [];
});

describe("cold start paints the saved worktrees scan (#1950)", () => {
  it("sidebar: saved rows with their age while discovery runs, replaced when it lands", async () => {
    bridge.saved = { payload: JSON.stringify(scan("saved-repo")), age_secs: 3 * 3600, stale: false };
    mount(<WorktreeSidebar />);

    const notice = await screen.findByText(/Showing the last scan, from 3 hours ago/);
    expect(notice).toBeTruthy();
    const savedList = screen.getByRole("list", { name: "Repositories from the last scan" });
    expect(within(savedList).getByText("saved-repo")).toBeTruthy();
    // A saved row is not a live one: nothing to select.
    expect(screen.queryByRole("button", { name: /saved-repo/ })).toBeNull();
    expect(screen.queryByText("Looking for repositories…")).toBeNull();

    await waitFor(() => expect(bridge.scans).toHaveLength(1));
    await act(async () => bridge.scans[0](scan("live-repo")));

    expect(await screen.findByRole("button", { name: /live-repo/ })).toBeTruthy();
    expect(screen.queryByText("saved-repo")).toBeNull();
    expect(screen.queryByText(/Showing the last scan/)).toBeNull();
  });

  it("sidebar: no saved scan keeps the plain scanning state", async () => {
    mount(<WorktreeSidebar />);
    expect(await screen.findByText("Looking for repositories…")).toBeTruthy();
    expect(screen.queryByText(/Showing the last scan/)).toBeNull();
  });

  it("All repositories: saved rows with their age, then the live table", async () => {
    bridge.saved = { payload: JSON.stringify(scan("saved-repo")), age_secs: 2 * 86400, stale: true };
    mount(<AllRepositoriesTable />);

    expect(await screen.findByText(/Showing the last scan, from 2 days ago/)).toBeTruthy();
    expect(screen.getByText(/may have changed/)).toBeTruthy();
    expect(screen.getByText("saved-repo")).toBeTruthy();
    expect(screen.queryByText("Scanning for repositories…")).toBeNull();
    // No live table yet, so no heading claiming to be it.
    expect(screen.queryByRole("heading", { name: "All Repositories" })).toBeNull();

    await waitFor(() => expect(bridge.scans).toHaveLength(1));
    await act(async () => bridge.scans[0](scan("live-repo")));

    expect(await screen.findByRole("heading", { name: "All Repositories" })).toBeTruthy();
    expect(screen.getByText("live-repo")).toBeTruthy();
    expect(screen.queryByText("saved-repo")).toBeNull();
    expect(screen.queryByText(/Showing the last scan/)).toBeNull();
  });
});
