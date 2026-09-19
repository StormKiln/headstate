import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { WorktreeKebab } from "./WorktreeKebab";
import type { Worktree } from "../types/pr";

const wt: Worktree = {
  path: "/code/proj-a",
  branch: "feature",
  head: "abc",
  size_bytes: 1024,
  safety: { kind: "unmerged" },
  is_main: false,
  merged_at: null,
  upstream: null,
  last_commit: null,
};

function open(props: Partial<Parameters<typeof WorktreeKebab>[0]> = {}) {
  const handlers = {
    onClaudify: vi.fn(),
    onCopyClaudify: vi.fn(),
    onForget: vi.fn(),
    onRemove: vi.fn(),
    onForce: vi.fn(),
    onUnlock: vi.fn(),
  };
  render(<WorktreeKebab worktree={wt} assessed {...handlers} {...props} />);
  fireEvent.click(screen.getByRole("button", { name: /more/i }));
  return handlers;
}

/// The Claudify items, which #1126 split in two.
describe("WorktreeKebab and the configured terminal", () => {
  it("offers only Copy when no terminal is configured", () => {
    // The pre-existing behaviour, and the default. A second item would
    // duplicate the row's own button, which is what the original
    // comment says the menu must not do.
    open();
    expect(screen.getByText("Copy the Claudify command")).toBeTruthy();
    expect(screen.queryByText("Open the Claudify command")).toBeNull();
  });

  it("offers BOTH once a terminal is configured", () => {
    // Copy must stay reachable: the terminal is one user's choice of
    // one tool, and the raw string is what you need to paste elsewhere
    // or read before running.
    open({ terminalConfigured: true });
    expect(screen.getByText("Open the Claudify command")).toBeTruthy();
    expect(screen.getByText("Copy the Claudify command")).toBeTruthy();
  });

  it("routes Copy to onCopyClaudify, never to the primary action", () => {
    // THE bug this split exists to prevent: with a terminal configured
    // `onClaudify` launches, so an item labelled "Copy" wired to it
    // would open a terminal -- a button lying about itself.
    const h = open({ terminalConfigured: true });
    fireEvent.click(screen.getByText("Copy the Claudify command"));
    expect(h.onCopyClaudify).toHaveBeenCalledOnce();
    expect(h.onClaudify).not.toHaveBeenCalled();
  });

  it("routes Open to the primary action, so it cannot drift from the button", () => {
    const h = open({ terminalConfigured: true });
    fireEvent.click(screen.getByText("Open the Claudify command"));
    expect(h.onClaudify).toHaveBeenCalledOnce();
    expect(h.onCopyClaudify).not.toHaveBeenCalled();
  });

  it("still routes the single item to Copy when no terminal is configured", () => {
    // The unconfigured menu's only Claudify item must copy. Wiring it
    // to `onClaudify` would happen to work today -- the primary copies
    // too -- and break silently the moment a terminal is set.
    const h = open();
    fireEvent.click(screen.getByText("Copy the Claudify command"));
    expect(h.onCopyClaudify).toHaveBeenCalledOnce();
    expect(h.onClaudify).not.toHaveBeenCalled();
  });
});
