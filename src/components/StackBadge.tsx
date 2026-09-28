import { stackLabel, stackTitle } from "../lib/stack";
import type { PrStack } from "../types/pr";

/// Where a pull request sits in a stack (#1452), for the detail view's
/// metadata line. Taken from GitHub rather than the list, where the parent
/// is usually not on screen.
///
/// Renders nothing while not yet asked, unknown, or not stacked: a guessed
/// position is worse than none. Leads with its own separator so the host
/// line needs only the one element.
export function StackBadge({ stack }: { stack: PrStack | undefined }) {
  if (stack?.kind !== "stacked") return null;
  return (
    <>
      <span aria-hidden="true">·</span>
      <span
        data-testid="stack-badge"
        className="rounded-full border border-[#a371f7]/40 px-1.5 text-[#a371f7]"
        title={stackTitle(stack)}
      >
        {stackLabel(stack)}
      </span>
    </>
  );
}
