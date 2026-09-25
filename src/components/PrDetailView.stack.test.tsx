import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PrDetail } from "@/types/pr";

const state = vi.hoisted(() => ({
  data: undefined as PrDetail | undefined,
  isLoading: false,
  isPlaceholderData: false,
  isError: false,
}));

vi.mock("../api/hooks", () => ({
  useClaudeSessionsForPr: () => ({ data: [] }),
  usePrDetail: () => ({ ...state, error: null, refetch: vi.fn() }),
  useActOnPr: () => vi.fn(() => Promise.resolve()),
  useDeleteHeadBranch: () => vi.fn(() => Promise.resolve()),
  useReviewPr: () => vi.fn(() => Promise.resolve()),
  useRerunChecks: () => vi.fn(() => Promise.resolve()),
  useCommentOnPr: () => vi.fn(() => Promise.resolve()),
  useResolveThread: () => vi.fn(() => Promise.resolve()),
  useUnresolveThread: () => vi.fn(() => Promise.resolve()),
  useReplyToThread: () => vi.fn(() => Promise.resolve()),
  useViewer: () => ({ data: undefined }),
  // Claudify's inputs (#1455), rendered by this component since #1462.
  useWorktrees: () => ({ data: [], unreadable: [], isError: false, error: null }),
  useUiPrefs: () => ({ prefs: { terminal_command: null } }),
  // Review gates (#1454): not fetched here, as when the lookup is pending.
  useReviewGates: () => ({ data: undefined }),
}));

import { PrDetailView } from "./PrDetailView";

const detail = (over: Partial<PrDetail> = {}): PrDetail => ({
  id: "PR_stack",
  number: 30,
  title: "Third layer",
  url: "https://example.invalid/acme/widgets/pull/30",
  state: "open",
  is_draft: false,
  body: "",
  author: "someone",
  repo: "acme/widgets",
  head_ref: "feat-c",
  head_oid: "oid",
  head_ref_id: null,
  base_ref: "feat-b",
  merge_status: "clean",
  review: "none",
  additions: 1,
  deletions: 0,
  changed_files: 1,
  unresolved_threads: 0,
  comment_count: 0,
  comments: [],
  review_threads: [],
  review_threads_total: 0,
  latest_reviews: [],
  merge_queue_enabled: false,
  in_merge_queue: false,
  checks: [],
  checks_total: 0,
  ...over,
});

const view = (over: Partial<PrDetail>) => {
  state.data = detail(over);
  return render(<PrDetailView repo="acme/widgets" number={30} onBack={() => {}} />);
};

describe("PrDetailView stack badge (#1452)", () => {
  afterEach(cleanup);

  it("shows the position GitHub reported", () => {
    view({
      stack: {
        kind: "stacked",
        native: false,
        stack_number: null,
        position: 3,
        size: 4,
        position_exact: true,
        size_exact: true,
        below: 20,
      },
    });
    expect(screen.getByTestId("stack-badge").textContent).toBe("stack 3/4");
  });

  it("qualifies a position from an incomplete walk", () => {
    view({
      stack: {
        kind: "stacked",
        native: false,
        stack_number: null,
        position: 2,
        size: 5,
        position_exact: true,
        size_exact: false,
        below: 20,
      },
    });
    expect(screen.getByTestId("stack-badge").textContent).toBe("stack 2 of at least 5");
  });

  /// Not asked yet (the seeded row), could not tell, and not stacked all
  /// show no badge -- never a guessed position.
  it("shows nothing unless the pull request is known to be stacked", () => {
    for (const stack of [undefined, { kind: "unknown" } as const, { kind: "none" } as const]) {
      view({ stack });
      expect(screen.queryByTestId("stack-badge")).toBeNull();
      cleanup();
    }
  });
});
