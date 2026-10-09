import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// The connection module, not the whole transport world beneath it:
// this panel's questions are "which desktop" and "what keys does this
// phone hold", and both come from there.
const state = vi.hoisted(() => ({
  current: { kind: "connected", desktop: "studio", lastPoll: null, protocolVersion: 2, stale: false } as Record<string, unknown>,
}));
const mldsa = vi.hoisted(() => ({ current: null as boolean | null }));
vi.mock("@/api/connection", () => ({
  useConnectionState: () => state.current,
  usePhoneHasMldsa: () => mldsa.current,
  isStale: () => false,
}));
vi.mock("@/api/pairing", () => ({
  useUnpair: () => ({ mutate: vi.fn(), isPending: false }),
}));

import { PairedDesktopPanel } from "./PairedDesktopPanel";

describe("PairedDesktopPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    state.current = {
      kind: "connected",
      desktop: "studio",
      lastPoll: null,
      protocolVersion: 2,
      stale: false,
    };
    mldsa.current = null;
  });

  it("keeps remote repair available while the desktop is unreachable", () => {
    state.current = { kind: "unreachable", desktop: "studio" };
    render(<PairedDesktopPanel />);
    expect(screen.getByRole("region", { name: "Remote connection" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check connection" })).toBeTruthy();
  });

  it("names the paired desktop", () => {
    render(<PairedDesktopPanel />);
    expect(screen.getByText("studio")).toBeTruthy();
  });

  /// #670: the desktop has always shown this in its paired-devices
  /// list, but the phone -- the device someone is actually holding
  /// when they wonder what their hardware does -- could not say.
  it("says when this phone signs with a post-quantum key", () => {
    mldsa.current = true;
    render(<PairedDesktopPanel />);
    expect(screen.getByText(/post-quantum/i)).toBeTruthy();
    expect(screen.getByText(/ML-DSA-65/)).toBeTruthy();
  });

  /// Stated plainly, not warned about. ECDSA P-256 is not broken, and
  /// a device that cannot hold an ML-DSA key has no fault the user can
  /// fix -- a warning would imply otherwise.
  it("says plainly when it signs with ECDSA alone", () => {
    mldsa.current = false;
    render(<PairedDesktopPanel />);
    expect(screen.getByText(/ECDSA P-256/)).toBeTruthy();
    expect(screen.queryByText(/ML-DSA-65/)).toBeNull();
  });

  /// Unanswered is not "no". A keychain that would not open is not
  /// evidence of a classical-only device, and on a desktop build there
  /// is no phone to describe at all -- so the line is absent rather
  /// than claiming either way.
  it("says nothing at all when the answer is unknown", () => {
    mldsa.current = null;
    render(<PairedDesktopPanel />);
    expect(screen.queryByText(/post-quantum/i)).toBeNull();
    expect(screen.queryByText(/ECDSA/)).toBeNull();
  });
});
