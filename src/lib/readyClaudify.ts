import { type ReadyRow, readyListMarkdown } from "./readyMarkdown";

/// The batch Claudify's instruction, VERBATIM from #1579 (the owner's
/// words). A test holds it byte for byte; rewording it here is a change
/// to what Claude is told to approve and merge, and belongs in an issue.
export const READY_BATCH_PROMPT = `Review all of these PRs for eligibility to approve and merge. For each PR, if all of the below conditions are satisfied, then approve and merge it:
- CI is green
- There are no merge conflicts
- The PR is ready for review (not in draft)
- There are no concerns or issues identified in comments (e.g. other reviewers, wiz, ai reviews, etc) that are relevant to the latest state of the branch
- I did not push most recently to the branch`;

/// The whole prompt the strip's batch Claudify starts Claude on (#1579).
///
/// The instruction, then who the viewer is -- "I did not push most
/// recently" means nothing without a login to compare against -- then
/// the SAME list "Copy as markdown" copies (#1578), in its agent form.
///
/// `viewer` must be a login that was READ. The caller disables the
/// button rather than calling this without one: a prompt that guessed
/// the viewer would let Claude clear the fifth condition wrongly.
export function readyBatchPrompt(rows: readonly ReadyRow[], viewer: string, now: Date): string {
  return [
    READY_BATCH_PROMPT,
    "",
    `I am @${viewer} on GitHub.`,
    "",
    readyListMarkdown(rows, { now, forAgent: true }),
  ].join("\n");
}
