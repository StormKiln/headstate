import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DiskObservation } from "../types/disk";
const api = vi.hoisted(() => ({ diskStatus: vi.fn(), diskHistory: vi.fn(), diskSettings: vi.fn(), saveDiskSettings: vi.fn(), startDiskScan: vi.fn(), cancelDiskScan: vi.fn() }));
vi.mock("../api/diskInventory", () => api);
vi.mock("../api/tauri", () => ({ claudeRevealPath: vi.fn() }));
import { DiskUsagePanel } from "./DiskUsagePanel";
const gib = 1073741824n;
const bytes = (n: number) => (BigInt(n) * gib).toString();
const sample = (): DiskObservation => ({ id: "one", volume: { id: "volume", identity_stable: true, mount: "/fixture", label: "Fixture volume", scope: "filesystem", capacity: bytes(200), used: bytes(100), available: bytes(100), method: "fixture", limitation: null }, method: "fixture", configuration: "one", started_at: 100, finished_at: 110, status: "complete", visited: 3, locations: [{ id: "target", identity_stable: true, path: "/fixture/output", aliases: [], owners: ["/fixture/project"], category: "Cargo output", evidence: ["Cargo signatures; manifest unavailable"], logical: bytes(80), allocated: bytes(70), reclaimable: null, complete: true, active: false, review_only: true }], categories: [{ name: "Cargo output", allocated: bytes(70), logical: bytes(80), locations: ["target"] }], coverage: [{ path: "/fixture/private", reason: "Permission denied; size unknown" }], measured: bytes(70), remainder: bytes(30), accounting_note: "The remainder is an accounting difference, not space known to be disposable.", approximate: true, comparison: { baseline_at: null, reason: "No baseline", changes: [] }, history_error: null });
beforeEach(() => { vi.clearAllMocks(); api.diskStatus.mockResolvedValue({ run_id: null, running: false, visited: 0, current_path: null, observations: [sample()], error: null }); api.diskHistory.mockResolvedValue([]); api.diskSettings.mockResolvedValue({ external_roots: [], additional_locations: false }); });
afterEach(cleanup);
async function show() { await act(async () => { render(<DiskUsagePanel />); }); fireEvent.click(screen.getByText("Disk usage")); await act(async () => { }); }
describe("disk reconciliation", () => {
    it("renders scoped totals, remainder, coverage and review-only contributors", async () => { await show(); expect(screen.getByText(/100\.0 GiB used/)).toBeTruthy(); expect(screen.getByText("30.0 GiB unexplained")).toBeTruthy(); expect(screen.getByText(/70.0% of reported used space/)).toBeTruthy(); expect(screen.getByText(/Permission denied/)).toBeTruthy(); expect(screen.getByText("No baseline")).toBeTruthy(); fireEvent.click(screen.getByText("Cargo output")); expect(screen.getByText("/fixture/output")).toBeTruthy(); expect(screen.queryByRole("button", { name: /remove|delete/i })).toBeNull(); });
    it("exposes a negative difference instead of clamping to zero", async () => { const o = sample(); o.remainder = bytes(-5); o.accounting_note = "Measured allocation exceeds reported usage."; api.diskStatus.mockResolvedValue({ running: false, observations: [o] }); await show(); expect(screen.getByText(/5.0 GiB over reported usage/)).toBeTruthy(); });
    it("does not invent growth or zero for unavailable measurements", async () => { const o = sample(); o.remainder = null; o.measured = null; o.volume.used = null; o.comparison.reason = "Scan locations or exclusions changed"; api.diskStatus.mockResolvedValue({ running: false, observations: [o] }); await show(); expect(screen.getByText(/Used space unavailable/)).toBeTruthy(); expect(screen.getByText("Unexplained amount unavailable")).toBeTruthy(); expect(screen.getByText("Scan locations or exclusions changed")).toBeTruthy(); });
    it("shows comparable change with its observation interval", async () => { const o = sample(); o.comparison = { baseline_at: 10, reason: null, changes: [{ location_id: "target", path: "/fixture/output", bytes: bytes(4) }] }; api.diskStatus.mockResolvedValue({ running: false, observations: [o] }); await show(); expect(screen.getByText(/\+4.0 GiB/)).toBeTruthy(); expect(screen.getByText(/Change since/)).toBeTruthy(); });
});
describe("disk measurement lifecycle", () => {
    it("does not fetch or poll while the panel is closed", async () => {
        vi.useFakeTimers();
        try {
            render(<DiskUsagePanel />);
            await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
            expect(api.diskStatus).not.toHaveBeenCalled();
            expect(api.diskHistory).not.toHaveBeenCalled();
            api.diskStatus.mockResolvedValue({ run_id: "run", running: true, visited: 4, current_path: null, observations: [sample()], error: null });
            await act(async () => { fireEvent.click(screen.getByText("Disk usage")); });
            expect(api.diskStatus).toHaveBeenCalledTimes(1);
            fireEvent.click(screen.getByText("Disk usage"));
            await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
            expect(api.diskStatus).toHaveBeenCalledTimes(1);
        }
        finally {
            vi.useRealTimers();
        }
    });
    it("retains the previous useful measurement while a new run starts", async () => {
        await show();
        api.startDiskScan.mockResolvedValue({ run_id: "new", running: true, visited: 0, current_path: null, observations: [], error: null });
        api.diskStatus.mockResolvedValue({ run_id: "new", running: true, visited: 0, current_path: null, observations: [], error: null });
        await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Measure disk usage" })); });
        expect(screen.getByText("30.0 GiB unexplained")).toBeTruthy();
        await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Cancel scan" })); });
        expect(api.cancelDiskScan).toHaveBeenCalledWith("new");
    });
    it("stops polling after an unsupported backend response and preserves the error", async () => {
        vi.useFakeTimers();
        try {
            api.diskStatus.mockRejectedValue(new Error("Unsupported command"));
            await show();
            await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
            expect(api.diskStatus).toHaveBeenCalledTimes(1);
            expect(screen.getByText(/Unsupported command/)).toBeTruthy();
        }
        finally {
            vi.useRealTimers();
        }
    });
});

it("lets users exclude additional locations again without starting a scan", async () => {
  api.diskSettings.mockResolvedValue({external_roots:["/fixture/builds"],additional_locations:true});
  await show();
  await act(async()=>{fireEvent.click(screen.getByRole("button",{name:"Exclude additional locations from future scans"}));});
  expect(api.saveDiskSettings).toHaveBeenCalledWith({external_roots:["/fixture/builds"],additional_locations:false});
  expect(api.startDiskScan).not.toHaveBeenCalled();
});
