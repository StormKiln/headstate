import { describe, expect, it } from "vitest";
// The Rust enum and this file's own text, for the variant-parity test at the
// end of the `isSafe` block. `?raw` rather than `node:fs` because the project
// deliberately carries no `@types/node` -- see `vite.config.ts`, which
// resolves its own paths with `import.meta.url` for the same reason.
import modelRs from "../../src-tauri/src/worktrees/model.rs?raw";
// Every `Safety` variant as Rust serialises it, pinned by
// `safety_serialises_as_the_frontend_fixture_says` in that file (#1437).
import safetyFixture from "../../src-tauri/tests/fixtures/safety_variants.json?raw";
import testSource from "./worktrees.test.ts?raw";
import type {
  ClaudeSession, Lock, Safety, Worktree, WorktreeRepo } from "@/types/pr";
import {
  worktreeSessions,
  occupancy,
  canClaudify,
  forceWarning,
  formatSize,
  isDeadLock,
  isSafe,
  lockAge,
  lockHolderIsGone,
  lockHolderNote,
  lockReason,
  pathBasename,
  prForWorktree,
  safetyReason,
  safetyTone,
  sessionWorktree,
  sortWorktrees,
  totalSize,
  WORKTREE_SORT_LABELS,
} from "./worktrees";

/// A lock for tests that care only about some of its fields.
///
/// Reasons are synthetic per CONTRIBUTING.md: this repository is
/// public, and the real ones name a tool and a machine. The default
/// `underlying` is `unmerged` rather than `safe` so a test that does
/// not mention it cannot accidentally assert the "would be safe once
/// unlocked" wording.
const lock = (over: Partial<Lock> = {}): Lock => ({
  reason: "some tool (pid 123)",
  age_days: 0,
  holder_running: null,
  underlying: { kind: "unmerged" },
  ...over,
});

describe("isSafe", () => {
  // Only `safe` is deletable. Everything else is disabled rather than
  // warned past: a cleanup tool that occasionally eats a day of work is
  // worse than no cleanup tool.
  it("is true only for the merged states", () => {
    expect(isSafe({ kind: "safe" })).toBe(true);
    // #732: merged, then the remote branch was deleted. The work is on
    // the default branch, so this is as removable as `safe` -- the
    // whole point of the fix, since treating it as never-pushed left
    // every merged worktree unremovable.
    expect(isSafe({ kind: "merged_upstream_deleted" })).toBe(true);
    // #1439: merged content on a branch with no tracking config. Same
    // evidence; it used to read "never pushed" and could not be removed.
    expect(isSafe({ kind: "merged_no_upstream" })).toBe(true);
    // #819: a branchless checkout contained in the default branch. The
    // evidence is the same `merged_into` bar the two above clear --
    // ancestry or an exact patch-id match -- and what this row LACKS is a
    // branch, which is the thing removal would otherwise lose. Four such
    // rows on the reporting machine were `unknown` with no action at all.
    expect(isSafe({ kind: "detached_merged", detail: "detached at v1.13.0~30" })).toBe(true);
    for (const s of [
      { kind: "main_checkout" },
      { kind: "dirty", detail: 3 },
      { kind: "unpushed", detail: 2 },
      { kind: "never_pushed" },
      // `empty` included deliberately. Nothing on the branch could be
      // lost, and it is still not one-click removable: #701 reported
      // that the WORDING was wrong, and quietly widening the app's only
      // unrecoverable action on the back of a copy fix is not what was
      // asked for. The forced path stays available.
      { kind: "empty" },
      { kind: "unmerged" },
      // #753, both spellings of a lock and the stale registration.
      // Safe-by-default: a locked tree is one git refuses outright,
      // and a prunable one has no directory left to remove.
      { kind: "locked", detail: lock() },
      { kind: "locked", detail: lock({ reason: null }) },
      // #775: a lock whose work IS merged underneath. It must stay
      // un-removable -- the row now says it "would be safe once
      // unlocked", and that must remain a statement about a
      // hypothetical rather than a licence.
      { kind: "locked", detail: lock({ underlying: { kind: "safe" } }) },
      { kind: "prunable", detail: "gitdir file points to non-existent location" },
      // #851: the two variants Rust's own refusal list singles out, and
      // the two this mirror was missing. Rust's `is_safe` test names both
      // (`worktrees/model.rs:667-668`) and is followed by one whose doc
      // reads "the default must never be deletable… this is the one place
      // where getting it wrong deletes someone's work".
      //
      // `pending` is the DEFAULT (`Safety::default() == Pending`), which
      // is what makes the hole matter: `isSafe` is an allowlist, so a
      // careless widening -- or adding `pending` to it -- would arm Remove
      // on every row still classifying, and this suite would have passed.
      // Rust's would not, but Rust is not what greys the button.
      { kind: "pending" },
      // `orphaned` means the repository itself is gone, so nothing here
      // can be checked at all ("its repository is gone -- nothing here
      // can be checked", `model.rs:521`). An unknowable state is the last
      // one that should license an unrecoverable action.
      { kind: "orphaned" },
      { kind: "unknown", detail: "x" },
      // The other half of #819, and the one that keeps it honest. A
      // detached checkout whose HEAD is NOT on the default branch arrives
      // as `unknown` and must stay un-removable: without this, the change
      // would be indistinguishable from "call every detached row safe",
      // which is the opposite of what the issue asks. Spelled with the
      // real detail string the Rust side produces, so the pairing is
      // visible here rather than only in scan.rs.
      { kind: "unknown", detail: "detached HEAD at v1.13.0~30 — not found on main" },
    ] as Safety[]) {
      expect(isSafe(s)).toBe(false);
    }
  });

  /// Every Rust `Safety` variant appears above, and nothing is missing.
  ///
  /// #851's finding was that the enumeration above had a HOLE: `pending`
  /// and `orphaned` -- the two Rust's own refusal list singles out, one of
  /// them the default -- were absent, so the suite guarding the app's only
  /// unrecoverable action passed without ever asking about the state most
  /// rows are in for the first minute of a scan.
  ///
  /// Listing them (done) fixes today. This fixes tomorrow: it reads both
  /// enumerations and fails when they diverge, so a variant added to Rust
  /// cannot be silently absent from the mirror. Without it the same hole
  /// reopens the next time `Safety` grows a case, and the next reviewer has
  /// to notice by eye what a test can notice for them.
  ///
  /// # Why it parses the Rust source
  ///
  /// The Rust enum is the definition; `types/pr.ts` is a hand-written
  /// mirror of it. Comparing the mirror against itself would assert
  /// nothing, and there is no generated artifact to compare against -- so
  /// the only thing that can catch a divergence is the original. The
  /// `?raw` import does the same job `App.lazy.test.tsx` uses it for.
  it("enumerates every Rust Safety variant", () => {
    // Variant names from the Rust enum's own declaration. Anchored to the
    // `pub enum Safety {` block so the `match` arms elsewhere in the file,
    // which repeat every name, cannot pad the set.
    const block = modelRs.slice(
      modelRs.indexOf("pub enum Safety {"),
      modelRs.indexOf("\n}", modelRs.indexOf("pub enum Safety {")),
    );
    expect(block.length).toBeGreaterThan(0);
    // A struct variant (`InProgress {`) counts too. The pattern once
    // accepted only unit and tuple variants, so `InProgress` was never in
    // this set and its misspelt TS kind went unnoticed (#1437).
    const rustVariants = new Set(
      [...block.matchAll(/^\s{4}([A-Z][A-Za-z]*)(?:\(|,|\s*\{|\s*$)/gm)].map((m) => m[1]),
    );
    // A sanity floor: if the regex stops matching, an empty set would make
    // this test pass while checking nothing.
    expect(rustVariants.size).toBeGreaterThanOrEqual(12);

    // `MergedUpstreamDeleted` -> `merged_upstream_deleted`, which is what
    // `#[serde(rename_all = "snake_case")]` produces and what the TS union
    // spells.
    const snake = (name: string) =>
      name.replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase();

    // Every `kind` this file asserts on, across both the safe and the
    // refused list. Read off the test source rather than maintained as a
    // second list, so adding a variant to the arrays above is all it takes.
    const asserted = new Set(
      [...testSource.matchAll(/\{\s*kind:\s*"([a-z_]+)"/g)].map((m) => m[1]),
    );

    const expected = [...rustVariants].map(snake).sort();
    const missing = expected.filter((k) => !asserted.has(k));
    expect(
      missing,
      `isSafe is an ALLOWLIST, so a variant nobody asserts about is a \
variant a careless widening could arm Remove for. Rust's own refusal test \
(worktrees/model.rs) names every one of these; add the missing ones to the \
lists above. Missing: ${missing.join(", ")}`,
    ).toEqual([]);
  });
});

describe("safetyReason", () => {
  it("pluralises counts", () => {
    expect(safetyReason({ kind: "dirty", detail: 1 })).toBe("1 uncommitted file");
    expect(safetyReason({ kind: "dirty", detail: 3 })).toBe("3 uncommitted files");
    expect(safetyReason({ kind: "unpushed", detail: 1 })).toBe("1 unpushed commit");
  });

  // The most dangerous state deserves the plainest words: 52 of 295
  // worktrees on this machine hold commits that exist nowhere else.
  it("says plainly when commits exist nowhere else", () => {
    expect(safetyReason({ kind: "never_pushed" })).toContain("only here");
    // Both halves matter: "merged" is why the button is enabled, and
    // "upstream deleted" is why no remote branch can be pointed at.
    const gone = safetyReason({ kind: "merged_upstream_deleted" });
    expect(gone).toContain("merged");
    expect(gone).toContain("upstream deleted");
    // It must NOT read like the state it was being confused with.
    expect(gone).not.toContain("only here");
    // #1439: merged, and the branch has no tracking config. It must say
    // merged and must not repeat the claim it replaced.
    const untracked = safetyReason({ kind: "merged_no_upstream" });
    expect(untracked).toContain("merged");
    expect(untracked).toContain("no upstream");
    expect(untracked).not.toContain("only here");
    expect(untracked).not.toContain("never pushed");
  });

  // The bug in #701: a scratch branch was described as holding commits
  // that exist only here, beside "0 commits ahead". Both cannot be
  // true, and the user believed the scarier one.
  it("does not claim an empty branch holds commits", () => {
    const reason = safetyReason({ kind: "empty" });
    expect(reason).toContain("no commits of its own");
    expect(reason).not.toContain("only here");
    expect(reason).not.toBe(safetyReason({ kind: "never_pushed" }));
  });

  // #753 named the locker; #775 leads with the AGE instead.
  //
  // The reason alone stopped discriminating once locks accumulated:
  // all 20 on the reporting machine name the same pid, which is alive
  // only because it is the parent that outlived the workers. The age
  // is the part that differs per row, so a stale lock has to READ as
  // stale rather than merely carry a string that looks like evidence.
  it("leads with a lock's age, and still names the holder", () => {
    const held = safetyReason({
      kind: "locked",
      detail: lock({ age_days: 5 }),
    });
    expect(held).toContain("locked 5 days ago");
    // The reason is demoted, not dropped: it is the locker's own words.
    expect(held).toContain("some tool (pid 123)");
    // The age comes FIRST. A row that opened with the pid would put
    // the misleading half in the place the eye lands.
    expect(held.indexOf("5 days ago")).toBeLessThan(
      held.indexOf("some tool"),
    );

    // A lock without `--reason` is still a lock, and must not render
    // as an empty quotation that reads like a display bug.
    const bare = safetyReason({
      kind: "locked",
      detail: lock({ reason: null }),
    });
    expect(bare).toContain("locked");
    expect(bare).toContain("no reason given");
  });

  // #775: the fact that makes unlocking a decision rather than a leap.
  // 16 of the 18 classifiable locked worktrees measured were merged
  // underneath, so this is the common case, not a corner.
  it("says when a locked worktree is merged underneath", () => {
    const merged = safetyReason({
      kind: "locked",
      detail: lock({ underlying: { kind: "safe" } }),
    });
    expect(merged).toContain("would be safe once unlocked");

    // And stays silent when it is not true. Claiming it over unmerged
    // work would invite exactly the blind unlock this exists to stop.
    const notMerged = safetyReason({
      kind: "locked",
      detail: lock({ underlying: { kind: "unmerged" } }),
    });
    expect(notMerged).not.toContain("would be safe");
  });

  // An age the app could not read must not become a reassuring one.
  // "locked today" over a lock of unknown age is the one wrong answer
  // that makes clearing it feel safer than it is.
  it("says nothing about the age it could not read", () => {
    const unknown = safetyReason({
      kind: "locked",
      detail: lock({ age_days: null }),
    });
    expect(unknown).toContain("locked");
    expect(unknown).not.toContain("today");
    expect(unknown).not.toContain("days ago");
  });

  // #753: this used to read "could not determine: directory is
  // missing" -- true, but it names neither the cause nor the cure for
  // something one safe command fixes. #814 then reordered it: naming the
  // cure is not enough while the sentence still OPENS with a loss.
  it("leads a stale registration with the reassurance, not the problem", () => {
    const stale = safetyReason({
      kind: "prunable",
      detail: "gitdir file points to non-existent location",
    });
    // THE ORDER is the issue. Asserted as a prefix rather than a
    // `contains`, because "directory is gone — nothing to lose" would
    // pass a containment check while being the exact sentence #814
    // objects to: the answer has to arrive before the problem.
    expect(stale.startsWith("nothing to lose")).toBe(true);
    // The remedy survives, named as a VERB the user can act on rather
    // than as the adjective "prunable" -- git vocabulary was half of what
    // made the row read as a warning.
    expect(stale).toContain("prune to clear");
    // And git's own reason is still carried: #814 asked for a reordering,
    // not for evidence to be dropped.
    expect(stale).toContain("non-existent location");
    expect(stale).not.toContain("could not determine");
  });

  // #819: four worktrees read "could not determine: detached HEAD" while
  // `merge-base --is-ancestor` answered true for every one of them. The
  // row must lead with the answer, identify the sha, and say the thing
  // that makes removal easy to accept -- that there is no branch to lose.
  it("says a merged detached checkout is merged, and what it is", () => {
    const d = safetyReason({ kind: "detached_merged", detail: "detached at v1.13.0~30" });
    expect(d.startsWith("merged")).toBe(true);
    // The sha is IDENTIFIED. "detached at v1.13.0~30" is what turns a
    // row the user cannot act on into one they can.
    expect(d).toContain("v1.13.0~30");
    expect(d).toContain("no branch to delete");
    // It is not a hedge any more.
    expect(d).not.toContain("could not determine");
    // And it must never read like the state #776 stopped it claiming.
    expect(d).not.toContain("only here");
  });
});

describe("lockAge", () => {
  // Whole days is the resolution the decision needs. Nobody unlocks
  // differently for 5 days versus 5 days and 3 hours.
  it("reads as prose, not as a number of days", () => {
    expect(lockAge(lock({ age_days: 0 }))).toBe("today");
    expect(lockAge(lock({ age_days: 1 }))).toBe("yesterday");
    expect(lockAge(lock({ age_days: 5 }))).toBe("5 days ago");
  });

  // Null, not "today". An age the app could not read is not a lock
  // taken this second, and that is the direction in which a wrong
  // guess makes clearing it feel safer than it is.
  it("says nothing when the age is unknown", () => {
    expect(lockAge(lock({ age_days: null }))).toBeNull();
  });
});

describe("lockHolderNote", () => {
  // The whole point of #775's first problem. A running pid is TRUE for
  // all 20 locks on the reporting machine and every one of them is
  // abandoned, because the pid belongs to the parent session rather
  // than to the worker that took the lock. So the sentence must carry
  // its own caveat -- an unqualified "still running" is the app
  // laundering weak evidence into a strong claim.
  it("does not present a running process as proof the lock is live", () => {
    const note = lockHolderNote(lock({ holder_running: true }));
    expect(note).toContain("still running");
    expect(note).toContain("weak evidence");
  });

  // The one unambiguous signal available here, and it deserves saying
  // plainly rather than hedged like the "true" case.
  it("says plainly when the named process is gone", () => {
    const note = lockHolderNote(lock({ holder_running: false }));
    expect(note).toContain("no longer running");
    expect(note).not.toContain("weak evidence");
    // And says WHY it can be trusted (#792). Pids are recycled, so a
    // reader who knows that has no reason to believe "no longer
    // running" unless the sentence says the start time was checked too
    // -- and being believable is this line's entire job.
    expect(note).toContain("start time");
  });

  // Nothing to check is not the same as "the holder is gone". The
  // second would read as evidence the lock is stale, which is a claim
  // nothing supports for a lock that simply names no pid.
  it("says nothing when there was no pid to check", () => {
    expect(lockHolderNote(lock({ holder_running: null }))).toBeNull();
  });
});

describe("lockReason", () => {
  // One sentence, shared by the row and the confirmation. #753's
  // `forceWarning` showed the cost of two copies of a warning: two
  // chances to drift on the wording that decides whether somebody
  // clears another process's claim.
  it("orders the evidence best-first", () => {
    const line = lockReason(
      lock({ age_days: 5, underlying: { kind: "safe" } }),
    );
    expect(line).toBe(
      "locked 5 days ago by some tool (pid 123) — merged, would be safe once unlocked",
    );
  });

  it("degrades to the fact of the lock when it knows nothing else", () => {
    const line = lockReason(
      lock({ age_days: null, reason: null, underlying: { kind: "unmerged" } }),
    );
    expect(line).toBe("locked — no reason given");
  });

  // #792: the row is where the decision is made, and it never read
  // `holder_running` -- the single consumer was the unlock dialog, so the
  // user learned the holder was dead only after deciding to unlock.
  //
  // APPENDED, never folded in. Git's reason is the locker's own words and
  // rewriting them would be the app inventing a claim on another
  // process's behalf, so the exact full string is asserted rather than a
  // substring: that is what pins the order.
  it("says when the named holder is gone, after git's own reason", () => {
    const line = lockReason(lock({ age_days: 2, holder_running: false }));
    expect(line).toBe("locked 2 days ago by some tool (pid 123) — holder process is gone");
  });

  // Before the merge verdict, because the two answer different questions
  // and they are read in that order: "is anything holding this" decides
  // whether to unlock at all, "what is underneath" decides whether it was
  // worth it.
  it("puts the dead holder before what is underneath", () => {
    const line = lockReason(
      lock({ age_days: 2, holder_running: false, underlying: { kind: "safe" } }),
    );
    expect(line).toBe(
      "locked 2 days ago by some tool (pid 123) — holder process is gone — merged, would be safe once unlocked",
    );
  });

  // Silent in both the other cases. A running pid was true for all 20
  // locks on the reporting machine and every one was abandoned, so
  // announcing it would spend weak evidence as proof; `null` means
  // nothing was named to check.
  it("says nothing about a holder that is running or was never checked", () => {
    expect(lockReason(lock({ holder_running: true }))).not.toContain("holder process");
    expect(lockReason(lock({ holder_running: null }))).not.toContain("holder process");
  });
});

describe("safetyTone", () => {
  it("uses green only for safe", () => {
    expect(safetyTone({ kind: "safe" })).toContain("3fb950");
    expect(safetyTone({ kind: "unmerged" })).not.toContain("3fb950");
    expect(safetyTone({ kind: "never_pushed" })).not.toContain("3fb950");
    // Green, like `safe`: same verdict, different evidence (#732).
    expect(safetyTone({ kind: "merged_upstream_deleted" })).toContain("3fb950");
    // #1439: in `isSafe`, so green.
    expect(safetyTone({ kind: "merged_no_upstream" })).toContain("3fb950");
    // Green for the same reason again (#819). Green on this page means
    // one-click removable, `isSafe` includes this kind, and the two must
    // not disagree -- a green row with a disabled button, or a grey row
    // with an enabled one, is the colour lying about the action.
    expect(safetyTone({ kind: "detached_merged", detail: "detached at v1.13.0~30" })).toContain(
      "3fb950",
    );
  });

  // The main checkout is not a problem, so it must not look like one.
  it("does not alarm about the main checkout", () => {
    expect(safetyTone({ kind: "main_checkout" })).toContain("8b949e");
  });

  // Grey, not red. An empty branch holds no work, so painting it the
  // same colour as "commits exist only here" would repeat #701 in a
  // medium the user reads before the words.
  it("does not alarm about an empty branch", () => {
    expect(safetyTone({ kind: "empty" })).toContain("8b949e");
    expect(safetyTone({ kind: "empty" })).not.toBe(safetyTone({ kind: "never_pushed" }));
  });

  // #753: neither new state may look removable, and neither is a
  // danger. A lock is an obstacle the user can clear, so amber; a
  // prunable row has no directory left to endanger anything, so grey.
  it("marks locked and prunable as neither safe nor alarming", () => {
    const locked = safetyTone({ kind: "locked", detail: lock() });
    expect(locked).toContain("d29922");
    expect(locked).not.toContain("3fb950");
    const prunable = safetyTone({ kind: "prunable", detail: "gone" });
    expect(prunable).toContain("8b949e");
    expect(prunable).not.toContain("3fb950");
  });

  // #792: amber asks for a judgement, and a lock whose named process is
  // provably gone has none left to make -- so it takes the grey that
  // `prunable` already uses for the other pure-bookkeeping state. Still
  // not green: green means one-click removable, and this takes an unlock
  // first.
  it("tones a lock with no live holder as stale rather than amber", () => {
    const dead = safetyTone({ kind: "locked", detail: lock({ holder_running: false }) });
    expect(dead).toContain("8b949e");
    expect(dead).not.toContain("d29922");
    expect(dead).not.toContain("3fb950");
    // And only for the decisive case. A running holder keeps the amber
    // that asks the user to think, and an unchecked one must not be
    // treated as a negative answer.
    expect(safetyTone({ kind: "locked", detail: lock({ holder_running: true }) })).toContain(
      "d29922",
    );
    expect(safetyTone({ kind: "locked", detail: lock({ holder_running: null }) })).toContain(
      "d29922",
    );
  });
});

describe("lockHolderIsGone", () => {
  // The distinction the whole of #792 rests on, and the reason this is a
  // named predicate rather than `=== false` written out in three places.
  // `null` is "nobody was named to check", not "nothing holds it" --
  // most locks not written by our own tooling land there, and treating
  // an unasked question as a negative answer is how a live claim gets
  // cleared.
  it("is true only when a named process was checked and found gone", () => {
    expect(lockHolderIsGone(lock({ holder_running: false }))).toBe(true);
    expect(lockHolderIsGone(lock({ holder_running: true }))).toBe(false);
    expect(lockHolderIsGone(lock({ holder_running: null }))).toBe(false);
  });
});

describe("isDeadLock", () => {
  const wt = (safety: Safety): Worktree => ({
    path: "/code/a",
    branch: "feature",
    head: "abc",
    size_bytes: null,
    safety,
    is_main: false,
    merged_at: null,
    upstream: null,
    last_commit: null,
  });

  // The bulk unlock's selector (#792). Narrower than "locked" on
  // purpose: a batch over claims that might be live is exactly what #753
  // refused to offer, and nothing here reopens that.
  it("selects locked rows whose holder is provably gone, and nothing else", () => {
    expect(isDeadLock(wt({ kind: "locked", detail: lock({ holder_running: false }) }))).toBe(true);
    expect(isDeadLock(wt({ kind: "locked", detail: lock({ holder_running: true }) }))).toBe(false);
    expect(isDeadLock(wt({ kind: "locked", detail: lock({ holder_running: null }) }))).toBe(false);
    // Not a lock at all. `prunable` is the near miss worth naming: it is
    // also pure stale bookkeeping and also grey, and it has its own verb
    // (#793) rather than being swept into this batch.
    expect(isDeadLock(wt({ kind: "prunable", detail: "gone" }))).toBe(false);
    expect(isDeadLock(wt({ kind: "safe" }))).toBe(false);
  });
});

describe("forceWarning", () => {
  // The confirmation is the moment the user decides, so a false claim
  // there is worse than one on the row.
  it("does not warn about commits an empty branch does not have", () => {
    const warning = forceWarning({ kind: "empty" });
    expect(warning).toContain("nothing on it would be lost");
    expect(warning).not.toContain("not pushed anywhere");
    expect(warning).toContain("cannot be undone");
  });

  it("still names the specific loss for a never-pushed branch", () => {
    expect(forceWarning({ kind: "never_pushed" })).toContain("not pushed anywhere");
  });

  it("falls back to the general form for everything else", () => {
    expect(forceWarning({ kind: "unmerged" })).toContain("does not consider this safe");
  });

  // #798: the count is the sentence the user needed, and the app had
  // it all along. The generic line described Headstate's opinion where
  // the question is what disappears -- and this is now the one
  // previously-impossible removal that actually goes through, so the
  // stakes have to be stated before it does.
  it("names how many uncommitted files a dirty worktree would lose", () => {
    const warning = forceWarning({ kind: "dirty", detail: 2 });
    expect(warning).toContain("2 uncommitted files");
    expect(warning).toContain("deleted permanently");
    expect(warning).toContain("cannot be undone");
    expect(warning).not.toContain("does not consider this safe");
  });

  // One file is one file. A plural here would be the kind of small
  // wrongness that makes a user doubt the number itself, at the moment
  // the number is the whole reason to read the dialog.
  it("says it in the singular for one file", () => {
    expect(forceWarning({ kind: "dirty", detail: 1 })).toContain("1 uncommitted file will");
  });

  // #753/#798: forcing does not work here, and the dialog must say so.
  // Since #798 that is a decision rather than a gap -- git wants
  // `--force --force` for a lock and Headstate passes it once, because
  // a lock is another process's claim. So git still refuses, and the
  // general wording would walk the user through a destructive-sounding
  // confirmation and then hand them an error.
  it("tells the truth about a locked worktree", () => {
    const warning = forceWarning({ kind: "locked", detail: lock() });
    expect(warning).toContain("locked");
    expect(warning).toContain("unlocked");
    expect(warning).not.toContain("does not consider this safe");
  });

  // Nothing can be lost when the directory is already gone, so the
  // destructive framing would be simply false. Since #793 it also
  // names the action that exists rather than a command to retype
  // elsewhere -- the app can run `git worktree prune` itself now.
  it("does not threaten loss for a directory that is already gone", () => {
    const warning = forceWarning({ kind: "prunable", detail: "gone" });
    expect(warning).toContain("Nothing can be lost");
    expect(warning).toContain("Prune stale registrations");
    expect(warning).not.toContain("cannot be undone");
  });
});

describe("canClaudify", () => {
  // The rule is "not removable and not the main checkout". `empty` is
  // both, and the gate keeps it un-removable -- so withholding the
  // assess action too would leave the row with no action at all.
  it("offers assessment for an empty branch", () => {
    expect(canClaudify({ kind: "empty" })).toBe(true);
  });

  it("still refuses the states with no question to ask", () => {
    expect(canClaudify({ kind: "safe" })).toBe(false);
    expect(canClaudify({ kind: "main_checkout" })).toBe(false);
    expect(canClaudify({ kind: "pending" })).toBe(false);
  });

  // #753: the two states where "not removable and not the main
  // checkout" stops implying "offer the agent". A prunable row has no
  // directory for an agent to open, and a locked one is very often
  // locked BY an agent already working in it -- pointing a second one
  // at that directory is what the lock exists to prevent.
  it("does not send an agent into a locked or missing directory", () => {
    expect(canClaudify({ kind: "locked", detail: lock() })).toBe(false);
    expect(canClaudify({ kind: "locked", detail: lock({ reason: null }) })).toBe(false);
    expect(canClaudify({ kind: "prunable", detail: "gone" })).toBe(false);
  });
});

describe("formatSize", () => {
  it("scales units", () => {
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1024)).toBe("1.0 KB");
    expect(formatSize(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatSize(3 * 1024 ** 3)).toBe("3.0 GB");
  });

  // An unmeasured size must not read as an empty directory.
  it("shows a dash rather than claiming zero", () => {
    expect(formatSize(null)).toBe("—");
    expect(formatSize(0)).toBe("0 B");
  });
});

describe("pathBasename", () => {
  // The bug: split("/") returns the whole string unchanged on a Windows
  // path, so every row would show the full path instead of the directory.
  it("finds the last component of a Windows path", () => {
    expect(pathBasename("C:\\Users\\me\\code\\proj-feature")).toBe("proj-feature");
  });

  it("finds the last component of a Unix path", () => {
    expect(pathBasename("/Users/me/code/proj-feature")).toBe("proj-feature");
  });

  // Git on Windows often reports forward slashes even for Windows paths,
  // so both must work regardless of which platform produced them.
  it("handles a Windows drive with forward slashes", () => {
    expect(pathBasename("C:/Users/me/code/proj")).toBe("proj");
  });

  it("ignores a trailing separator rather than returning empty", () => {
    expect(pathBasename("/code/proj/")).toBe("proj");
    expect(pathBasename("C:\\code\\proj\\")).toBe("proj");
  });

  it("returns the input when there is no separator at all", () => {
    expect(pathBasename("proj")).toBe("proj");
  });
});

describe("prForWorktree", () => {
  const pr = (repo: string, head: string, number: number) =>
    ({ repo, head_ref: head, number } as unknown as import("@/types/pr").PullRequest);

  it("pairs a worktree with its pull request", () => {
    const prs = [pr("octocat/api", "feat/x", 1)];
    expect(prForWorktree(prs, "octocat/api", "feat/x")?.number).toBe(1);
  });

  // THE trap. Branch names are not unique across repositories -- this
  // account has feat/egr33-* in two of them -- and a wrong pairing would
  // attach GitHub's authoritative-looking "merged" to the wrong
  // directory.
  it("never matches the same branch in a different repository", () => {
    const prs = [pr("octocat/api", "feat/shared", 1)];
    expect(prForWorktree(prs, "octocat/worker", "feat/shared")).toBeNull();
  });

  // A repo with no remote resolves to null identity, which must mean
  // "no pairing" rather than "match anything".
  //
  // The PR fixture below carries repo=null so that a comparison-only
  // implementation WOULD match it -- without that, the guard could be
  // deleted and this test would still pass, since `repo === null` never
  // equals a real repo name.
  it("makes no match when the repository cannot be identified", () => {
    const prs = [
      { repo: null, head_ref: "feat/x", number: 9 } as unknown as
        import("@/types/pr").PullRequest,
    ];
    expect(prForWorktree(prs, null, "feat/x")).toBeNull();
  });

  it("makes no match for a detached worktree", () => {
    const prs = [pr("octocat/api", "feat/x", 1)];
    expect(prForWorktree(prs, "octocat/api", "")).toBeNull();
  });
});

describe("totalSize", () => {
  const w = (size_bytes: number | null) => ({ size_bytes });

  /// THE bug: `reduce(...) || null` turned a real zero into "unknown",
  /// rendering as `—` where `0 B` was the answer.
  it("reports a measured total of zero as zero, not as unknown", () => {
    expect(totalSize([w(0), w(0)])).toBe(0);
  });

  /// The other half: an unmeasured set also summed to 0 and showed the
  /// same dash, so "still measuring" was indistinguishable from
  /// "nothing to reclaim". Sizing is slow, which is why this appeared
  /// only sometimes.
  it("reports nothing-measured as unknown", () => {
    expect(totalSize([w(null), w(null)])).toBeNull();
    expect(totalSize([])).toBeNull();
  });

  it("sums what is known and ignores what is not", () => {
    expect(totalSize([w(100), w(null), w(50)])).toBe(150);
  });

  it("is a real total when everything is measured", () => {
    expect(totalSize([w(1024), w(2048)])).toBe(3072);
  });
});

/// #771: the list rendered name, size and age on every row and could
/// order by none of them.
describe("sortWorktrees", () => {
  const w = (over: Partial<Worktree>): Worktree => ({
    path: "/code/octocat-hello-world",
    branch: "feature",
    head: "abc",
    size_bytes: 1024,
    safety: { kind: "unmerged" },
    is_main: false,
    merged_at: null,
    upstream: null,
    last_commit: null,
    ...over,
  });
  const names = (list: Worktree[]) => list.map((x) => pathBasename(x.path));

  describe("size", () => {
    const list = [
      w({ path: "/code/small", size_bytes: 10 }),
      w({ path: "/code/huge", size_bytes: 9_000_000 }),
      w({ path: "/code/middling", size_bytes: 5_000 }),
    ];

    it("puts the biggest first on size-desc", () => {
      expect(names(sortWorktrees(list, "size-desc"))).toEqual(["huge", "middling", "small"]);
    });

    it("puts the smallest first on size-asc", () => {
      expect(names(sortWorktrees(list, "size-asc"))).toEqual(["small", "middling", "huge"]);
    });

    /// THE ordering bug this has to avoid (#360, and the reason
    /// `ArtifactsPage` answers it the same way). A null size treated as
    /// zero ranks the unmeasured rows as the SMALLEST on the page,
    /// which under "Largest first" buries exactly the directory that
    /// might be the biggest thing on the disk.
    it("sorts an unmeasured size last rather than as zero", () => {
      const withUnknown = [
        w({ path: "/code/unmeasured", size_bytes: null }),
        w({ path: "/code/small", size_bytes: 10 }),
        w({ path: "/code/huge", size_bytes: 9_000_000 }),
      ];
      expect(names(sortWorktrees(withUnknown, "size-desc"))).toEqual([
        "huge",
        "small",
        "unmeasured",
      ]);
    });

    /// The other direction, which is the half a "nulls are zero"
    /// implementation gets accidentally right and a "nulls are
    /// infinity" one gets wrong. An unknown is ABSENT from the
    /// ordering, not an extreme of it, so it does not lead here either
    /// -- ranking it first would claim it is the smallest.
    it("still sorts an unmeasured size last when smallest leads", () => {
      const withUnknown = [
        w({ path: "/code/unmeasured", size_bytes: null }),
        w({ path: "/code/small", size_bytes: 10 }),
        w({ path: "/code/huge", size_bytes: 9_000_000 }),
      ];
      expect(names(sortWorktrees(withUnknown, "size-asc"))).toEqual([
        "small",
        "huge",
        "unmeasured",
      ]);
    });

    /// A repository mid-measurement is mostly unknowns, and an
    /// arbitrary order among them would reshuffle on every render.
    it("orders unmeasured rows among themselves by path, stably", () => {
      const allUnknown = [
        w({ path: "/code/ccc", size_bytes: null }),
        w({ path: "/code/aaa", size_bytes: null }),
        w({ path: "/code/bbb", size_bytes: null }),
      ];
      expect(names(sortWorktrees(allUnknown, "size-desc"))).toEqual(["aaa", "bbb", "ccc"]);
    });

    /// A measured zero is an ANSWER -- an empty checkout really does
    /// hold nothing -- so it must outrank an unknown rather than share
    /// its place at the bottom.
    it("ranks a measured zero above an unmeasured row", () => {
      const list0 = [
        w({ path: "/code/unmeasured", size_bytes: null }),
        w({ path: "/code/empty", size_bytes: 0 }),
      ];
      expect(names(sortWorktrees(list0, "size-desc"))).toEqual(["empty", "unmeasured"]);
    });
  });

  describe("age", () => {
    const list = [
      w({ path: "/code/recent", last_commit: "2026-09-01T00:00:00Z" }),
      w({ path: "/code/ancient", last_commit: "2025-01-01T00:00:00Z" }),
      w({ path: "/code/middling", last_commit: "2026-01-01T00:00:00Z" }),
    ];

    /// The safe wins first: a worktree last touched four months ago is
    /// a far easier delete than one from this morning.
    it("puts the least recently committed first on age-desc", () => {
      expect(names(sortWorktrees(list, "age-desc"))).toEqual(["ancient", "middling", "recent"]);
    });

    it("puts the most recently committed first on age-asc", () => {
      expect(names(sortWorktrees(list, "age-asc"))).toEqual(["recent", "middling", "ancient"]);
    });

    /// A null timestamp read as epoch 0 would date the row to 1970 and
    /// pin it to the top of "least recently committed" -- a confident
    /// claim that the app has no evidence for.
    it("sorts an unknown last commit last rather than as 1970", () => {
      const withUnknown = [
        w({ path: "/code/undated", last_commit: null }),
        ...list,
      ];
      expect(names(sortWorktrees(withUnknown, "age-desc"))).toEqual([
        "ancient",
        "middling",
        "recent",
        "undated",
      ]);
    });

    /// Garbage from git is the same kind of absence as no value at all,
    /// and `Date.parse` answers NaN rather than throwing -- which would
    /// otherwise poison every comparison it took part in.
    it("treats an unparseable timestamp as unknown, not as NaN", () => {
      const withJunk = [
        w({ path: "/code/junk", last_commit: "not a date" }),
        w({ path: "/code/recent", last_commit: "2026-09-01T00:00:00Z" }),
      ];
      expect(names(sortWorktrees(withJunk, "age-desc"))).toEqual(["recent", "junk"]);
    });
  });

  describe("name", () => {
    const list = [
      w({ path: "/code/charlie" }),
      w({ path: "/code/alpha" }),
      w({ path: "/code/bravo" }),
    ];

    it("orders A to Z, and back again", () => {
      expect(names(sortWorktrees(list, "name-asc"))).toEqual(["alpha", "bravo", "charlie"]);
      expect(names(sortWorktrees(list, "name-desc"))).toEqual(["charlie", "bravo", "alpha"]);
    });

    /// Ordering by the whole path would order by a prefix every row in
    /// a repository shares, which is ordering by nothing.
    it("orders by the basename the row shows, not the full path", () => {
      const nested = [
        w({ path: "/code/zzz/alpha" }),
        w({ path: "/code/aaa/bravo" }),
      ];
      expect(names(sortWorktrees(nested, "name-asc"))).toEqual(["alpha", "bravo"]);
    });

    /// Name is the one axis that is fully known at first render, which
    /// is what makes it the escape hatch while sizes are still landing.
    it("is unaffected by a missing size", () => {
      const unmeasured = [
        w({ path: "/code/bravo", size_bytes: null }),
        w({ path: "/code/alpha", size_bytes: null }),
      ];
      expect(names(sortWorktrees(unmeasured, "name-asc"))).toEqual(["alpha", "bravo"]);
    });
  });

  describe("the invariants no sort may break", () => {
    /// The main checkout is not a peer of the rows below it -- every
    /// one of those is a removal candidate and it never is -- and its
    /// row carries the upstream prose that explains why the others are
    /// stale.
    it("keeps the main checkout first whatever the axis", () => {
      const list = [
        w({ path: "/code/huge", size_bytes: 9_000_000 }),
        w({ path: "/code/zzz-main", size_bytes: 1, is_main: true }),
      ];
      for (const sort of Object.keys(WORKTREE_SORT_LABELS) as (keyof typeof WORKTREE_SORT_LABELS)[]) {
        expect(names(sortWorktrees(list, sort))[0]).toBe("zzz-main");
      }
    });

    /// The user has just come back from reading a verdict; a size sort
    /// must not bury the one row they were mid-decision on.
    it("keeps an assessed row above the unassessed ones", () => {
      const list = [
        w({ path: "/code/huge", size_bytes: 9_000_000 }),
        w({ path: "/code/tiny", size_bytes: 1 }),
      ];
      const order = sortWorktrees(list, "size-desc", new Set(["/code/tiny"]));
      expect(names(order)).toEqual(["tiny", "huge"]);
    });

    it("does not mutate the array it was given", () => {
      const list = [
        w({ path: "/code/bravo", size_bytes: 1 }),
        w({ path: "/code/alpha", size_bytes: 9 }),
      ];
      sortWorktrees(list, "size-desc");
      expect(names(list)).toEqual(["bravo", "alpha"]);
    });
  });
});

/// #920: matching a Claude Code session's recorded cwd to a worktree.
describe("sessionWorktree", () => {
  const wt = (over: Partial<Worktree> = {}): Worktree => ({
    path: "/Users/acme/code/widget/.worktrees/spoon",
    branch: "feat/spoon",
    head: "abc1234",
    size_bytes: 1024,
    safety: { kind: "safe" },
    is_main: false,
    merged_at: "2026-09-12",
    upstream: { kind: "current" },
    last_commit: "2026-09-12T10:00:00Z",
    ...over,
  });
  const repo = (worktrees: Worktree[], over: Partial<WorktreeRepo> = {}): WorktreeRepo => ({
    identity: "acme/widget",
    name: "widget",
    path: "/Users/acme/code/widget",
    worktrees,
    ...over,
  });

  it("matches on the absolute path and returns the navigation target", () => {
    const got = sessionWorktree(
      "/Users/acme/code/widget/.worktrees/spoon",
      "feat/spoon",
      [repo([wt()])],
    );
    expect(got).not.toBeNull();
    expect(got?.repoPath).toBe("/Users/acme/code/widget");
    expect(got?.repoName).toBe("widget");
    expect(got?.worktree.branch).toBe("feat/spoon");
    expect(got?.movedOnFrom).toBeNull();
  });

  it("returns null when no worktree has that path", () => {
    expect(
      sessionWorktree("/Users/acme/code/widget/.worktrees/deleted", "feat/gone", [repo([wt()])]),
    ).toBeNull();
  });

  /// `undefined` repos is "the listing has not loaded or could not be
  /// read", which is NOT the same fact as "no match" -- the caller renders
  /// them differently. This only pins that it does not throw or invent a
  /// match.
  it("returns null when the listing is absent", () => {
    expect(sessionWorktree("/Users/acme/code/widget", "main", undefined)).toBeNull();
  });

  it("returns null when the session recorded no directory", () => {
    expect(sessionWorktree(null, "main", [repo([wt()])])).toBeNull();
  });

  /// **The measurement-driven test.** The branch is NOT part of the key.
  ///
  /// MEASURED over the real corpus: of 206 sessions whose cwd matches a
  /// registered worktree, the recorded branch disagrees with the current
  /// one on 54 (26.2%) -- a main checkout accumulates sessions across
  /// every branch it ever held. Matching on `(path, branch)` would refuse
  /// a quarter of the valid jumps, so this must still match and must
  /// report the divergence instead.
  it("still matches when the worktree has moved to another branch", () => {
    const got = sessionWorktree(
      "/Users/acme/code/widget",
      "stats-dashboard",
      [repo([wt({ path: "/Users/acme/code/widget", branch: "main", is_main: true })])],
    );
    expect(got).not.toBeNull();
    expect(got?.worktree.branch).toBe("main");
    expect(got?.movedOnFrom).toBe("stats-dashboard");
  });

  it("reports no divergence when the branches agree", () => {
    expect(
      sessionWorktree("/Users/acme/code/widget/.worktrees/spoon", "feat/spoon", [repo([wt()])])
        ?.movedOnFrom,
    ).toBeNull();
  });

  it("reports no divergence when either branch is unknown", () => {
    expect(
      sessionWorktree("/Users/acme/code/widget/.worktrees/spoon", null, [repo([wt()])])
        ?.movedOnFrom,
    ).toBeNull();
  });

  /// git's `worktree list` and Claude Code's `cwd` are independent
  /// spellings of the same directory, so a trailing separator or a `.`
  /// segment must not decide whether the jump is offered.
  it("ignores trailing separators and dot segments", () => {
    for (const cwd of [
      "/Users/acme/code/widget/.worktrees/spoon/",
      "/Users/acme/code/widget/./.worktrees/spoon",
      "/Users/acme/code/widget/.worktrees/spoon//",
    ]) {
      expect(sessionWorktree(cwd, "feat/spoon", [repo([wt()])]), cwd).not.toBeNull();
    }
  });

  /// #969: the match RATE used to live in a comment on `ClaudeCodePage`
  /// ("206 of 1,461 -- 14.1% of all sessions, 83.1% of those whose
  /// directory still exists"). A number measured once is right on the day
  /// it is written and decays from then on; it was four points out a day
  /// later, and it is per-machine besides.
  ///
  /// The design does not rest on the rate, it rests on the RULE: a jump is
  /// offered only where the recorded directory is still a registered
  /// worktree, so the majority of a realistic corpus -- whose directories
  /// are deleted when the work lands -- gets no button, and every session
  /// that does get one has a live directory.
  ///
  /// This MEASURES that over a corpus instead of remembering a figure.
  it("offers a jump only for directories that are still registered", () => {
    const live = wt();
    const repos = [repo([live])];
    // A corpus in the shape a real one has: a few live worktrees, and a
    // long tail of agent worktrees deleted when their work landed.
    const cwds = [
      live.path,
      "/Users/acme/code/widget/.worktrees/gone-1",
      "/Users/acme/code/widget/.worktrees/gone-2",
      "/Users/acme/code/widget/.worktrees/gone-3",
      "/Users/acme/code/widget/.worktrees/gone-4",
    ];
    const matched = cwds.filter((c) => sessionWorktree(c, "feat/spoon", repos) !== null);

    // Exactly the registered one, and nothing else -- measured, not recalled.
    expect(matched).toEqual([live.path]);
    // The shape the design rests on: most rows get no button, which is why
    // the section renders a REASON rather than leaving a dead control.
    expect(matched.length).toBeLessThan(cwds.length / 2);
    // And every match is a directory the listing still knows about, which
    // is what makes the offered jump land on a row that exists.
    for (const c of matched) {
      expect(repos[0].worktrees.some((w) => w.path === c)).toBe(true);
    }
  });

  /// Case is PRESERVED, deliberately. Lowercasing would match more often
  /// on macOS and would be wrong on Linux, where these are two
  /// directories -- and a jump to the wrong tree is worse than a missing
  /// button.
  it("does not match paths differing only in case", () => {
    expect(
      sessionWorktree("/Users/acme/code/WIDGET/.worktrees/spoon", "feat/spoon", [repo([wt()])]),
    ).toBeNull();
  });

  it("searches every repository, not only the first", () => {
    const other = repo([wt({ path: "/Users/acme/code/other" })], {
      name: "other",
      path: "/Users/acme/code/other",
    });
    const got = sessionWorktree(
      "/Users/acme/code/widget/.worktrees/spoon",
      "feat/spoon",
      [other, repo([wt()])],
    );
    expect(got?.repoName).toBe("widget");
  });

  /// A registered worktree whose directory was deleted is a real state
  /// (git calls it prunable and `WorktreesPage` renders it), and it is
  /// exactly the row that explains where a session's work went. This
  /// function compares against git's own listing and does not stat, so
  /// such a row still matches.
  it("matches a prunable worktree whose directory is gone", () => {
    const got = sessionWorktree(
      "/Users/acme/code/widget/.worktrees/spoon",
      "feat/spoon",
      [repo([wt({ prunable: "gitdir file points to non-existent location" })])],
    );
    expect(got).not.toBeNull();
    expect(got?.worktree.prunable).toBeTruthy();
  });
});

/// #1137: which agent is working where.
///
/// With ~100 worktrees and parallel agents, the question at the top of
/// the Worktrees page is "which of these is something actively working
/// in right now" -- and the page could not answer it, while the join
/// already existed in the other direction at a measured 83.1% match
/// rate.
describe("worktree occupancy", () => {
  const session = (over: Partial<ClaudeSession> = {}): ClaudeSession =>
    ({
      session_id: "s1",
      name: "Fixing the retry",
      cwd: "/code/widget",
      git_branch: "feat/x",
      last_activity_at: "2026-09-01T10:00:00Z",
      liveness: { state: "running", pid: 42, status: "busy" },
      cwd_state: { state: "exists" },
      kind: { kind: "own" },
      subagents: 0,
      waiting: { state: "no", reason: "never-observed" },
      context_pressure: null,
      ...over,
    }) as ClaudeSession;

  it("finds the running session in a worktree", () => {
    const idx = worktreeSessions([session()]);
    expect(occupancy("/code/widget", [session()], idx)).toEqual({
      kind: "occupied",
      session: expect.objectContaining({ session_id: "s1" }),
    });
  });

  /// A session that ENDED in a directory is not working in it. Treating
  /// it as occupied would refuse removals forever on every worktree that
  /// ever hosted one.
  it("ignores a session that is no longer running", () => {
    const ended = session({ liveness: { state: "not-running", why: "no-process" } as never });
    const idx = worktreeSessions([ended]);
    expect(occupancy("/code/widget", [ended], idx)).toEqual({ kind: "free" });
  });

  /// The load-bearing distinction. "No agent here" and "we could not
  /// tell" must not render the same, and they must not behave the same:
  /// a removal is refused in both, but only one is actionable.
  it("reports unknown when the session list could not be read", () => {
    expect(occupancy("/code/widget", undefined, new Map())).toEqual({ kind: "unknown" });
  });

  /// And an EMPTY list is a real answer, distinct from an absent one.
  it("reports free when the list is empty rather than unread", () => {
    expect(occupancy("/code/widget", [], new Map())).toEqual({ kind: "free" });
  });

  /// The same `normalisePath` as `sessionWorktree`, so the two
  /// directions cannot disagree about one worktree.
  it("matches paths the way the forward join does", () => {
    const s = session({ cwd: "/code/widget/" });
    const idx = worktreeSessions([s]);
    expect(occupancy("/code/widget", [s], idx).kind).toBe("occupied");
  });
});

/// #1137: the refusal, which is the half that protects work.
///
/// A worktree an agent is mid-edit in must not be one-click removable,
/// and neither must one whose occupancy could not be established --
/// "we could not tell" is not "nothing is there".
describe("occupancy and removability", () => {
  const running = (cwd: string): ClaudeSession =>
    ({
      session_id: "s1",
      name: "Fixing the retry",
      cwd,
      git_branch: null,
      last_activity_at: null,
      liveness: { state: "running", pid: 42, status: "busy" },
      cwd_state: { state: "exists" },
      kind: { kind: "own" },
      subagents: 0,
      waiting: { state: "no", reason: "never-observed" },
      context_pressure: null,
    }) as ClaudeSession;

  it("is occupied when a running session is in that directory", () => {
    const list = [running("/code/widget")];
    expect(occupancy("/code/widget", list, worktreeSessions(list)).kind).toBe("occupied");
  });

  it("is free for a worktree no session is in", () => {
    const list = [running("/code/other")];
    expect(occupancy("/code/widget", list, worktreeSessions(list)).kind).toBe("free");
  });

  /// The three-state rule. An unread list yields `unknown`, which the
  /// page refuses on -- distinct from `free`, which it allows.
  it("is unknown when the list could not be read, never free", () => {
    const o = occupancy("/code/widget", undefined, worktreeSessions(undefined));
    expect(o.kind).toBe("unknown");
    expect(o.kind).not.toBe("free");
  });
});

/// #1136: a stopped rebase, merge or cherry-pick.
///
/// Reported as `dirty` before this -- "7 uncommitted files" -- which
/// reads like ordinary edits and is the one state a user must not
/// remove: `git worktree remove` on a half-replayed rebase discards a
/// commit series that exists nowhere else.
describe("an operation in progress", () => {
  const inProgress = (
    op: "rebase" | "merge" | "cherryPick" | "revert" | "bisect",
    conflicts: number | null,
  ): Safety => ({ kind: "in_progress", detail: { op, conflicts } });

  it("names the operation and the conflict count", () => {
    expect(safetyReason(inProgress("rebase", 3))).toBe(
      "rebase in progress — 3 conflicted files",
    );
  });

  it("renders cherryPick as the word git uses", () => {
    expect(safetyReason(inProgress("cherryPick", 1))).toContain("cherry-pick");
  });

  /// An unreadable `git status` is NOT zero conflicts. The operation is
  /// in progress either way, so the count is omitted rather than
  /// rendered as 0 -- which would read as a clean conflict-free rebase.
  it("omits the count when status could not be read", () => {
    expect(safetyReason(inProgress("rebase", null))).toBe("rebase in progress");
  });

  it("is never one-click removable", () => {
    expect(isSafe(inProgress("rebase", 1))).toBe(false);
  });

  /// Red rather than amber: a half-replayed rebase holds commits no
  /// other ref points at, so this is "stop", not "needs attention".
  it("is coloured like never_pushed", () => {
    expect(safetyTone(inProgress("rebase", 1))).toBe(safetyTone({ kind: "never_pushed" }));
  });

  /// The force path names what is lost and what to do instead.
  it("warns about the commits the removal would discard", () => {
    const w = forceWarning(inProgress("rebase", 2));
    expect(w).toContain("rebase");
    expect(w).toMatch(/finish or abort/i);
  });
});

/// #1437: the frontend against what Rust ACTUALLY sends.
///
/// Every test above builds its `Safety` by hand, in this file's own
/// spelling, so the two sides were never compared: Rust sent
/// `in_progress` with its fields under `detail`, this side matched
/// `inProgress`, and every in-progress row read "could not determine:
/// [object Object]". The fixture is Rust's serialisation, pinned by a
/// Rust test, so a rename on either side fails one of the two.
describe("the Safety wire contract", () => {
  const variants = JSON.parse(safetyFixture) as Safety[];

  it("reads a fixture that covers every kind", () => {
    // A floor, so a fixture that stopped parsing into anything could not
    // make the loops below pass vacuously.
    expect(new Set(variants.map((v) => v.kind)).size).toBeGreaterThanOrEqual(15);
  });

  it("describes every variant by name, never as an unreadable payload", () => {
    for (const s of variants) {
      const label = JSON.stringify(s);
      for (const text of [safetyReason(s), forceWarning(s)]) {
        expect(text, label).not.toContain("[object Object]");
        expect(text, label).not.toContain("undefined");
        expect(text, label).not.toContain("unrecognised state");
      }
      // The fallback wording belongs to `unknown` alone: anything else
      // reaching it means this side did not recognise the kind.
      if (s.kind === "unknown") {
        expect(safetyReason(s), label).toContain("could not determine");
      } else {
        expect(safetyReason(s), label).not.toContain("could not determine");
      }
    }
  });

  it("names the operation for every in-progress variant", () => {
    const inProgress = variants.filter((v) => v.kind === "in_progress");
    expect(inProgress.length).toBe(5);
    for (const s of inProgress) {
      expect(safetyReason(s), JSON.stringify(s)).toMatch(
        /^(rebase|merge|cherry-pick|revert|bisect) in progress/,
      );
    }
  });

  /// The default branch, for a kind the backend might add before this
  /// side learns it: say so by name, never stringify the payload.
  it("names an unrecognised kind instead of printing its payload", () => {
    const future = { kind: "some_future_state", detail: { a: 1 } } as unknown as Safety;
    const text = safetyReason(future);
    expect(text).toBe("unrecognised state: some_future_state");
    expect(text).not.toContain("[object Object]");
  });
});
