import type { PrStack } from "../types/pr";

type Stacked = Extract<PrStack, { kind: "stacked" }>;

/// The facts the queue gate needs, from whichever source has them.
///
/// The detail view has the whole `PrStack` from GitHub. List rows have only
/// `deriveStacked`'s parent number, which proves the row IS stacked (its
/// base is another open PR's head) but cannot see a native stack or a
/// parent outside the list -- so a list row can be gated on less evidence,
/// never on more. `stackFactsFromList` builds the same shape from it, and
/// both paths then go through ONE predicate.
export interface StackFacts {
  native: boolean;
  stack_number: number | null;
  position: number;
  below: number | null;
  label: string | null;
}

export function stackFacts(stack: PrStack | undefined): StackFacts | null {
  if (stack?.kind !== "stacked") return null;
  return {
    native: stack.native,
    stack_number: stack.stack_number,
    position: stack.position,
    below: stack.below,
    label: stackLabel(stack),
  };
}

/// From `deriveStacked`'s answer for one list row. `position` is 2 as a
/// floor: the row is on SOMETHING, which is all this source can say.
export function stackFactsFromList(stackedOn: number | undefined): StackFacts | null {
  if (stackedOn === undefined) return null;
  return { native: false, stack_number: null, position: 2, below: stackedOn, label: null };
}

/// "stack 2/4", qualified with "at least" where a walk stopped early.
///
/// Null for anything not stacked -- including `unknown` and not-yet-asked,
/// which render nothing rather than a guess.
export function stackLabel(stack: PrStack | undefined): string | null {
  if (stack?.kind !== "stacked") return null;
  if (stack.position_exact && stack.size_exact) return `stack ${stack.position}/${stack.size}`;
  const pos = stack.position_exact ? `${stack.position}` : `at least ${stack.position}`;
  const size = stack.size_exact ? `${stack.size}` : `at least ${stack.size}`;
  return `stack ${pos} of ${size}`;
}

/// The badge's tooltip: what the numbers mean and where they came from.
export function stackTitle(stack: Stacked): string {
  const where = stack.native
    ? `GitHub stack${stack.stack_number ? ` #${stack.stack_number}` : ""}`
    : "a stack of pull requests, each based on the branch of the one below";
  const below = stack.below ? ` Directly on #${stack.below}.` : "";
  const partial =
    stack.position_exact && stack.size_exact
      ? ""
      : " Only part of the stack was checked, so these numbers are minimums.";
  return `Position ${stack.position} (1 is closest to the base branch) in ${where}.${below}${partial}`;
}

/// Why "Add to merge queue" must not be offered, or null.
///
/// - A NATIVE stack: GitHub merges it only through its stack merge. The
///   schema says `mergePullRequest` "does not support stacked pull
///   requests", and GitHub's stack merge API documentation says the legacy
///   merge paths cannot merge a stack. That holds at every position,
///   including the bottom.
/// - A base-chain stack above the bottom: the pull request's base is
///   another open pull request's branch, so queueing it would merge into
///   THAT branch -- the refusal #743 recorded. The bottom of such a stack
///   targets the trunk and queues normally, so it is not gated.
export function stackBlocksQueue(facts: StackFacts | null): string | null {
  if (facts === null) return null;
  if (facts.native) return nativeReason(facts);
  if (facts.below !== null) return `stacked on #${facts.below} — merge #${facts.below} first`;
  if (facts.position > 1) return "stacked on another open pull request's branch — merge that one first";
  return null;
}

/// Why a plain Merge must not be offered, or null. Native stacks only: a
/// base-chain PR merges into its parent's branch, which GitHub allows.
export function stackBlocksMerge(facts: StackFacts | null): string | null {
  return facts?.native ? nativeReason(facts) : null;
}

function nativeReason(facts: StackFacts): string {
  const which = facts.stack_number ? `GitHub stack #${facts.stack_number}` : "a GitHub stack";
  const where = facts.label ? ` (${facts.label})` : "";
  return `part of ${which}${where}, which GitHub merges only as a stack — merge it on GitHub or with "gh stack merge"`;
}
