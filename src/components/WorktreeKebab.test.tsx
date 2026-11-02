import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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

/// The menu was cut off by the list it sits in: it opened `absolute
/// top-full` inside the row, and the rows' container is
/// `overflow-hidden`, so on the last rows Remove was clipped out of
/// reach. jsdom has no layout, so the flip itself cannot be measured
/// here -- what CAN be asserted is the structural fix: the menu is no
/// longer a descendant of anything that might clip it.
describe("WorktreeKebab is not clipped by its container", () => {
  function renderInClippingList() {
    const handlers = {
      onClaudify: vi.fn(),
      onCopyClaudify: vi.fn(),
      onForget: vi.fn(),
      onRemove: vi.fn(),
      onForce: vi.fn(),
      onUnlock: vi.fn(),
    };
    const onRowClick = vi.fn();
    render(
      <div data-testid="list" style={{ overflow: "hidden", height: 24 }}>
        <div onClick={onRowClick}>
          <WorktreeKebab worktree={wt} assessed {...handlers} />
        </div>
      </div>,
    );
    fireEvent.click(screen.getByRole("button", { name: /more actions for feature/i }));
    return { handlers, onRowClick };
  }

  it("renders the menu outside the overflow container (a portal)", () => {
    renderInClippingList();
    const menu = screen.getByRole("menu");
    expect(screen.getByTestId("list").contains(menu)).toBe(false);
    expect(document.body.contains(menu)).toBe(true);
  });

  it("keeps the trigger's menu semantics", () => {
    renderInClippingList();
    const trigger = screen.getByRole("button", { name: /more actions for feature/i });
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
  });

  it("still keeps an item click from reaching the row behind the portal", () => {
    // React bubbles synthetic events along the component tree, not the
    // DOM, so a portalled item's click would reach the row's own
    // handler without the wrapper's stopPropagation.
    const { handlers, onRowClick } = renderInClippingList();
    fireEvent.click(screen.getByRole("menuitem", { name: /remove worktree/i }));
    expect(handlers.onForce).toHaveBeenCalledOnce();
    expect(onRowClick).not.toHaveBeenCalled();
  });

  it("closes on Escape", async () => {
    renderInClippingList();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });
});
