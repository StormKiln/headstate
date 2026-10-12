import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConfigComparisonPanel } from "./ConfigComparisonPanel";

const api = vi.hoisted(() => ({ compare: vi.fn() }));
vi.mock("@/api/tauri", () => ({ claudeCompareConfig: api.compare }));
vi.mock("@/api/hooks", () => ({ useWorktrees: () => ({ data: [
  { path: "/code/alpha", name: "Alpha" }, { path: "/code/beta", name: "Beta" },
], unreadable: [], error: null, isLoading: false }) }));
vi.mock("@/store/filters", () => ({ useActiveFilters: () => ({ repo: "/code/alpha" }) }));

const result = { leftLabel: "Alpha", rightLabel: "User configuration", limitations: [], rows: [
  { id: "model", category: "settings", name: "model", status: "different",
    left: { state: "present", value: '"opus"', detail: ".claude/settings.local.json" },
    right: { state: "present", value: '"sonnet"', detail: "~/.claude/settings.json" } },
] };
const mount = () => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ConfigComparisonPanel enabled /></QueryClientProvider>);
const open = () => fireEvent.click(screen.getByRole("button", { name: "Compare repository configurations" }));
beforeEach(() => api.compare.mockReset().mockResolvedValue(result));
afterEach(cleanup);

it("reads only after opening and reuses the result when reopened", async () => {
  mount();
  expect(api.compare).not.toHaveBeenCalled();
  open();
  await screen.findByText('"opus"');
  expect(api.compare).toHaveBeenCalledWith("/code/alpha", null);
  open(); open();
  await act(async () => {});
  expect(api.compare).toHaveBeenCalledTimes(1);
  expect(screen.getByText(".claude/settings.local.json")).toBeTruthy();
});

it("keeps a previous comparison visible on a failed explicit refresh", async () => {
  mount(); open();
  await screen.findByText('"opus"');
  api.compare.mockRejectedValueOnce(new Error("File read failed"));
  fireEvent.click(screen.getByRole("button", { name: "Refresh comparison" }));
  await screen.findByText(/File read failed/);
  expect(screen.getByText('"opus"')).toBeTruthy();
  expect(screen.getByText(/Showing the previous comparison/)).toBeTruthy();
});

it("never shows the prior pair's result while a new pair is loading", async () => {
  mount(); open();
  await screen.findByText('"opus"');
  api.compare.mockImplementationOnce(() => new Promise(() => {}));
  fireEvent.change(screen.getByLabelText("Compare with"), { target: { value: "/code/beta" } });
  await waitFor(() => expect(api.compare).toHaveBeenLastCalledWith("/code/alpha", "/code/beta"));
  expect(screen.queryByText('"opus"')).toBeNull();
  expect(screen.getByText("Reading configurations…")).toBeTruthy();
});

it("shows unknown separately from missing and keeps partial scan warnings", async () => {
  api.compare.mockResolvedValue({ ...result, limitations: ["Some directories could not be read"], rows: [{
    ...result.rows[0], status: "cannotCompare", left: { state: "unknown", value: null, detail: "Invalid JSON" },
  }] });
  mount(); open();
  await screen.findByText("Invalid JSON");
  expect(screen.getByText("Cannot compare (1)")).toBeTruthy();
  expect(screen.getByText("Some directories could not be read")).toBeTruthy();
  expect(screen.queryByText("Not present")).toBeNull();
});

it("preserves the selected pair when collapsed and reopened", async () => {
  mount(); open();
  await screen.findByText('"opus"');
  fireEvent.change(screen.getByLabelText("Compare with"), { target: { value: "/code/beta" } });
  await waitFor(() => expect(api.compare).toHaveBeenLastCalledWith("/code/alpha", "/code/beta"));
  open(); open();
  expect((screen.getByLabelText("Compare with") as HTMLSelectElement).value).toBe("/code/beta");
  await act(async () => {});
  expect(api.compare).toHaveBeenCalledTimes(2);
});
