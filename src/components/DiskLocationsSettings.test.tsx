import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ diskSettings: vi.fn(), saveDiskSettings: vi.fn() }));
vi.mock("@/api/diskInventory", () => api);
import { DiskLocationsSettings } from "./DiskLocationsSettings";

async function show() {
  await act(async () => {
    render(<DiskLocationsSettings />);
  });
}
const box = () => screen.getByLabelText("External build and cache folders") as HTMLTextAreaElement;
const saveButton = () => screen.getByRole("button", { name: "Save folders" }) as HTMLButtonElement;

beforeEach(() => {
  vi.clearAllMocks();
  api.diskSettings.mockResolvedValue({ external_roots: ["/fixture/cache"], additional_locations: true });
  api.saveDiskSettings.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("external build and cache folders in Settings", () => {
  it("shows the saved folders, one per line", async () => {
    await show();
    expect(box().value).toBe("/fixture/cache");
    // Nothing edited: nothing to save.
    expect(saveButton().disabled).toBe(true);
  });

  it("saves the folders and keeps the additional-locations choice as read", async () => {
    await show();
    fireEvent.change(box(), { target: { value: " /fixture/cache \n\n/fixture/other\n" } });
    await act(async () => {
      fireEvent.click(saveButton());
    });
    expect(api.saveDiskSettings).toHaveBeenCalledWith({
      external_roots: ["/fixture/cache", "/fixture/other"],
      additional_locations: true,
    });
  });

  it("shows a refused save rather than pretending it saved", async () => {
    api.saveDiskSettings.mockRejectedValue("Stop the current scan before changing its locations.");
    await show();
    fireEvent.change(box(), { target: { value: "/fixture/other" } });
    await act(async () => {
      fireEvent.click(saveButton());
    });
    expect(screen.getByRole("alert").textContent).toMatch(/Stop the current scan/);
    expect(box().value).toBe("/fixture/other");
  });

  it("cannot save over folders it could not read, and offers a retry", async () => {
    api.diskSettings.mockRejectedValueOnce(new Error("settings unreadable"));
    await show();
    expect(screen.getByRole("alert").textContent).toMatch(/Could not load the saved folders: settings unreadable/);
    expect(box().disabled).toBe(true);
    expect(saveButton().disabled).toBe(true);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Retry loading folders" }));
    });
    expect(box().value).toBe("/fixture/cache");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
