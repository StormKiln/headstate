import type {
  ClaudeSession, Lock, PullRequest, Safety, SubmoduleState, Upstream, Worktree, WorktreeRepo } from "@/types/pr";
import type { Filters } from "./derive";

/// Only `safe` may be deleted.
///
/// Everything else is genuinely disabled rather than warned past: a
/// cleanup tool that occasionally eats a day of work is worse than no
/// cleanup tool.
/// The `repo` filter value that selects orphaned worktrees.
///
/// A sentinel rather than a new axis on the store: the sidebar already
/// drives everything through `repo`, and adding a parallel "mode" would
/// mean every consumer had to learn about it. A path-shaped value that
/// no real repository can have keeps the existing plumbing honest.
export const ORPHAN_FILTER = "\0orphaned";

/// Whether a worktree's repository is gone.
///
/// Its own predicate rather than an inline comparison, so the several
/// places that need it cannot drift -- the same reason `isSafe` exists.
export function isOrphaned(s: Safety): boolean {
  return s.kind === "orphaned";
}

export function isSafe(s: Safety): boolean {
  // Mirrors `Safety::is_safe` in `src-tauri/src/worktrees/model.rs`, and
  // the two MUST agree: this one greys the button and that one is the
  // gate the command itself checks.
  //
  // Every kind here means the work is on the default branch and the tree
  // is clean, so all are removable (#732). They stay separate kinds so
  // the row can say which evidence was used -- `merged_upstream_deleted`
  // cannot be re-checked against a remote that no longer exists, and
  // `detached_merged` has no branch to re-check at all.
  //
  // `detached_merged` joined in #819 and is the one real widening, so the
  // argument: the evidence is the same `merged_into` bar `safe` clears --
  // ancestry or an exact patch-id match -- and what the row LACKS is a
  // branch, which is the thing removing a worktree would otherwise lose.
  // Its absence makes removal safer rather than riskier. Those rows were
  // `unknown` with no action offered, which is the dead end #819 reports.
  //
  // `prunable` stays out, deliberately and per #814: there is no
  // directory to remove, so remove is the wrong verb, and this allowlist
  // guards directory deletion. That row is cleared by the header's Prune
  // action instead -- #814 is a fix to how it READS, not to what it
  // permits.
  return (
    s.kind === "safe" ||
    s.kind === "merged_upstream_deleted" ||
    // #1439: the same `merged_into` evidence, on a branch that simply
    // has no tracking config. An unmerged one is still `never_pushed`.
    s.kind === "merged_no_upstream" ||
    s.kind === "detached_merged" ||
    // #1440: GitHub's record of a merged pull request that contains this
    // worktree's HEAD. Only ever an upgrade of a clean `unmerged` or
    // `unpushed` row, and the remove command re-asks GitHub rather than
    // trusting the scan.
    s.kind === "merged_as_pr"
  );
}

/// How long ago a lock was taken, as prose, or null if unknown.
///
/// Whole days, because that is the resolution the decision needs:
/// nobody unlocks differently for 5 days versus 5 days and 3 hours, and
/// a precise figure would imply a precision the source does not carry.
/// "today" rather than "0 days ago", which reads as a missing value.
///
/// Null when the age could not be read. A lock of unknown age is not a
/// fresh one, and the caller says nothing rather than guessing in the
/// direction that makes clearing it feel safer.
export function lockAge(lock: Lock): string | null {
  if (lock.age_days === null) return null;
  if (lock.age_days === 0) return "today";
  if (lock.age_days === 1) return "yesterday";
  return `${lock.age_days} days ago`;
}

/// One line describing a lock: when, by whom, and what is underneath.
///
/// Mirrors the Rust `Safety::reason` arm for `Locked`. Its own function
/// rather than inline in `safetyReason` because the confirmation needs
/// exactly this sentence too, and #753's `forceWarning` showed what two
/// copies of one warning cost -- two chances to drift on the wording
/// that decides whether somebody clears another process's claim.
///
/// The order is the argument of #775. Age leads because it is the only
/// part that differs per row and cannot be faked by a long-lived
/// parent. The reason follows because it is the locker's own words.
/// The merge state comes last because it is about the worktree rather
/// than about the lock -- and it is why the sentence is worth reading
/// before unlocking at all.
///
/// DELIBERATELY STILL LONG, and #818 asked whether it had to be.
///
/// That issue's row -- "locked today by claude agent agent-a0cc… (pid
/// 14779 start Fri Sep 11 09:43:48 2026) — merged, would be safe once
/// unlocked" -- broke the desktop layout, and it proposed keeping the
/// pid and start time for a tooltip so the row could read "locked today
/// by a claude agent". Tempting, and not done, for two reasons that are
/// about correctness rather than effort:
///
/// - The parenthesis is INSIDE `lock.reason`, which is git's free text
///   written by whoever took the lock (`git worktree lock --reason`).
///   Shortening it means pattern-matching a format this app does not
///   own and did not define. Our own agents happen to write
///   `(pid N start …)` today; a lock taken by a human, another tool, or
///   a future version of ours does not, and a regex that misfires would
///   either leave the string untouched (no fix) or eat a clause that
///   mattered (worse than the bug). The comments below and on
///   `Lock::reason` in the Rust model both turn on not rewriting the
///   locker's words; a trim is a rewrite with a friendlier name.
/// - The pid and start time are the EVIDENCE for the clause appended
///   right after them. "Holder process is gone" is a claim about a
///   specific process, and `holder_running` is computed by checking
///   exactly that pid against exactly that start time. Hiding the
///   identifiers behind a hover would leave the row asserting a
///   conclusion while its grounds needed a mouse.
///
/// So #818 is fixed in the LAYOUT instead -- the cell truncates and
/// carries the full sentence in its `title` -- which also fixes every
/// other unbounded verdict rather than just this one phrasing. The
/// length here is a real cost, paid knowingly: a row shows as much of
/// the sentence as it has room for, and the decisive part leads because
/// of the ordering argued above, so what gets clipped is the tail.
export function lockReason(lock: Lock): string {
  const age = lockAge(lock);
  let s = age === null ? "locked" : `locked ${age}`;
  s += lock.reason === null ? " — no reason given" : ` by ${lock.reason}`;
  // APPENDED after git's reason, never folded into it (#792).
  //
  // `holder_running` was computed on every scan from the beginning and
  // read by nothing but `lockHolderNote` in the unlock dialog -- so the
  // user learned the holder was dead only AFTER deciding to unlock and
  // opening the confirmation, which is the wrong end of the decision.
  // The row is where the decision is made.
  //
  // After the reason rather than replacing it, because the reason is
  // the locker's own words and rewriting them would be the app
  // inventing a claim on another process's behalf. So the row reads
  // "locked 2 days ago by claude agent … — holder process is gone":
  // git's sentence, then ours.
  if (lockHolderIsGone(lock)) s += " — holder process is gone";
  // The fact that turns unlocking from a leap into a decision. Only
  // said when it is true: silence is the honest default, and claiming
  // "would be safe" over unmerged work would invite exactly the blind
  // unlock this exists to prevent.
  if (isSafe(lock.underlying)) s += " — merged, would be safe once unlocked";
  return s;
}

/// Whether the process a lock names is PROVABLY gone.
///
/// Mirrors `Lock::holder_is_gone` in `src-tauri/src/worktrees/model.rs`.
/// Its own predicate, in both languages, because three places now turn
/// on this one fact -- the row's prose, the row's colour, and which rows
/// a bulk unlock may touch -- and `=== false` written out three times is
/// three chances to drift into `!== true` (#792).
///
/// That distinction is the whole point. `false` is the one decisive
/// signal available here: the reason named a process and it is not
/// running. `null` means the reason named no process to check, and it
/// must NOT read as "nothing holds it" -- most locks not written by our
/// own tooling land there, and treating an unasked question as a
/// negative answer is how a live claim gets cleared. `true` is weak
/// evidence in the other direction and is not spent as proof either;
/// see `lockHolderNote`.
export function lockHolderIsGone(lock: Lock): boolean {
  return lock.holder_running === false;
}

/// Whether this worktree is locked by a holder that is provably gone.
///
/// The selector the bulk unlock is built on (#792), and the reason it is
/// a function rather than an inline pair of comparisons: it is read by
/// the header's count, by the dialog's list, and by the button that
/// sends the batch, and those three MUST agree -- a count that includes
/// a row the batch then skips is a count the user cannot trust about an
/// action on another process's claim.
///
/// Deliberately narrower than "locked". A lock whose holder is running,
/// or whose reason named nobody to check, is excluded: the batch exists
/// for the case where the evidence is decisive, and a bulk action over
/// claims that might be live is precisely what #753 refused to offer.
/// Those rows keep the single-row unlock and its confirmation.
export function isDeadLock(wt: Worktree): boolean {
  return wt.safety.kind === "locked" && lockHolderIsGone(wt.safety.detail);
}

/// What checking the lock's holder actually established, or null when
/// there was nothing to check.
///
/// The LONG form, for the unlock confirmation. `lockReason` carries the
/// short one on the row -- four words, appended after git's reason, and
/// only for the decisive case. Both exist because the two places need
/// different lengths of the same fact: a row is a glance and a
/// confirmation is where the caveat has room to be read.
///
/// Deliberately NOT phrased as "the lock is live". A running holder is
/// weak evidence and the app must not launder it into a strong claim:
/// on the reporting machine this is true for all 20 locks and every one
/// of them is abandoned, because the pid belongs to the parent session
/// rather than to the worker that took the lock. So the "true" wording
/// says what was checked and immediately says what it does not prove.
///
/// The "false" wording is the opposite: a named process that is gone is
/// the one unambiguous signal available here, and it deserves to be
/// stated plainly. It says (pid, start_time) was checked rather than
/// just the pid, because that is what makes it sound: pids are recycled,
/// and a bare pid check would call a recycled number a live holder
/// (#792).
export function lockHolderNote(lock: Lock): string | null {
  if (lock.holder_running === null) return null;
  return lock.holder_running
    ? "The process it names is still running — though that is weak evidence, since a parent process outlives the work that took the lock."
    : // Says it checked the START TIME too, because that is what makes
      // this the decisive line rather than a guess (#792). Without it a
      // reader who knows pids are recycled has no reason to trust
      // "no longer running", and the whole point of this sentence is
      // that it can be trusted.
      "The process it names is no longer running — checked by its process id and the start time the lock recorded, so a reused id is not mistaken for it.";
}

/// Display-ready prose for a row.
///
/// Mirrors `Safety::reason` on the Rust side. Deliberately duplicated
/// rather than sent over the wire: the wire type is data, and prose in a
/// payload is harder to change than prose in a component.
/// Whether a worktree survives the Worktrees page's filters (#1140).
///
/// # Empty means everything
///
/// `undefined` and `[]` are both "no safety facet chosen". An empty
/// facet set is what a user gets by unticking the last box, and
/// rendering an empty list for it would read as "this repository has no
/// worktrees" -- the confident wrong answer #846 keeps having to
/// remove. Narrowing to nothing is not a question anyone asks.
///
/// # The main checkout is never filtered out
///
/// It is the repository, not a candidate for removal, and every count
/// and total on the page is stated relative to it. Hiding it behind a
/// facet would make the page describe a repository it is not showing.
///
/// # It is a PREDICATE, not a selection
///
/// This narrows what is displayed. What a bulk action destroys is the
/// user's explicit selection, which is a separate set -- see #1140's
/// safety rule: a filtered-away row must never be silently included,
/// and a filtered-IN row must never be silently acted on either.
export function matchesWorktreeFilters(
  w: Worktree,
  f: Pick<Filters, "safety" | "worktreeQuery" | "occupiedOnly">,
  isOccupied: (path: string) => boolean,
): boolean {
  if (w.is_main) return true;

  if (f.safety && f.safety.length > 0 && !f.safety.includes(w.safety.kind)) {
    return false;
  }

  if (f.occupiedOnly && !isOccupied(w.path)) return false;

  const q = f.worktreeQuery?.trim().toLowerCase();
  if (q) {
    // Path AND branch, because a user looking for a worktree knows one
    // or the other -- the directory name they created it under, or the
    // branch they have open in it -- and which of the two they reach
    // for is not something this can predict.
    const hay = `${w.path} ${w.branch}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }

  return true;
}

/// Whether any worktree filter is currently narrowing the list.
///
/// Used to decide whether a count needs qualifying: "9 safe to remove"
/// means something different when six others are hidden, and a bulk
/// action's dialog has to say so. Derived in ONE place so the page and
/// the dialog cannot disagree about whether a filter is on.
export function worktreeFiltersActive(
  f: Pick<Filters, "safety" | "worktreeQuery" | "occupiedOnly">,
): boolean {
  return Boolean(
    (f.safety && f.safety.length > 0) || f.worktreeQuery?.trim() || f.occupiedOnly,
  );
}

/// What a worktree's submodules add to the dirty count, or null.
///
/// A SEPARATE sentence rather than a change to `safetyReason`, and that
/// is deliberate. `safetyReason` takes a `Safety` and answers "can I
/// remove this"; the submodule state does not change that answer --
/// `Safety::Dirty` already refuses -- it changes what the user should
/// DO about it. Folding it in would also mean threading a second
/// argument through every caller of a function whose whole job is to
/// turn one value into one string.
///
/// Null when there is nothing to say. A worktree whose submodules are
/// clean and in sync is the ordinary case, and a line reading
/// "0 submodules with changes" on every row is noise.
export function submoduleNote(sub: SubmoduleState | null | undefined): string | null {
  if (!sub || (sub.dirty === 0 && sub.out_of_sync === 0)) return null;
  const parts: string[] = [];
  // Dirty first: it is the half that means work would be lost, and the
  // half the dirty count the user is already looking at came from.
  if (sub.dirty > 0) {
    parts.push(`${sub.dirty} submodule${sub.dirty === 1 ? "" : "s"} with uncommitted work`);
  }
  if (sub.out_of_sync > 0) {
    // "not at the recorded commit" rather than "out of date": the
    // submodule may be AHEAD, and calling that out of date would be
    // wrong in a way that sends the user to the wrong command.
    parts.push(`${sub.out_of_sync} not at the recorded commit`);
  }
  return parts.join(", ");
}

export function safetyReason(s: Safety): string {
  switch (s.kind) {
    case "safe":
      return "merged, pushed — safe to delete";
    case "main_checkout":
      return "the repository's main checkout";
    case "dirty":
      return `${s.detail} uncommitted file${s.detail === 1 ? "" : "s"}`;
    case "unpushed":
      return `${s.detail} unpushed commit${s.detail === 1 ? "" : "s"}`;
    case "never_pushed":
      return "never pushed — commits exist only here";
    case "merged_upstream_deleted":
      // Says both halves. "Merged" is why it is removable; "upstream
      // deleted" is why the row cannot point at a remote branch to
      // prove it, and is the fact a user comparing this row against
      // GitHub would otherwise find missing.
      return "merged; upstream deleted — safe to delete";
    case "merged_no_upstream":
      // MERGED FIRST (#1439). This row used to read "never pushed —
      // commits exist only here", which the merge check proves false.
      // "No upstream configured" says only what was observed: the branch
      // has no tracking config, not that nobody ever pushed it.
      return "merged; no upstream configured — safe to delete";
    case "detached_merged":
      // MERGED FIRST, then the detachment (#819).
      //
      // These rows used to read "could not determine: detached HEAD",
      // which led with a failure and named a missing branch. Both halves
      // were the wrong way round: the question is "can I clear this", the
      // answer is yes, and the detachment is the caveat.
      //
      // `detail` carries the `at <ref>` when one resolved, so the row
      // reads "merged — detached at v1.13.0~30, no branch to delete".
      // That identifies the checkout, which "detached HEAD" never did.
      //
      // Ends on "no branch to delete" because that is the reassurance the
      // user came for: the usual worry about removing a worktree is
      // losing the branch, and here there is none.
      return `merged — ${s.detail}, no branch to delete`;
    case "merged_as_pr":
      // Names the ROUTE (#1440): the number is GitHub's evidence, and
      // what the user can open to check. Plain "merged" would read like
      // the offline verdict, which on this row found nothing.
      return `merged as #${s.detail} on GitHub — safe to delete`;
    case "empty":
      // Says what is TRUE of the branch, not what the app will let you
      // do about it: the Remove button stays disabled, deliberately,
      // but the row must stop claiming commits that do not exist.
      return "no commits of its own — nothing to lose";
    case "unmerged":
      return "branch not merged";
    case "locked":
      // AGE FIRST, then the locker (#775).
      //
      // #753 led with the reason, on the theory that "some tool (pid
      // 123)" answers whether the claim is live. Measured once locks
      // accumulated, it does not: all 20 on the reporting machine name
      // one pid, alive only because it is the parent that outlived the
      // workers, so the reason reads as live evidence on every row
      // including every abandoned one.
      //
      // The age is what differs per row and what a stale lock cannot
      // fake, so a five-day-old lock now READS as five days old. The
      // reason still follows, unrewritten -- demoted, not dropped.
      return lockReason(s.detail);
    case "prunable":
      // THE SAFE PART FIRST, then the mechanism (#814).
      //
      // #753 gave this row "directory is gone — prunable (<reason>)",
      // which beat "could not determine: directory is missing" by naming
      // a remedy where the old wording named neither cause nor cure, and
      // #793 shipped that remedy as a button. What both left is the
      // READING: the row still opened with a loss and a piece of git
      // vocabulary, while the reassurance that nothing can be lost lived
      // only in a tooltip. The issue was filed twice, which is the signal
      // -- the page answered "can I clear this?" with a vocabulary lesson
      // instead of a yes.
      //
      // So the order is inverted. "Nothing to lose" is the answer, the
      // missing directory is why, and git's own reason stays in
      // parentheses because it is what git actually said.
      //
      // The VERB is still prune, not remove, and `isSafe` still excludes
      // this kind. That distinction is right -- there is no directory to
      // remove, and `git worktree prune` is repo-wide -- and #814 is
      // explicit that it is not asking for the allowlist to widen. It
      // asks that being correct stop reading as a warning.
      return `nothing to lose — its directory is already gone, prune to clear (${s.detail})`;
    case "pending":
      return "checking…";
    case "orphaned":
      // Says what IS known rather than hedging: the parent repository
      // is gone, so nothing about the contents can be established, and
      // the user needs that before deciding.
      return "its repository is gone — nothing here can be checked";
    case "in_progress": {
      // The row's own sentence (#1136). Names the operation, because
      // that is the fact that survives the remedy -- committing the
      // working tree does not end a rebase -- and it is what the user
      // has to resolve.
      //
      // A null count is omitted rather than rendered as 0: an
      // unreadable `git status` is not a conflict-free rebase.
      const { op, conflicts } = s.detail;
      const what = op === "cherryPick" ? "cherry-pick" : op;
      return conflicts && conflicts > 0
        ? `${what} in progress — ${conflicts} conflicted file${conflicts === 1 ? "" : "s"}`
        : `${what} in progress`;
    }
    case "unknown":
      return `could not determine: ${s.detail}`;
    default:
      // A kind this side does not know. Says so by NAME rather than
      // interpolating a payload it cannot read: that is how an
      // unmatched `in_progress` printed "[object Object]" (#1437).
      return `unrecognised state: ${unrecognisedKind(s)}`;
  }
}

/// The `kind` of a `Safety` no case matched, as printable text.
///
/// Typed `never` because the union is exhaustive at compile time; this
/// runs only when the backend sends a kind the frontend does not
/// declare, which is exactly the drift #1437 was.
function unrecognisedKind(s: never): string {
  const kind = (s as { kind?: unknown } | null)?.kind;
  return typeof kind === "string" ? kind : "unknown kind";
}

/// What the force-removal confirmation warns about, for one safety
/// state.
///
/// Four named cases, not two, because #701 and #798 each showed what a
/// missing one costs. `never_pushed` names the specific loss -- commits
/// that exist nowhere else. `dirty` names the count, because it can and
/// because the generic line was what a user with a 153-package lockfile
/// bump in the balance actually read (#798). `empty` has no loss to
/// name, so it says so plainly rather than inheriting a warning about
/// commits it does not have; that sentence is the whole reason this
/// state exists. Everything else gets the general form, which is honest
/// about the app's uncertainty without inventing a danger.
///
/// Every branch still ends in "this cannot be undone": the directory
/// goes either way, and the user is one click from it.
export function forceWarning(s: Safety): string {
  switch (s.kind) {
    case "never_pushed":
      return "These commits are not pushed anywhere. This cannot be undone.";
    case "dirty":
      // Names the NUMBER, which the app already knows and used to
      // throw away at exactly the moment it mattered (#798). This case
      // fell through to the generic "Headstate does not consider this
      // safe to remove", which describes the app's opinion rather than
      // the user's loss -- and the forced path could not even carry it
      // out, so the sentence the user read was wrong about the stakes
      // AND about the outcome. Now the stakes are specific and the
      // removal works, which is the pair that makes the confirmation
      // worth reading.
      //
      // "deleted permanently" rather than "lost": these are files on
      // disk that no git object holds a copy of, so there is no reflog
      // and no stash to recover them from.
      return `${s.detail} uncommitted file${s.detail === 1 ? "" : "s"} will be deleted permanently. This cannot be undone.`;
    case "in_progress": {
      // NAMES the operation, because that is what the user has to
      // resolve and the remedy differs per operation (#1136). The stakes
      // are specific for the same reason `dirty` above names its count:
      // a half-replayed rebase holds commits that exist in no other
      // ref, so there is no reflog entry to recover them from once the
      // directory is gone.
      const what =
        s.detail.op === "cherryPick" ? "cherry-pick" : s.detail.op;
      return `A ${what} is in progress here. Removing the worktree discards it, along with any commits it has replayed so far. This cannot be undone — finish or abort the ${what} first.`;
    }
    case "empty":
      return "This branch has no commits of its own, so nothing on it would be lost. Removing the directory cannot be undone.";
    case "locked":
      // Says the truth the general wording would hide: forcing here
      // does not work, and that is now a CHOICE rather than a defect.
      //
      // Until #798 the reason was that `remove_worktree_forced` never
      // passed git's `--force` at all. It does now -- and still only
      // once, where a locked worktree needs `--force --force`. That
      // second force is deliberately withheld: a lock is the only
      // signal another process has for saying it is working in a
      // directory, and on the reporting machine 13 of 34 worktrees
      // were locked by agents actively doing so. So git refuses, by
      // design, and the user would otherwise confirm a
      // destructive-sounding dialog and get an error.
      //
      // Naming the unlock is not an invitation to ignore the lock; it
      // is the route that makes the user read the claim first, and the
      // reason is quoted beside it (#753).
      return "This worktree is locked, and git will refuse to remove it until it is unlocked — check the lock reason above first, in case the process that set it is still running.";
    case "prunable":
      // There is no directory to remove, so the destructive framing is
      // simply wrong here. Nothing can be lost and nothing will be.
      //
      // UNREACHABLE from the UI as of #793, and kept anyway. The kebab
      // no longer offers removal on a prunable row -- that route ran
      // `git worktree remove` against a directory that is not there --
      // and `canClaudify` has always excluded the state, so no force
      // confirmation can open over one. `safetyReason` is what a
      // prunable row actually reads now, and the kebab points at the
      // header's Prune action.
      //
      // Retained because `forceWarning` is a total function over
      // `Safety` and the exhaustiveness is the safety property: if some
      // future path does reach a prunable row, the sentence it gets
      // should be this one and not the generic "Headstate does not
      // consider this safe to remove. This cannot be undone." -- which
      // would threaten a loss that cannot happen. Updated to name the
      // action that now exists rather than the command the user used to
      // have to retype in a terminal.
      return "This worktree's directory is already gone; only the stale registration remains. Nothing can be lost — use “Prune stale registrations” above the list, which runs `git worktree prune`.";
    default:
      return "Headstate does not consider this safe to remove. This cannot be undone.";
  }
}

/// The open pull request for a worktree, if there is one.
///
/// Keyed on repo AND branch, never branch alone: branch names are not
/// unique across repositories -- this account has `feat/egr33-*` in two
/// of them -- and repo identity comes from the git remote rather than
/// the directory name.
///
/// DISPLAY ONLY. This must never widen a safety gate: a wrong pairing
/// would attach GitHub's authoritative-looking "merged" to the wrong
/// directory, and deletion is the one unrecoverable action here.
export function prForWorktree(
  prs: PullRequest[],
  repoIdentity: string | null,
  branch: string,
): PullRequest | null {
  if (!repoIdentity || !branch) return null;
  return (
    prs.find((p) => p.repo === repoIdentity && p.head_ref === branch) ?? null
  );
}

/// How much disk the removable worktrees are holding (#1181).
///
/// # The join the app never made
///
/// Headstate's stated purpose is finding what stale build output is
/// costing you in disk. It already knew all three facts -- a worktree's
/// branch, that branch's merge state, and the worktree's size -- and
/// never put them together. On the machine that filed this, 23
/// worktrees held 164 GB of `src-tauri/target/`, 124 GB of it in
/// eighteen whose pull requests had already merged, and a build failed
/// with `No space left on device` at 130 MB free. Finding that took
/// `du` and `gh pr list` by hand, from the app whose job it is.
///
/// # It takes the ALREADY-GATED list
///
/// The caller passes the rows it has decided are removable, rather than
/// this re-deriving them from `isSafe`. The page's gate is not only
/// safety -- it also excludes worktrees a Claude session is working in
/// -- and a summary offering to reclaim space from rows the button then
/// refuses would send the user round a loop. #770's lesson: one gate,
/// so the two cannot disagree.
///
/// # Unmeasured is not zero
///
/// A worktree whose size could not be read contributes nothing to the
/// total, and its absence would understate the figure silently (#846,
/// #1044). `unmeasured` is what lets the caller say "at least" -- the
/// same qualification `rollupRepos` already applies.
export function reclaimable(removableRows: Worktree[]): {
  /// Their total size, over the ones that were measured.
  bytes: number;
  /// How many have no size, so `bytes` is a floor rather than a total.
  unmeasured: number;
} {
  let bytes = 0;
  let unmeasured = 0;
  for (const w of removableRows) {
    if (w.size_bytes === null || w.size_bytes === undefined) unmeasured += 1;
    else bytes += w.size_bytes;
  }
  return { bytes, unmeasured };
}

/// Whether this row can be handed to a coding agent.
///
/// Every state that is NOT removable and not the main checkout. That is
/// 124 of 268 worktrees on a real machine, and the largest group --
/// never-pushed, at 52 -- is the one where the question matters most,
/// because those commits exist nowhere else.
///
/// `pending` is excluded on purpose: offering an action based on a
/// safety verdict that has not arrived is the bug #190 was.
///
/// `locked` and `prunable` are excluded too, and they are the states
/// where the blanket rule "not removable and not the main checkout"
/// stops fitting (#753). The action copies a prompt asking a coding
/// agent to assess the worktree's CONTENT, and neither of these is a
/// question about content:
///
/// - `prunable` has no directory left. There is nothing on disk for an
///   agent to open, so the prompt would send it somewhere that does
///   not exist.
/// - `locked` is a claim by another process -- very often an agent
///   already working in that tree, which is precisely how the
///   reporting machine acquired 13 of them. Pointing a second agent at
///   a directory the first has locked is the one thing the lock exists
///   to prevent. The lock is also not a property of the branch, so
///   once it is cleared the row reports its real state and the action
///   returns on its own.
export function canClaudify(s: Safety): boolean {
  return (
    s.kind === "unmerged" ||
    s.kind === "never_pushed" ||
    s.kind === "unpushed" ||
    s.kind === "dirty" ||
    // Included, because the rule above is "not removable and not the
    // main checkout" and `empty` is both. It is a weaker case than the
    // others -- there is nothing on the branch to assess -- but the
    // gate keeps `empty` un-removable, so withholding the assess action
    // too would leave the row with no action at all, which is precisely
    // the dead end this list exists to avoid.
    s.kind === "empty"
  );
  // `detached_merged` is deliberately NOT here, and that is not an
  // omission (#819).
  //
  // The rule is "not removable and not the main checkout", and since
  // #819 these rows ARE removable -- `isSafe` includes them, so the row
  // gets the primary Remove button and the kebab's confirmed path. The
  // issue asks for "at least one real action", and removal is the one the
  // evidence supports; adding Claudify beside it would ask an agent to
  // assess content that is already on the default branch.
  //
  // An UNMERGED detached row is a different case and is still not here:
  // it reports `unknown`, which this function excludes, so it keeps the
  // dead end the issue describes for the rows where the merge question
  // could not be answered. Enabling it would need `assess()` to work
  // without a branch name -- it builds `{base}..{branch}` and
  // `log -1 -- {branch}`, both of which take an empty right-hand side for
  // a detached checkout -- which is a change in `assess.rs` rather than
  // here. Left as a follow-up rather than half-wired: a Claudify button
  // that copies a prompt naming no branch is the closed loop moved, not
  // broken.
}

/// The final component of a path, whichever separator it uses.
///
/// `split("/")` returns the whole string unchanged on a Windows path, so
/// every row would show `C:\Users\me\code\proj-feature` instead of
/// `proj-feature`. Splitting on both separators is enough here: a
/// directory name cannot itself contain either one.
export function pathBasename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/// Whether a row is still waiting on its safety check.
///
/// The fast listing lands in ~2.6s; classification follows it. The figure
/// that used to be here -- "takes up to ~57s" -- was a TOTAL for one tree
/// on one machine, and quoting a total is how #830 went unnoticed: a
/// total cannot distinguish "many worktrees, each quick" from "one
/// worktree that never answers", which are the same number and completely
/// different bugs.
///
/// MEASURED per worktree instead, serial, by
/// `live_classification_cost_per_worktree`:
///
/// ```text
///   repo          worktrees   p50     p90      max
///   ghstat           18      140ms    576ms    724ms
///   enc-api          15      114ms   3139ms   3295ms
/// ```
///
/// The spread is what matters: within ONE repository the slowest worktree
/// costs 29x the median, because the cost tracks CHANGED FILES rather
/// than worktree count. So "how long until this row resolves" has no
/// single answer, and a row must say "still working" only while that is
/// true of IT -- which is why verdicts now stream per worktree and a row
/// that cannot be classified says so (`Safety.kind === "unknown"`)
/// instead of holding this skeleton indefinitely.
export function isPending(s: Safety): boolean {
  return s.kind === "pending";
}

/// Display-ready prose for a checkout's upstream state.
///
/// Mirrors `Upstream::reason` on the Rust side, for the same reason
/// `safetyReason` does.
///
/// Says "as of last fetch" on the states where staleness changes the
/// meaning: "behind 40" invites a pull, and the user should know the
/// number could itself be stale. "Up to date" carries the same caveat
/// and is the one most likely to mislead.
function upstreamReason(u: Upstream): string {
  const commits = (n: number) => `${n} commit${n === 1 ? "" : "s"}`;
  switch (u.kind) {
    case "current":
      return "up to date with upstream (as of last fetch)";
    case "ahead":
      return `${commits(u.n)} ahead of upstream`;
    case "behind":
      return `${commits(u.n)} behind upstream (as of last fetch)`;
    case "diverged":
      return `diverged — ${commits(u.n[0])} ahead, ${commits(u.n[1])} behind`;
    case "untracked":
      return "no upstream — local only";
    case "detached":
      return "detached HEAD";
    default:
      return `upstream unknown: ${u.n}`;
  }
}

/// A compact ahead/behind for a dense worktree row.
///
/// The long form ("3 commits behind upstream (as of last fetch)") is
/// right for the main checkout's own line, where it is the only thing
/// said. A row already carries name, branch, safety, date, and size, so
/// this is the arrow notation git users already read.
///
/// Returns null when there is nothing worth saying: an up-to-date branch
/// adds noise, not information.
export function upstreamShort(u: Upstream): string | null {
  switch (u.kind) {
    case "ahead":
      return `↑${u.n}`;
    case "behind":
      return `↓${u.n}`;
    case "diverged":
      return `↑${u.n[0]} ↓${u.n[1]}`;
    // Local-only is worth saying: it is the difference between "this is
    // redundant" and "this is the only copy".
    case "untracked":
      return "local only";
    default:
      return null;
  }
}

/// Tailwind colour for an upstream state.
///
/// Green only for genuinely current. Amber where action might be wanted,
/// grey where the question simply does not apply -- a local-only repo is
/// not a problem, it is a choice.
export function upstreamTone(u: Upstream): string {
  switch (u.kind) {
    case "current":
      return "text-[#3fb950]";
    case "behind":
    case "diverged":
      return "text-[#d29922]";
    default:
      return "text-[#8b949e]";
  }
}

/// Tailwind colour for a safety state.
///
/// Green ONLY for safe. Amber for states that hold work you might still
/// want; grey for the main checkout, which is not a problem at all.
export function safetyTone(s: Safety): string {
  switch (s.kind) {
    // Green for every merged state: the button is enabled for each, and
    // a different colour would imply a different degree of safety rather
    // than a different route to the same verdict (#732, and
    // `detached_merged` on the same argument in #819 -- green on this
    // page means one-click removable, and every one is in `isSafe`; #1439
    // added `merged_no_upstream` on the same argument).
    case "safe":
    case "merged_upstream_deleted":
    case "merged_no_upstream":
    case "detached_merged":
    case "merged_as_pr":
      return "text-[#3fb950]";
    case "main_checkout":
      return "text-[#8b949e]";
    case "pending":
      return "text-[#8b949e]";
    case "never_pushed":
      return "text-[#f85149]";
    // RED, like `never_pushed` and for the same reason (#1136): a
    // half-replayed rebase holds commits no other ref points at, so
    // removing the worktree loses work with no reflog to recover it
    // from. Amber would read as "needs attention"; this is "stop".
    case "in_progress":
      return "text-[#f85149]";
    case "empty":
      // Grey, and explicitly so rather than by falling through to the
      // default. This is the one colour on the row that separates
      // "nothing here" from the red beside it -- an empty branch put in
      // red would repeat #701's mistake in a different medium, telling
      // the user work is at risk when there is no work. Not green
      // either: green means one-click removable, and it is not.
      return "text-[#8b949e]";
    case "dirty":
    case "unpushed":
      return "text-[#d29922]";
    case "locked":
      // A dead holder is GREY, and the rest of the locks stay amber
      // (#792).
      //
      // Amber on this page means "you may want to act on this, and
      // there is a judgement to make". That is exactly right for a lock
      // whose holder might be working in the directory, and exactly
      // wrong for one whose named process is provably gone: there is no
      // judgement left, only a leftover file. Colouring both the same
      // is what made five stale locks on the reporting machine read as
      // live claims and the Remove button look blocked for a reason.
      //
      // Grey is the shade `prunable` already uses for the other state
      // that is pure stale bookkeeping, which is the comparison worth
      // making -- not a new colour, because a sixth shade on this row
      // would have to be learned where this one is already understood.
      //
      // Not green, though the row is now easy to clear: green on this
      // page means one-click removable, and a dead-holder lock is not.
      // It takes an unlock first, and `is_safe` deliberately still
      // excludes it.
      return lockHolderIsGone(s.detail) ? "text-[#8b949e]" : "text-[#d29922]";
    case "prunable":
      // Grey. There is no directory left, so there is nothing at risk
      // and nothing to reclaim -- it is a bookkeeping entry, and amber
      // would ask for attention that the row does not deserve.
      return "text-[#8b949e]";
    case "orphaned":
      // Amber, not red: an orphan is not dangerous, it is
      // UNVERIFIABLE. Red would put it beside "commits exist only
      // here" and imply work is at risk; grey would let 2.5 GB of
      // unreachable directories read as ordinary.
      return "text-[#d29922]";
    default:
      return "text-[#8b949e]";
  }
}

/// Bytes as a short human string. `null` renders as an em dash rather
/// than "0 B", which would claim a measurement that has not happened.
export function formatSize(bytes: number | null): string {
  if (bytes === null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  return `${n < 10 && i > 0 ? n.toFixed(1) : Math.round(n)} ${units[i]}`;
}

/// What a worktree's pull request says is wrong, or null if nothing is.
///
/// `prForWorktree` already resolves the pull request for a row, but the
/// row showed only its NUMBER -- while the `PullRequest` in hand carries
/// `ci`, `merge`, and `review`. That is usually the reason to come back
/// to a worktree at all: its pull request broke.
///
/// Ordered by loudness, and returns only ONE: a row is a glance, not a
/// report, and three stacked badges would push the path off the line.
/// Red CI wins over conflicts because it is the more surprising of the
/// two -- a conflict is a consequence of other work landing, a red build
/// is a consequence of yours.
///
/// Silent on a healthy pull request, and silent while CI is still
/// RUNNING: pending is an unfinished answer, not a problem, and a badge
/// on every row is noise. The number stays a link for anyone who wants
/// the detail. No new fetching: every field here is already on the
/// PullRequest this function is handed.
export function worktreeSignal(
  // `undefined` as well as `null`: the prop is optional on the row, and
  // both mean the same thing here -- no pull request, nothing to say.
  pr: PullRequest | null | undefined,
): { label: string; className: string } | null {
  if (!pr) return null;
  if (pr.ci === "failure") return { label: "CI failing", className: "text-[#f85149]" };
  if (pr.merge === "conflicted") return { label: "Conflicts", className: "text-[#f85149]" };
  if (pr.review === "changes_requested") {
    return { label: "Changes requested", className: "text-[#d29922]" };
  }
  return null;
}

/// The total size of a set of worktrees, distinguishing "not measured
/// yet" from "measured, and it is zero".
///
/// The bug this replaces summed with `?? 0` and then wrote `|| null`:
///
///     .reduce((n, w) => n + (w.size_bytes ?? 0), 0) || null
///
/// `0` is falsy, so a genuine zero became null and rendered as `—`.
/// Worse, an unmeasured set also summed to 0 and rendered the same
/// way, so "still measuring" and "nothing to reclaim" were one symbol.
/// Sizing is the slow half of the scan, which is why the dash appeared
/// only sometimes -- it depended on whether sizing had caught up.
///
/// Returns null ONLY when nothing in the set has a known size.
export function totalSize(items: { size_bytes: number | null }[]): number | null {
  const measured = items.filter((w) => w.size_bytes !== null);
  if (measured.length === 0) return null;
  return measured.reduce((n, w) => n + (w.size_bytes ?? 0), 0);
}

/// How old the answers on a repository's rows are, as prose, or `null`
/// when they are fresh enough not to mention.
///
/// Every merge and upstream verdict is computed against `origin/*` refs
/// already on disk -- the scan never goes to the network, deliberately,
/// because fetching every remote would turn a one-second view into a
/// thirty-second one. What was missing is saying so: on one machine a
/// repository's refs were 12 days old while its rows read like the
/// present tense (#702).
///
/// Silent only below an HOUR (#815). It used to be silent below a DAY,
/// which made this read as reassurance in exactly the window where the
/// bug lives: on a repository whose `main` lands PRs hourly, refs fetched
/// 23 hours ago are stale enough to make a merged branch look unmerged,
/// and the page said nothing at all. #815 reports that loop happening
/// "many times" -- an agent assessed stale refs, did the work, and found
/// it already upstream at PR time.
///
/// An hour rather than "any non-zero age", which #815 floats as the
/// better option. Rejected: a fetch 90 seconds ago is genuinely current,
/// and a caveat on every row of every repository all the time is a
/// caveat nobody reads -- the failure the old day-long threshold was
/// guarding against, and the reason there is still a threshold at all.
/// An hour is the shortest window in which this repository's `main` can
/// actually move, which is the quantity being estimated.
///
/// Hours up to a day, then days. Not "2026-09-11T08:00:00Z": the reader
/// is deciding whether to trust a verdict on screen right now, and a
/// duration answers that where an instant makes them do the arithmetic.
/// (The Claudify PROMPT makes the opposite choice and prints the absolute
/// instant, because it may be pasted a day after it was copied, at which
/// point a duration computed at copy time would be a lie.)
///
/// `null` fetch time is NOT silent: never fetched is the stalest state
/// there is, not the freshest.
///
/// THE FETCH BUTTON NOW EXISTS (#788), which closes what this comment
/// used to defer. #815 asked for one as item 3 and was right; it was left
/// out then because `WorktreesPage.tsx` was owned by a concurrent change
/// and because there was no production fetch anywhere in the app -- the
/// six in `scan.rs` were all test fixtures -- so it needed a Rust
/// command, allowlist entries, a mutation hook and a re-scan. #788 did
/// all of that: `fetch_refs`, `Class::Write` in both surfaces, and
/// `useFetchRefs`. So this function's caveat is no longer a dead end the
/// reader can do nothing about.
///
/// #788 also moved the age ONTO THE ROW, as `refAgeShort` and
/// `upstreamReasonAged`. This function is still the PAGE HEADER's form
/// and is not superseded: it qualifies every row including the merge
/// verdicts, which have no per-row note of their own and are what #702
/// and #815 were actually filed about. The row's note qualifies the one
/// claim -- "up to date with upstream" -- that a reader takes at face
/// value. Both read this same threshold, deliberately, so the row and the
/// header cannot disagree about whether a repository is stale at all.
export function refAge(fetchedAt: string | null, now = new Date()): string | null {
  if (fetchedAt === null) return "never fetched";
  const at = Date.parse(fetchedAt);
  if (Number.isNaN(at)) return "never fetched";
  const hours = Math.floor((now.getTime() - at) / 3_600_000);
  if (hours < 1) return null;
  // A future timestamp -- a clock that jumped, or an `mtime` ahead of
  // now -- gives a negative `hours` and is caught by the `< 1` return
  // above, so it goes SILENT rather than rendering "-3 hours ago". Silent
  // is the right failure here: "we cannot tell how stale this is" is not
  // a claim of staleness, and the one state that must never go silent
  // (never fetched) is handled before any arithmetic runs.
  if (hours < 24) return `as of a fetch ${hours} hour${hours === 1 ? "" : "s"} ago`;
  const days = Math.floor(hours / 24);
  return `as of a fetch ${days} day${days === 1 ? "" : "s"} ago`;
}

/// How much a row's upstream claim is resting on refs nobody has
/// checked, as ONE decision the text and the colour both read (#788).
///
/// `refAge` already says the age in prose and already says it in hours
/// (#815 lowered the threshold from a calendar day, which is the defect
/// #788 describes but which was fixed before #788 was filed -- see the
/// threshold note below). What #788 adds is that the age belongs on the
/// ROW, beside the claim it qualifies, and that the claim should not
/// stay the same colour either way.
///
/// ### Why a discriminated result rather than two functions
///
/// The bug #788 reports is two mechanisms disagreeing: a green badge
/// saying "up to date with upstream" and a header note, 1400 lines away
/// in the markup and silent most of the time, saying the refs are two
/// days old. Splitting the answer into `rowAgeText()` and `rowAgeTone()`
/// would rebuild that disagreement inside one file -- two thresholds to
/// keep in step, and nothing failing when they drift. One value, read
/// twice, cannot drift.
///
/// ### The three states, and why `unknown` is not `stale`
///
/// - `fresh` -- fetched within the hour. Nothing to say; the green
///   badge means what it looks like.
/// - `stale` -- fetched, and we know how long ago. Carries the age, so
///   the row can print a number rather than a hedge.
/// - `unknown` -- never fetched, or a timestamp that will not parse.
///
/// `unknown` is separate from `stale` because it is a different claim
/// and the page must not flatten it into one. This codebase's
/// characteristic bug is absent-read-as-success: #769's sizes summed a
/// never-measured tree to `0` bytes and rendered it as "empty", #841
/// read a missing health sample as healthy. The same mistake here would
/// be treating a null `fetched_at` as age zero -- "up to date · as of 0h
/// ago" on a repository that has NEVER contacted its remote, which is
/// the single most wrong sentence this page could print. It is also not
/// `fresh`: silence on a repository whose refs have no known provenance
/// is the reassurance with the least behind it.
///
/// ### The threshold is `refAge`'s, not a new one
///
/// An hour, inherited by CALLING `refAge` rather than re-deriving it.
/// #815 chose that figure against a measurement -- an hour is the
/// shortest window in which this repository's `main` can actually move --
/// and #788 asks for "near where the claim starts being wrong", which is
/// the same quantity. A second threshold here would mean the row and the
/// header could disagree about whether a repository is stale at all,
/// which is the class of bug being fixed, not a refinement of it.
///
/// So #788's "lower the threshold" is already done. Its diagnosis quotes
/// a `days < 1` early return that no longer exists; what remains of the
/// complaint, and what this serves, is placement and tone.
type RefStaleness =
  | { kind: "fresh" }
  /// `age` is `refAge`'s prose, e.g. "as of a fetch 4 hours ago".
  /// `hours` is the same quantity as a number, so a caller wanting a
  /// different wording does not have to re-parse the timestamp (and so
  /// cannot disagree with the prose about which side of a boundary it
  /// falls on).
  | { kind: "stale"; age: string; hours: number }
  | { kind: "unknown" };

function refStaleness(fetchedAt: string | null, now = new Date()): RefStaleness {
  // Checked BEFORE the arithmetic, exactly as `refAge` does, so that a
  // null or unparseable timestamp can never reach a subtraction and come
  // back as a duration.
  const age = refAge(fetchedAt, now);
  if (age === null) return { kind: "fresh" };
  // `refAge` returns this literal for both a null time and one that will
  // not parse, which are the two "we do not know" cases. Compared as a
  // string because `refAge` is the single definition of what counts as
  // unknown -- duplicating its null-and-NaN checks here is how the two
  // would later disagree about a malformed timestamp.
  if (age === "never fetched") return { kind: "unknown" };
  // Reaching here means `refAge` already parsed `fetchedAt` successfully
  // and found it at least an hour old, so this subtraction is safe
  // without a re-check and `fetchedAt` is necessarily a string. The
  // arithmetic duplicates `refAge`'s, which is the cost of not widening
  // that function's return type -- but it is bounded here, in the one
  // place, rather than at each caller that wants a number.
  const hours = Math.floor((now.getTime() - Date.parse(fetchedAt as string)) / 3_600_000);
  return { kind: "stale", age, hours };
}

/// The row-sized version of `refAge`'s prose (#788).
///
/// "as of a fetch 4 hours ago" is right for the page header, where it is
/// the only thing on the line. A worktree row already carries a name, a
/// branch, a safety sentence, an upstream verdict, a PR number, a signal,
/// a date and a size, and on the desktop that line TRUNCATES -- the
/// verdict cell is deliberately the first to clip (#818). So the note
/// that qualifies the verdict has to be short enough to survive beside
/// it, or the fix ships as a tooltip nobody opens.
///
/// `as of 4h ago`, then. "as of" is kept because it is the word that does
/// the work -- it marks the number as the age of the EVIDENCE rather than
/// the age of the branch, which the same row already shows two cells away
/// via `relativeTime`. Dropping it to `4h ago` would put two unlabelled
/// durations on one line meaning different things.
///
/// `never fetched` is not abbreviated. It is three words rather than a
/// number because it is a different KIND of statement, and shortening it
/// to something like "age unknown" would make the stalest state on the
/// page read as a minor gap in our bookkeeping.
///
/// Returns null when the refs are fresh, so a caller can render nothing
/// at all rather than a reassurance on every row of every repository --
/// the failure #815's threshold note is explicit about.
function refAgeShort(fetchedAt: string | null, now = new Date()): string | null {
  const s = refStaleness(fetchedAt, now);
  if (s.kind === "fresh") return null;
  if (s.kind === "unknown") return "never fetched";
  // `s.hours`, not a regex over `s.age`. Parsing "as of a fetch 4 hours
  // ago" back into a number would make this a second reader of `refAge`'s
  // WORDING, so rewording the header note would silently change the row's
  // number rather than failing a test. The narrowed `stale` variant
  // carries the quantity for exactly this reason.
  return s.hours < 24
    ? `as of ${s.hours}h ago`
    : `as of ${Math.floor(s.hours / 24)}d ago`;
}

/// Tailwind colour for an upstream state, given how old the refs behind
/// it are (#788).
///
/// `upstreamTone` colours the VERDICT. This colours the verdict together
/// with its provenance, which is a different question for exactly one
/// state: `current`.
///
/// Green on this page means "verified current". Until #788 it also meant
/// "agreed with a ref nobody has checked for two days", and those are
/// opposite claims wearing one colour -- the green dot actively argued
/// against the "(as of last fetch)" parenthetical beside it, and the
/// parenthetical lost. A user read green, clicked Update, and got a large
/// number of commits.
///
/// So a `current` verdict resting on stale or unknown refs goes GREY, and
/// only grey. Not amber: amber on this page means "you may want to act on
/// this" (`behind`, `diverged`, a live lock), and an unverified up-to-date
/// is not a call to action -- it may well be perfectly current, and
/// painting it as a problem on every repository anyone has not fetched
/// this hour is how a caveat stops being read. Grey is already this
/// page's shade for "this is bookkeeping, not a verdict about your work"
/// (`untracked`, `prunable`, a dead lock), which is precisely what an
/// unverified comparison is.
///
/// Every OTHER state keeps its own colour whatever the age. A `behind 40`
/// is still amber when the refs are old -- if anything the real number is
/// larger, so the call to action holds -- and greying it would hide a
/// claim that staleness cannot falsify, only understate. `ahead`,
/// `untracked` and `detached` do not read `origin/*` for their verdict at
/// all, so the fetch age is not evidence about them.
export function upstreamToneAged(u: Upstream, fetchedAt: string | null, now = new Date()): string {
  if (u.kind === "current" && refStaleness(fetchedAt, now).kind !== "fresh") {
    return "text-[#8b949e]";
  }
  return upstreamTone(u);
}

/// Display-ready prose for a checkout's upstream state, with the age of
/// the refs it was computed from (#788).
///
/// `upstreamReason` alone says "up to date with upstream (as of last
/// fetch)". The qualifier was already there and still did not land,
/// because it has no MAGNITUDE: "as of last fetch" reads identically
/// whether the fetch was 30 seconds or 30 days ago, and a reader who
/// cannot tell those apart reasonably treats it as boilerplate. This
/// replaces the constant phrase with the number.
///
/// `up to date with upstream · as of 4h ago`, and the parenthetical is
/// DROPPED when the number appears -- saying both would be "up to date
/// with upstream (as of last fetch) · as of 4h ago", which is the same
/// caveat twice and pushes the one that carries information off the end
/// of a cell that truncates.
///
/// Below the threshold the parenthetical is dropped too, and nothing
/// replaces it: refs fetched inside the hour make "up to date" simply
/// true, and that is the one case where a hedge costs more than it
/// pays. `never fetched` is the loud case and reads
/// `up to date with upstream · never fetched`, which is deliberately a
/// jarring sentence -- it is a jarring situation.
///
/// `behind` keeps its own "(as of last fetch)" via `upstreamReason` and
/// gains the number beside it, because staleness means something
/// different there: "40 behind" can only be an UNDERSTATEMENT when the
/// refs are old, so the caveat is about the size of a real problem
/// rather than about whether there is one.
export function upstreamReasonAged(u: Upstream, fetchedAt: string | null, now = new Date()): string {
  // `current` loses its "(as of last fetch)" in BOTH branches: with a
  // number beside it the parenthetical is the same caveat twice, and
  // without one the refs are genuinely current and there is nothing to
  // hedge. Every other state keeps `upstreamReason`'s wording verbatim --
  // `behind`'s own parenthetical included, because staleness means
  // something different there (it can only make "40 behind" an
  // understatement, so the hedge is about the size of a real problem
  // rather than whether there is one).
  //
  // `upstreamReason` is no longer exported and this is now its only
  // caller, which is why the distinction lives here rather than being
  // pushed into it: the two phrasings differ by whether a `fetched_at` is
  // in hand, and only this function has one.
  const base = u.kind === "current" ? "up to date with upstream" : upstreamReason(u);
  const short = refAgeShort(fetchedAt, now);
  return short === null ? base : `${base} · ${short}`;
}

/// The orderings the worktree list offers (#771).
///
/// Three axes because three are already on every row -- name, size and
/// age -- and no new data is needed for any of them. The page exists to
/// answer "which of these is biggest" on a repository with 100+
/// worktrees, and until now that question could only be answered by
/// reading every row.
///
/// Each axis is bidirectional. Size descending is what reclaims disk;
/// size ascending finds the scratch trees worth clearing wholesale. Age
/// descending surfaces the safe wins -- a worktree last touched four
/// months ago is a far easier delete than one from this morning,
/// whatever the safety verdict says.
export type WorktreeSort =
  | "name-asc"
  | "name-desc"
  | "size-desc"
  | "size-asc"
  | "age-desc"
  | "age-asc";

/// What the sort control offers, in the order it offers it.
///
/// Prose rather than a bare column name, the same choice `ArtifactsPage`
/// made with "Least recently written": "oldest" invites reading the age
/// as a creation date, when what is actually ordered is the last commit.
///
/// Size leads because it is the question the view exists for.
export const WORKTREE_SORT_LABELS: Record<WorktreeSort, string> = {
  "size-desc": "Largest first",
  "size-asc": "Smallest first",
  "age-desc": "Least recently committed",
  "age-asc": "Most recently committed",
  "name-asc": "Name (A–Z)",
  "name-desc": "Name (Z–A)",
};

/// Order one repository's worktree rows.
///
/// Two invariants survive every sort, because neither is a preference:
///
/// The MAIN CHECKOUT is always first. It is not a peer of the rows below
/// it -- every one of those is a removal candidate and it never is, so
/// sorting it among them invites reading it as one. Its row also carries
/// the upstream prose ("behind by 40"), which is the reason the
/// worktrees under it are stale, and an explanation belongs above the
/// thing it explains.
///
/// ASSESSED rows come next. The user has just come back from reading a
/// verdict, and finding that row again among 124 candidates is the part
/// that made this flow feel unfinished. A size sort must not bury the
/// one row they were mid-decision on.
///
/// UNKNOWN VALUES SORT LAST, in both directions, and that is the whole
/// reason this comparator is not a one-liner. Sizes arrive
/// asynchronously (#754, #758) and can be genuinely absent -- still
/// pending, or measured and failed. Treating a missing size as zero
/// would rank every unmeasured row as the smallest on the page, which
/// under "Largest first" hides exactly the directories that might be
/// huge. That is the ordering bug #360 describes, and putting unknowns
/// last is the answer `ArtifactsPage` already reached.
///
/// Last means last in BOTH directions. Under "Smallest first" an
/// unknown is still not a claim of 0 B, so it does not get to lead
/// either -- an unknown is absent from the ordering, not an extreme of
/// it. Ties, including ties among unknowns, fall back to the path, so
/// the result is stable rather than arbitrary.
///
/// Name never has this problem: a worktree always has a path. That
/// makes a name sort the one ordering fully known at first render, and
/// a genuine escape hatch while sizes are still landing.
export function sortWorktrees<T extends Worktree>(
  worktrees: T[],
  sort: WorktreeSort,
  assessed: ReadonlySet<string> = new Set(),
): T[] {
  /// The value this row offers on the chosen axis, or null when it
  /// cannot answer. Age is the last commit as epoch milliseconds; an
  /// absent or unparseable timestamp is null rather than 0, which would
  /// date the row to 1970 and pin it to one end of the list.
  const key = (w: T): number | null => {
    if (sort === "size-asc" || sort === "size-desc") return w.size_bytes ?? null;
    if (sort === "age-asc" || sort === "age-desc") {
      if (!w.last_commit) return null;
      const at = Date.parse(w.last_commit);
      return Number.isNaN(at) ? null : at;
    }
    return null;
  };
  // A larger epoch is MORE recent, so "least recently committed" wants
  // the SMALLEST number first -- the inverse of size, where "largest
  // first" wants the biggest. Spelled out rather than folded into the
  // comparator: getting it backwards yields a list that looks plausibly
  // sorted and is exactly wrong.
  const biggestFirst = sort === "size-desc" || sort === "age-asc";

  return [...worktrees].sort((a, b) => {
    if (a.is_main !== b.is_main) return a.is_main ? -1 : 1;
    const aa = assessed.has(a.path) ? 0 : 1;
    const bb = assessed.has(b.path) ? 0 : 1;
    if (aa !== bb) return aa - bb;

    if (sort === "name-asc" || sort === "name-desc") {
      // The basename, not the whole path: it is what the row shows, and
      // ordering by a prefix every row in a repository shares would be
      // ordering by nothing.
      const cmp = pathBasename(a.path).localeCompare(pathBasename(b.path));
      return sort === "name-asc" ? cmp : -cmp;
    }

    const va = key(a);
    const vb = key(b);
    if (va === null && vb === null) return a.path.localeCompare(b.path);
    if (va === null) return 1;
    if (vb === null) return -1;
    if (va === vb) return a.path.localeCompare(b.path);
    return biggestFirst ? vb - va : va - vb;
  });
}

/// A Claude Code session's recorded directory, matched against a known
/// worktree (#920).
///
/// `null` from [`sessionWorktree`] means no match, and the caller MUST
/// then offer no jump: a button that navigates to a list where the thing
/// is absent is worse than no button.
export interface SessionWorktree {
  /// The repository holding it. `setFilter("repo", repoPath)` is what
  /// selects a repository on the Worktrees page, so this is the
  /// navigation target.
  repoPath: string;
  /// The repository's display name, for the button's label.
  repoName: string;
  /// The matched worktree itself, so the caller can show the state that
  /// actually answers "what was this session doing" -- whether the branch
  /// merged, whether there is uncommitted work, whether it is safe to
  /// remove.
  worktree: Worktree;
  /// The branch the session recorded, when it differs from the branch the
  /// worktree is on NOW. `null` when they agree, or when either is
  /// unknown.
  ///
  /// # Why this is reported and not matched on
  ///
  /// MEASURED over the real corpus: of 206 sessions whose cwd matches a
  /// registered worktree, the recorded branch **disagrees with the
  /// current one on 54 of them (26.2%)** -- and the disagreements are
  /// ordinary, not corruption. A repository's main checkout accumulates
  /// sessions across every branch it ever held (`ghstat` alone:
  /// `stats-dashboard` x10, `audit-top-five` x2, all now on `main`) while
  /// its path never changes.
  ///
  /// So matching on `(path, branch)` would refuse a quarter of the valid
  /// jumps. The path is the key; the branch is a fact the UI states, so
  /// the user is not led to believe they are looking at the session's
  /// branch when the tree has moved on.
  movedOnFrom: string | null;
}

/// Normalise a path for comparison.
///
/// Trailing separators and `.` segments are stripped, because git's
/// `worktree list` output and Claude Code's recorded `cwd` are two
/// independent spellings of the same directory and need not agree on
/// either.
///
/// Case is deliberately PRESERVED. macOS is case-insensitive by default,
/// so lowercasing would match more often there -- and would be wrong on
/// Linux, where `/code/Widget` and `/code/widget` are two different
/// directories. A jump to the wrong tree is worse than a missing button.
function normalisePath(path: string): string {
  const parts = path.split(/[\\/]/).filter((p) => p !== "" && p !== ".");
  const lead = /^[\\/]/.test(path) ? "/" : "";
  return lead + parts.join("/");
}

/// Which live Claude session, if any, is working in each worktree
/// (#1137).
///
/// The INVERSE of `sessionWorktree`, and it reuses that function's
/// `normalisePath` deliberately: two path comparisons that could
/// disagree would put an "agent here" badge on one page and not the
/// other for the same worktree.
///
/// Keyed by the NORMALISED path, so callers look up with
/// `normalisePath(wt.path)` and never have to know the rule.
///
/// Only RUNNING sessions. A session that ended in a directory is not
/// working in it, and treating it as occupied would refuse removals
/// forever on every worktree that ever hosted one.
export function worktreeSessions(
  sessions: ClaudeSession[] | undefined,
): Map<string, ClaudeSession> {
  const out = new Map<string, ClaudeSession>();
  // `undefined` is "we could not read the session list", which is NOT
  // "nothing is running". The caller distinguishes those -- see
  // `occupancy` below -- because an empty map cannot.
  if (!sessions) return out;
  for (const s of sessions) {
    if (s.liveness.state !== "running" || !s.cwd) continue;
    const key = normalisePath(s.cwd);
    // FIRST wins, and the list arrives newest-first, so the most recent
    // session is the one named. Two agents in one directory is possible
    // and the badge names one of them; the count is not the question the
    // row asks.
    if (!out.has(key)) out.set(key, s);
  }
  return out;
}

/// What is known about whether a worktree is occupied (#1137).
///
/// Three states, not two. "No agent here" and "we could not tell" must
/// not render the same, and they must not behave the same either: a
/// removal is refused in both cases, but only one of them is a reason
/// the user can act on.
export type Occupancy =
  | { kind: "free" }
  | { kind: "occupied"; session: ClaudeSession }
  /// The session list could not be read, so occupancy is UNKNOWN. The
  /// plain Remove path refuses on this too: "we could not tell" is not
  /// "nothing is there", and this codebase's rule is that anything it
  /// cannot establish is safe is refused rather than attempted.
  | { kind: "unknown" };

export function occupancy(
  worktreePath: string,
  sessions: ClaudeSession[] | undefined,
  index: Map<string, ClaudeSession>,
): Occupancy {
  if (!sessions) return { kind: "unknown" };
  const s = index.get(normalisePath(worktreePath));
  return s ? { kind: "occupied", session: s } : { kind: "free" };
}

/// Find the worktree a session ran in, or `null` when none matches.
///
/// # Why the cwd is the key and the branch is not
///
/// `Worktree.path` is the row identity throughout `WorktreesPage`
/// (`key={wt.path}`, and both the safety-verdict and size streams are
/// keyed by it), so the absolute path is the only join key that lines up
/// with what that page already believes. See
/// [`SessionWorktree.movedOnFrom`] for the measurement that rules the
/// branch out as a second key.
///
/// # Match rate, measured
///
/// Over 1,461 real sessions: 206 match a registered worktree, which is
/// 14.1% of all sessions but **83.1% of the 248 whose directory still
/// exists**. The gap is the point -- the sessions that do not match are
/// overwhelmingly the ones whose agent worktree was deleted when the work
/// landed, and for those there is genuinely nothing to jump to.
///
/// # Why a vanished directory can still match
///
/// This compares strings against a listing git produced; it does not
/// stat anything. That listing is itself the evidence: a path git still
/// reports is a registered worktree, and one whose directory was deleted
/// is a real state git calls prunable and `WorktreesPage` already
/// renders. Measured, 0 of 206 matches were in that state, but it is one
/// `rm -rf` away and refusing to match it would hide exactly the row
/// that explains where the session's work went.
export function sessionWorktree(
  cwd: string | null,
  gitBranch: string | null,
  repos: WorktreeRepo[] | undefined,
): SessionWorktree | null {
  // `undefined` repos means the worktree listing has not loaded or could
  // not be read, which is NOT "no match". It yields the same absent
  // button, so the CALLER distinguishes the two for the user rather than
  // this function inventing a third return value nothing could act on.
  if (!cwd || !repos) return null;
  const want = normalisePath(cwd);
  for (const repo of repos) {
    for (const wt of repo.worktrees) {
      if (normalisePath(wt.path) !== want) continue;
      return {
        repoPath: repo.path,
        repoName: repo.name,
        worktree: wt,
        movedOnFrom: gitBranch && wt.branch && gitBranch !== wt.branch ? gitBranch : null,
      };
    }
  }
  return null;
}
