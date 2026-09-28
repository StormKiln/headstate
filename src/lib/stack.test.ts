import { describe, expect, it } from "vitest";
import type { PrStack } from "@/types/pr";
import {
  stackBlocksMerge,
  stackBlocksQueue,
  stackFacts,
  stackGate,
  stackMergePlan,
  numberList,
  stackFactsFromList,
  stackLabel,
  stackTitle,
} from "./stack";

const stacked = (over: Partial<Extract<PrStack, { kind: "stacked" }>> = {}) =>
  ({
    kind: "stacked",
    native: false,
    stack_number: null,
    position: 2,
    size: 4,
    position_exact: true,
    size_exact: true,
    below: 11,
    ...over,
  }) as const;

describe("stackLabel", () => {
  it("prints an exact position as x/y", () => {
    expect(stackLabel(stacked())).toBe("stack 2/4");
  });

  /// "Qualify, or suppress": a walk that stopped early is a floor, and
  /// says so rather than printing a confident total.
  it("qualifies a total the upward walk did not finish", () => {
    expect(stackLabel(stacked({ size_exact: false }))).toBe("stack 2 of at least 4");
  });

  it("qualifies both when the downward walk did not finish", () => {
    expect(stackLabel(stacked({ position: 5, size: 6, position_exact: false, size_exact: false }))).toBe(
      "stack at least 5 of at least 6",
    );
  });

  /// Not asked, could not tell, and not stacked all render nothing -- and
  /// none of them renders as a position.
  it("prints nothing unless stacked", () => {
    expect(stackLabel(undefined)).toBeNull();
    expect(stackLabel({ kind: "unknown" })).toBeNull();
    expect(stackLabel({ kind: "none" })).toBeNull();
  });

  it("explains a partial walk in the tooltip", () => {
    expect(stackTitle(stacked({ size_exact: false }))).toMatch(/minimums/);
    expect(stackTitle(stacked())).not.toMatch(/minimums/);
  });
});

describe("stackBlocksQueue", () => {
  it("names the pull request to merge first", () => {
    expect(stackBlocksQueue(stackFacts(stacked()))).toBe("stacked on #11 — merge #11 first");
  });

  /// GitHub merges a native stack only as a stack, at every position.
  it("blocks every member of a native stack, naming the stack", () => {
    const facts = stackFacts(stacked({ native: true, stack_number: 7, position: 1, below: null }));
    const why = stackBlocksQueue(facts);
    expect(why).toMatch(/GitHub stack #7/);
    expect(why).toMatch(/stack 1\/4/);
    expect(stackBlocksMerge(facts)).toBe(why);
  });

  /// The bottom of a base-chain stack targets the trunk and queues as
  /// normal: nothing is beneath it to merge first.
  it("leaves the bottom of a base-chain stack alone", () => {
    expect(stackBlocksQueue(stackFacts(stacked({ position: 1, below: null })))).toBeNull();
  });

  it("blocks an ambiguous parent without inventing its number", () => {
    const why = stackBlocksQueue(stackFacts(stacked({ below: null, position_exact: false })));
    expect(why).toMatch(/another open pull request/);
  });

  it("does not block a pull request that is not stacked, or not yet known", () => {
    expect(stackBlocksQueue(stackFacts({ kind: "none" }))).toBeNull();
    expect(stackBlocksQueue(stackFacts({ kind: "unknown" }))).toBeNull();
    expect(stackBlocksQueue(stackFacts(undefined))).toBeNull();
  });

  /// A base-chain pull request merges into its parent's branch, which
  /// GitHub allows -- only the queue is gated for it.
  it("does not block a plain merge outside a native stack", () => {
    expect(stackBlocksMerge(stackFacts(stacked()))).toBeNull();
  });

  it("gates a list row on the same predicate", () => {
    expect(stackBlocksQueue(stackFactsFromList(12))).toBe("stacked on #12 — merge #12 first");
    expect(stackBlocksQueue(stackFactsFromList(undefined))).toBeNull();
  });
});

describe("stackMergePlan (#1468)", () => {
  const native = stacked({
    native: true,
    stack_number: 7,
    position: 3,
    members: [
      { position: 3, number: 30, title: "c", state: "open" },
      { position: 1, number: 10, title: "a", state: "merged" },
      { position: 2, number: 20, title: "b", state: "open" },
      { position: 4, number: 40, title: "d", state: "open" },
    ],
    members_complete: true,
  });

  it("lands the open pull requests beneath and this one, bottom first", () => {
    expect(stackMergePlan(native, 30)?.map((m) => m.number)).toEqual([20, 30]);
    expect(numberList(stackMergePlan(native, 30) ?? [])).toBe("#20 and #30");
  });

  it("is not offered on a partial membership, a base-chain stack, or a PR not in it", () => {
    expect(stackMergePlan({ ...native, members_complete: false }, 30)).toBeNull();
    expect(stackMergePlan(stacked(), 30)).toBeNull();
    expect(stackMergePlan(native, 99)).toBeNull();
  });

  /// A stack GitHub can merge is not "blocked" -- the #1452 gate steps aside.
  it("lifts the #1452 gate only when the stack merge is offered", () => {
    expect(stackGate(native, 30)).toBeNull();
    expect(stackGate({ ...native, members_complete: false }, 30)?.native).toBe(true);
  });
});
