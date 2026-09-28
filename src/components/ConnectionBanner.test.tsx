import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnectionState } from "@/api/connection";
import { REQUIRED_PROTOCOL_VERSION } from "@/lib/protocol";
import { stubViewport } from "@/test-utils";

const connection = vi.hoisted(() => ({ current: { kind: "local" } as ConnectionState }));

// The update banner is an ExternalLink, which opens through the opener
// plugin; there is no plugin in jsdom.
const openUrl = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (u: string) => openUrl(u) }));

vi.mock("@/api/connection", () => ({
  useConnectionState: () => connection.current,
}));

// The banner opens Settings, which reads every preference hook. None of
// them matter here beyond not throwing.
vi.mock("./SettingsDialog", () => ({
  SettingsDialog: ({ initialSection }: { initialSection?: string }) => (
    <div role="dialog">Settings: {initialSection}</div>
  ),
}));

import { ConnectionBanner } from "./ConnectionBanner";

afterEach(() => {
  cleanup();
  stubViewport(null);
  connection.current = { kind: "local" };
});

describe("ConnectionBanner", () => {
  it("renders nothing on the desktop layout", () => {
    stubViewport(1400);
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: null,
      protocolVersion: REQUIRED_PROTOCOL_VERSION,
      stale: false,
    };
    const { container } = render(<ConnectionBanner />);
    expect(container.innerHTML).toBe("");
  });

  it("renders nothing while the connection is local, even when narrow", () => {
    // A desktop build in a narrow dev browser has no desktop to name.
    stubViewport(390);
    connection.current = { kind: "local" };
    const { container } = render(<ConnectionBanner />);
    expect(container.innerHTML).toBe("");
  });

  it("names the desktop, says it is reachable, and shows the last poll", () => {
    stubViewport(390);
    const tenMinutesAgo = new Date(Date.now() - 10 * 60_000).toISOString();
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: tenMinutesAgo,
      protocolVersion: REQUIRED_PROTOCOL_VERSION,
      stale: false,
    };
    // `updatedAt` is GitHub's freshness, which moved here from the
    // bottom status bar (#649): while the desktop is reachable, whether
    // the DATA is current is the more useful of the two timestamps, and
    // showing both in two strips said almost the same thing twice.
    render(<ConnectionBanner updatedAt={Date.now() - 10 * 60_000} />);
    const banner = screen.getByRole("button", { name: /octocat's laptop/ });
    expect(banner.textContent).toContain("reachable");
    // "PRs updated", not a bare "updated": the timestamp is the PR poll's,
    // and a subjectless one reads as a claim about everything (#1115).
    expect(banner.textContent).toContain("PRs updated 10 minutes ago");
  });

  /// Without a GitHub timestamp the line is just the desktop, rather
  /// than "updated never" or a dangling separator.
  it("says only what it knows when GitHub has not been polled", () => {
    stubViewport(390);
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: new Date().toISOString(),
      protocolVersion: REQUIRED_PROTOCOL_VERSION,
      stale: false,
    };
    render(<ConnectionBanner />);
    const banner = screen.getByRole("button", { name: /octocat's laptop/ });
    expect(banner.textContent).toContain("reachable");
    expect(banner.textContent).not.toContain("updated");
  });

  it("tells the user to update the desktop when its protocol is too old", () => {
    stubViewport(390);
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: null,
      protocolVersion: REQUIRED_PROTOCOL_VERSION - 1,
      stale: false,
    };
    render(<ConnectionBanner />);
    // A link to the desktop release, not a button into pairing
    // settings: pairing cannot fix an old desktop.
    expect(screen.queryByRole("button")).toBeNull();
    const banner = screen.getByRole("link");
    expect(banner.getAttribute("href")).toBe(
      "https://github.com/pktstorm/headstate/releases/latest",
    );
    expect(banner.textContent).toContain("Update Headstate on your desktop");
    // Names the minimum, and what the desktop reported.
    expect(banner.textContent).toContain(`needs protocol ${REQUIRED_PROTOCOL_VERSION}`);
    expect(banner.textContent).toContain(
      `octocat's laptop has ${REQUIRED_PROTOCOL_VERSION - 1}`,
    );
    expect(banner.textContent).not.toContain("reachable");
    fireEvent.click(banner);
    expect(openUrl).toHaveBeenCalledWith("https://github.com/pktstorm/headstate/releases/latest");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the ordinary connected line for a desktop at or above the required protocol", () => {
    stubViewport(390);
    for (const protocolVersion of [REQUIRED_PROTOCOL_VERSION, REQUIRED_PROTOCOL_VERSION + 1, null]) {
      connection.current = {
        kind: "connected",
        desktop: "octocat's laptop",
        lastPoll: null,
        protocolVersion,
        stale: false,
      };
      const { unmount } = render(<ConnectionBanner />);
      expect(screen.getByRole("button").textContent).toContain("reachable");
      expect(screen.queryByRole("link")).toBeNull();
      unmount();
    }
  });

  it("says when the desktop is away and when it was last seen", () => {
    stubViewport(390);
    connection.current = {
      kind: "unreachable",
      desktop: "octocat's laptop",
      lastPoll: new Date(Date.now() - 2 * 3_600_000).toISOString(),
      stale: true,
    };
    render(<ConnectionBanner />);
    expect(screen.getByRole("button").textContent).toContain(
      "octocat's laptop is unreachable · last seen 2 hours ago",
    );
  });

  it("invites pairing when there is no desktop", () => {
    stubViewport(390);
    connection.current = { kind: "unpaired" };
    render(<ConnectionBanner />);
    expect(screen.getByRole("button").textContent).toMatch(/not paired/i);
  });

  it("admits when it cannot ask, rather than claiming a state", () => {
    stubViewport(390);
    connection.current = { kind: "unknown" };
    render(<ConnectionBanner />);
    expect(screen.getByRole("button").textContent).toMatch(/unavailable/i);
  });

  it("opens Settings on the Phone topic when tapped", async () => {
    stubViewport(390);
    connection.current = { kind: "unpaired" };
    render(<ConnectionBanner />);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button"));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("phone"));
  });
});
