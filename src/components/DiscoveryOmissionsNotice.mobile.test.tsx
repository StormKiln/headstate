import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// The iOS build: scan folders are the desktop's, so the notice states the
// instruction without a button into a Settings topic.
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));
import { DiscoveryOmissionsNotice } from "./DiscoveryOmissionsNotice";

describe("DiscoveryOmissionsNotice on the iOS build", () => {
  it("keeps the qualified count and the instruction, without a Settings button", () => {
    render(
      <DiscoveryOmissionsNotice
        omissions={[{ root: "/fixture/projects/p/deep", why: "Permission denied" }]}
        scanDirs={["/fixture/projects"]}
      />,
    );
    expect(screen.getByText(/Discovery is incomplete in 1 location\./)).toBeTruthy();
    expect(screen.getByText(/Add a deeper folder in Settings → Repositories/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Open repository settings" })).toBeNull();
  });
});
