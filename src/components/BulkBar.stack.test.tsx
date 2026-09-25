import { fireEvent, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BatchOutcome } from "@/api/tauri";
import { BulkBar, prKey } from "@/components/BulkBar";
import { PR_FIXTURES } from "@/fixtures/prs";
import { useFilters } from "@/store/filters";
import { renderWithQuery as render } from "@/test-utils";
import type { PullRequest } from "@/types/pr";

type Batch = (prs: [string, string, number][], action: string) => Promise<BatchOutcome[]>;
const batch = vi.fn<Batch>(() => Promise.resolve([]));
vi.mock("@/api/hooks", () => ({ useActOnPrs: () => batch }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

const parent: PullRequest = { ...PR_FIXTURES[0], in_merge_queue: false, is_draft: false };
const child: PullRequest = {
  ...parent,
  id: "PR_stacked_child",
  number: parent.number + 1000,
  title: "Layer two",
  head_ref: "layer-two",
  base_ref: parent.head_ref,
};

beforeEach(() => {
  batch.mockClear();
  useFilters.getState().setChecked([prKey(parent), prKey(child)]);
});

describe("BulkBar enqueue on a stacked selection (#1452)", () => {
  it("skips the stacked pull request with the reason, and queues the rest", () => {
    batch.mockResolvedValueOnce([{ repo: parent.repo, number: parent.number, error: null }]);
    render(<BulkBar prs={[parent, child]} />);
    fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));

    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(`(stacked on #${parent.number} — merge #${parent.number} first)`)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: /^Add to merge queue 1 pull request$/ }));

    expect(batch).toHaveBeenCalledWith([[parent.id, parent.repo, parent.number]], "enqueue");
  });

  /// The gate is for the queue only: other bulk actions still include it.
  it("does not skip the stacked pull request for other actions", () => {
    render(<BulkBar prs={[parent, child]} />);
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    expect(within(screen.getByRole("dialog")).queryByText(/stacked on/)).toBeNull();
  });
});
