import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { PrKebab } from "@/components/PrKebab";
import { renderWithQuery as render } from "@/test-utils";
import type { PullRequest } from "@/types/pr";

const act = vi.fn(() => Promise.resolve());
vi.mock("@/api/hooks", () => ({
  useActOnPr: () => act,
  useSetAutoMerge: () => vi.fn(() => Promise.resolve()),
  useUpdatePrBranch: () => vi.fn(() => Promise.resolve()),
  // Claudify's inputs (#1455), rendered by this component since #1462.
  useWorktrees: () => ({ data: [], unreadable: [], isError: false, error: null }),
  useUiPrefs: () => ({ prefs: { terminal_command: null } }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const pr: PullRequest = {
  ...PR_FIXTURES[0],
  is_draft: false,
  in_merge_queue: false,
  merge: "mergeable",
  merge_status: "clean",
  ci: "success",
};

const open = () => fireEvent.click(screen.getByRole("button", { name: /Actions for/ }));

describe("PrKebab on a stacked row (#1452)", () => {
  it("disables Add to merge queue with the stack reason", () => {
    render(<PrKebab pr={pr} stackedOn={17} />);
    open();
    const item = screen.getByRole("menuitem", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(item.disabled).toBe(true);
    expect(item.title).toBe("stacked on #17 — merge #17 first");
    fireEvent.click(item);
    expect(act).not.toHaveBeenCalled();
  });

  it("offers it on a row that is not stacked", () => {
    render(<PrKebab pr={pr} />);
    open();
    expect((screen.getByRole("menuitem", { name: "Add to merge queue" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
});
