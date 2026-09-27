import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PairedDevice } from "../api/tauri";

const revoke = vi.hoisted(() => vi.fn<(id: number) => Promise<void>>());
const list = vi.hoisted(() => ({
  data: undefined as PairedDevice[] | undefined,
  isLoading: false,
  error: null as unknown,
}));
const setAccess = vi.hoisted(() =>
  vi.fn<(id: number, transcripts: boolean, reveal: boolean) => Promise<void>>(),
);
vi.mock("../api/hooks", () => ({
  usePairedDevices: () => list,
  useRevokePairedDevice: () => revoke,
  useSetPairedDeviceAccess: () => setAccess,
}));

import { PairedDevicesList } from "./PairedDevicesList";

const NOW = new Date("2026-09-05T10:00:00Z");

const PHONE: PairedDevice = {
  id: 1,
  name: "Octocat's phone",
  cert_fp: "ab12cd34ef5601237890abcdef0123456789abcdef0123456789abcdef012345",
  has_mldsa: true,
  paired_at: "2026-09-01T10:00:00Z",
  last_seen: "2026-09-05T07:00:00Z",
  transcripts_allowed: true,
  reveal_allowed: false,
};

const TABLET: PairedDevice = {
  id: 2,
  name: "Octocat's tablet",
  cert_fp: "cd".repeat(32),
  has_mldsa: false,
  paired_at: "2026-08-20T10:00:00Z",
  last_seen: null,
  transcripts_allowed: true,
  reveal_allowed: false,
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  revoke.mockReset();
  revoke.mockImplementation(() => Promise.resolve());
  setAccess.mockReset();
  setAccess.mockImplementation(() => Promise.resolve());
  list.data = [PHONE, TABLET];
  list.isLoading = false;
  list.error = null;
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const revokeButton = (name: string) => screen.getByRole("button", { name: `Revoke ${name}` });

describe("PairedDevicesList", () => {
  it("lists each phone with when it was paired and last seen", () => {
    render(<PairedDevicesList />);
    expect(screen.getByText("Octocat's phone")).toBeTruthy();
    expect(screen.getByText("Octocat's tablet")).toBeTruthy();
    // Paired date as a date, last seen relative -- the same wording the
    // PR list uses for its own timestamps.
    expect(screen.getByText(/Paired Sep 1, 2026 · Last seen 3 hours ago/)).toBeTruthy();
    expect(screen.getByText(/Paired Aug 20, 2026 · Never connected/)).toBeTruthy();
  });

  it("badges the phone that offered a post-quantum key, and only that one", () => {
    render(<PairedDevicesList />);
    expect(screen.getAllByText("post-quantum")).toHaveLength(1);
  });

  it("shows the fingerprint in groups of four", () => {
    render(<PairedDevicesList />);
    expect(
      screen.getByText(
        "ab12 cd34 ef56 0123 7890 abcd ef01 2345 6789 abcd ef01 2345 6789 abcd ef01 2345",
      ),
    ).toBeTruthy();
  });

  it("says so when nothing is paired", () => {
    list.data = [];
    render(<PairedDevicesList />);
    expect(screen.getByText(/no phones paired yet/i)).toBeTruthy();
  });

  it("shows why the list could not load", () => {
    list.error = "database is locked";
    render(<PairedDevicesList />);
    expect(screen.getByRole("alert").textContent).toBe("database is locked");
  });

  describe("revoke", () => {
    it("asks first, naming the device, and does nothing on Cancel", () => {
      render(<PairedDevicesList />);
      fireEvent.click(revokeButton("Octocat's phone"));
      const dialog = screen.getByRole("dialog");
      expect(dialog.textContent).toContain("Revoke Octocat's phone?");
      fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(revoke).not.toHaveBeenCalled();
    });

    it("revokes the confirmed device and closes", async () => {
      render(<PairedDevicesList />);
      fireEvent.click(revokeButton("Octocat's tablet"));
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /^revoke$/i }));
      });
      expect(revoke).toHaveBeenCalledWith(2);
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    it("keeps the dialog open and shows the reason when revoking fails", async () => {
      revoke.mockImplementation(() => Promise.reject("database is locked"));
      render(<PairedDevicesList />);
      fireEvent.click(revokeButton("Octocat's phone"));
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /^revoke$/i }));
      });
      expect(screen.getByRole("dialog")).toBeTruthy();
      expect(screen.getByRole("alert").textContent).toBe("database is locked");
    });
  });

  describe("transcript access (#1488)", () => {
    const readBox = () =>
      screen.getAllByRole("checkbox", { name: /read session transcripts/i })[0] as HTMLInputElement;
    const revealBox = () =>
      screen.getAllByRole("checkbox", { name: /reveal hidden text/i })[0] as HTMLInputElement;

    it("shows each phone's stored switches: reading on, reveal off by default", () => {
      render(<PairedDevicesList />);
      expect(readBox().checked).toBe(true);
      expect(revealBox().checked).toBe(false);
      expect(revealBox().disabled).toBe(false);
    });

    it("saves both switches for that phone only", async () => {
      render(<PairedDevicesList />);
      await act(async () => {
        fireEvent.click(revealBox());
      });
      expect(setAccess).toHaveBeenCalledWith(1, true, true);
      await act(async () => {
        fireEvent.click(readBox());
      });
      expect(setAccess).toHaveBeenLastCalledWith(1, false, false);
    });

    it("disables reveal, keeping its stored value, while reading is off", () => {
      list.data = [{ ...PHONE, transcripts_allowed: false, reveal_allowed: true }];
      render(<PairedDevicesList />);
      expect(readBox().checked).toBe(false);
      expect(revealBox().disabled).toBe(true);
      expect(revealBox().checked).toBe(true);
    });

    it("says why a change could not be saved", async () => {
      setAccess.mockImplementation(() => Promise.reject("this phone is no longer paired"));
      render(<PairedDevicesList />);
      await act(async () => {
        fireEvent.click(readBox());
      });
      expect(screen.getByRole("alert").textContent).toBe("this phone is no longer paired");
    });
  });
});
