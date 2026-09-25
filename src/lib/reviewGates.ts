import type { PrDetail, ReviewGates } from "../types/pr";

/// What the base branch's rules mean for THIS viewer on THIS pull request
/// (#1451, #1454). Every field is null when there is nothing to say, and
/// "nothing to say" is the answer whenever the rules could not be read:
/// "we could not ask" is not "no rule", so an unreadable lookup changes
/// nothing on screen.
export interface GateVerdict {
  /// The viewer's approval cannot count. Disables Approve with this reason.
  approveBlocked: string | null;
  /// The rule is on but who pushed last is unknown: qualify, do not assert.
  approveCaveat: string | null;
  /// Merge or enqueue is waiting on conversations. Replaces the generic
  /// "a required review or check is missing".
  mergeBlocked: string | null;
}

const NOTHING: GateVerdict = { approveBlocked: null, approveCaveat: null, mergeBlocked: null };

export const LAST_PUSH_BLOCKED =
  "You pushed the latest commit, so your approval won't count here.";

export const LAST_PUSH_UNKNOWN =
  "This branch needs the latest push approved by someone other than its pusher, and who pushed it could not be confirmed — your approval may not count.";

/// Threads that block a resolution requirement: every UNRESOLVED thread,
/// outdated ones included.
///
/// Deliberately wider than `PrDetail.unresolved_threads`, which excludes
/// outdated threads because it counts what needs the viewer's ATTENTION.
/// GitHub's requirement is about resolution, and an outdated thread is
/// still an unresolved conversation: the community request for GitHub to
/// auto-resolve outdated conversations exists precisely because they
/// keep blocking merge. So the gate counts them, and says how many of its
/// number are outdated so it never contradicts the header's count.
function blockingThreads(pr: PrDetail): { count: number; outdated: number } {
  const open = pr.review_threads.filter((t) => !t.is_resolved);
  return { count: open.length, outdated: open.filter((t) => t.is_outdated).length };
}

export function gateVerdict(
  gates: ReviewGates | undefined,
  pr: PrDetail,
  viewer: string | undefined,
  isPlaceholder: boolean,
): GateVerdict {
  // Pending (not arrived) and unreadable both render nothing new. Pending
  // needs no skeleton here: the absence of a notice is today's view, and
  // it is correct until a rule is actually read.
  if (!gates || gates.rules.state !== "read") return NOTHING;
  const rules = gates.rules;
  const out: GateVerdict = { ...NOTHING };

  // Your own pull request: GitHub refuses the approval outright, and
  // ReviewBox already says so. Nothing to add.
  const own = viewer !== undefined && viewer === pr.author;
  if (rules.require_last_push_approval && !own) {
    const p = gates.last_pusher;
    if (p.state === "known") {
      if (viewer !== undefined && p.login === viewer) out.approveBlocked = LAST_PUSH_BLOCKED;
    } else if (p.state === "unknown" || p.state === "declined") {
      out.approveCaveat = LAST_PUSH_UNKNOWN;
    }
  }

  // The seeded placeholder carries no threads, so it cannot say how many
  // are open -- and "0 conversations" would be invented.
  if (rules.required_review_thread_resolution && !isPlaceholder) {
    const { count, outdated } = blockingThreads(pr);
    // A truncated list (#802) makes the count a floor: qualify it. With
    // none open in the part we have, the rest is unknown, so suppress.
    const truncated = pr.review_threads_total > pr.review_threads.length;
    if (count > 0) {
      const n = `${truncated ? "at least " : ""}${count}`;
      const noun = count === 1 ? "conversation" : "conversations";
      const suffix = outdated > 0 ? ` (${outdated} outdated)` : "";
      out.mergeBlocked = `${n} ${noun} must be resolved first${suffix}`;
    }
  }
  return out;
}
