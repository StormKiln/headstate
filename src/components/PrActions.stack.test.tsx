import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PrDetail, PrStack } from "@/types/pr";

const actFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));

vi.mock("../api/hooks", () => ({ useActOnPr: () => actFn }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { PrActions } from "./PrActions";

const pr = (over: Partial<PrDetail> = {}): PrDetail => ({
  id: "PR_stack",
  number: 30,
  title: "Third layer",
  url: "u",
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
  review: "approved",
  additions: 1,
  deletions: 0,
  changed_files: 1,
  unresolved_threads: 0,
  comment_count: 0,
  comments: [],
  review_threads: [],
  review_threads_total: 0,
  latest_reviews: [],
  merge_queue_enabled: true,
  in_merge_queue: false,
  checks: [],
  checks_total: 0,
  ...over,
});

const chain: PrStack = {
  kind: "stacked",
  native: false,
  stack_number: null,
  position: 3,
  size: 4,
  position_exact: true,
  size_exact: true,
  below: 20,
};

beforeEach(() => actFn.mockClear());

describe("PrActions on a stacked pull request (#1452)", () => {
  /// The reported defect: approved, green, stacked -- and "Add to merge
  /// queue" was offered and then refused. It is still SHOWN, disabled
  /// with the reason, because an absent button teaches nothing.
  it("disables Add to merge queue and names the pull request beneath", () => {
    render(<PrActions pr={pr({ stack: chain })} />);
    const button = screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.title).toBe("stacked on #20 — merge #20 first");
    expect(screen.getByText(/Cannot queue: stacked on #20/)).toBeTruthy();
    fireEvent.click(button);
    expect(actFn).not.toHaveBeenCalled();
  });

  it("disables a plain Merge on a native stack as well, naming the stack", () => {
    const native: PrStack = { ...chain, native: true, stack_number: 7 };
    render(<PrActions pr={pr({ stack: native, merge_queue_enabled: false })} />);
    const button = screen.getByRole("button", { name: "Merge" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.title).toMatch(/GitHub stack #7 \(stack 3\/4\)/);
  });

  /// Only the queue is refused for a base-chain stack; merging into the
  /// parent's branch is something GitHub allows.
  it("leaves a plain Merge alone on a base-chain stack", () => {
    render(<PrActions pr={pr({ stack: chain, merge_queue_enabled: false })} />);
    expect((screen.getByRole("button", { name: "Merge" }) as HTMLButtonElement).disabled).toBe(false);
  });

  /// Both gates on enqueue (#1452 with #1454): the stack is named first,
  /// because resolving conversations would not make the queue accept a
  /// stacked pull request; the conversations reason still applies once the
  /// pull request is not stacked.
  it("names the stack before open conversations when both block the queue", () => {
    const conversations = "2 conversations must be resolved first";
    const blocked = { merge_status: "blocked" } as const;
    const { unmount } = render(
      <PrActions pr={pr({ ...blocked, stack: chain })} conversations={conversations} />,
    );
    const stacked = screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(stacked.disabled).toBe(true);
    expect(stacked.title).toBe("stacked on #20 — merge #20 first");
    unmount();

    render(
      <PrActions
        pr={pr({ ...blocked, stack: { kind: "none" }, base_ref: "main" })}
        conversations={conversations}
      />,
    );
    const plain = screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(plain.disabled).toBe(true);
    expect(plain.title).toBe(conversations);
  });

  it("leaves a pull request that is not stacked unaffected", () => {
    render(<PrActions pr={pr({ stack: { kind: "none" }, base_ref: "main" })} />);
    const button = screen.getByRole("button", { name: "Add to merge queue" }) as HTMLButtonElement;
    expect(button.disabled).toBe(false);
    expect(screen.queryByText(/Cannot queue/)).toBeNull();
  });
});
