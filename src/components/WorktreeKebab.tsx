import { Bot, MoreHorizontal, RotateCcw, Trash2, Unlock } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { Worktree } from "../types/pr";
import { forceWarning, isSafe, safetyReason } from "../lib/worktrees";

/// Per-row actions for a worktree.
///
/// The action cell holds ONE button by design — the row is dense, and a
/// fixed width keeps a label swap from reflowing every column (#393).
/// So the further actions live behind a kebab rather than beside the
/// first, the same shape `PrKebab` uses.
///
/// It began as the way BACK from "I read the assessment", which was a
/// one-way door: Claudify was replaced by "Remove anyway…", the mark
/// persisted across restarts, and only a moved branch cleared it. A
/// single exploratory click permanently removed the only way to copy
/// that worktree's prompt.
///
/// #770 gave it the more important job. Removal past the safety gate
/// used to be reachable ONLY from the Claudify toast's "I read the
/// assessment" button, and a toast is for something you can ignore — it
/// leaves on a timer or on a stray click, and nothing about it says it
/// is the only route to the thing you just asked for. The more careful
/// the user was being, the more likely they lost it, and the only way
/// back was to re-run Claudify and spend another agent invocation to
/// recover a button. So the menu is now on EVERY row, and it carries
/// removal on a surface that does not disappear.
export function WorktreeKebab({
  worktree,
  assessed = false,
  onClaudify,
  onCopyClaudify,
  terminalConfigured = false,
  onForget,
  onRemove,
  onForce,
  onUnlock,
}: {
  worktree: Worktree;
  /// This worktree has been handed to Claude Code and the branch has not
  /// moved since. Decides whether the Claudify/Forget pair is offered —
  /// NOT whether removal is, which #770 deliberately decoupled from it.
  assessed?: boolean;
  /// The row's PRIMARY Claudify action, whatever that currently is:
  /// it copies, or -- once a terminal is configured -- it launches one
  /// (#1126). The kebab does not decide which; it calls the same thing
  /// the button does, so the two can never disagree.
  onClaudify: (wt: Worktree) => void;
  /// Copy the command as text, ALWAYS, whatever `onClaudify` does.
  ///
  /// A separate prop rather than a flag on the one above, because with
  /// a terminal configured these are two different actions and the menu
  /// offers both. Copy stays reachable on purpose: the terminal is one
  /// user's choice of one tool, and the raw string is what you need to
  /// paste somewhere else, read before running, or send to someone.
  onCopyClaudify: (wt: Worktree) => void;
  /// Whether a terminal is configured, which decides the LABELS above.
  ///
  /// The menu must say which of the two it does. With a terminal set,
  /// an item reading "Copy the Claudify command" that opened a terminal
  /// would be a button lying about itself -- and the label is the only
  /// thing distinguishing the two items from each other.
  terminalConfigured?: boolean;
  onForget: (wt: Worktree) => void;
  /// The plain, confirmed removal. Only ever called for a row the gate
  /// already considers safe.
  onRemove: (wt: Worktree) => void;
  /// The override, behind the same confirmation the "Remove anyway…"
  /// button opens. It relaxes Headstate's gate, not git's.
  onForce: (wt: Worktree) => void;
  /// Clear the lock, behind a confirmation naming the holder and the
  /// age (#775). Only offered on a locked row, and it removes nothing.
  onUnlock: (wt: Worktree) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onClick);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onClick);
    };
  }, [open]);

  const item =
    "flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs text-[#e6edf3] hover:bg-[#21262d]";
  // Same as `item`, but top-aligned: this one carries a second line
  // under its label, and centring would float the icon against the
  // middle of a two-line block.
  const itemStacked =
    "flex w-full items-start gap-2 rounded px-2 py-1.5 text-left text-xs text-[#e6edf3] hover:bg-[#21262d]";
  // Red, like every other removal affordance on this page. A menu item
  // that deletes a directory must not look like one that copies a
  // string.
  const destructive =
    "flex w-full items-start gap-2 rounded px-2 py-1.5 text-left text-xs text-[#f85149] hover:bg-[#f85149]/10";

  // The gate, read here exactly as the primary button reads it (#770).
  //
  // `isSafe` covers `safe`, `merged_upstream_deleted`, since #819
  // `detached_merged`, and since #1439 `merged_no_upstream`: each means
  // the work is on the default branch and the tree is clean, so each
  // takes the plain confirmed path.
  // Everything else goes through `onForce`, which opens the confirmation
  // naming the specific loss.
  //
  // The detached row needs no wiring of its own here, which is the
  // argument for routing #819 through `isSafe` rather than through a new
  // predicate: a row that is provably contained in the default branch
  // wants the same route for the same reason, and one gate means the
  // menu and the primary button cannot disagree about it (#770).
  //
  // The menu never HIDES removal — that is the entire point of the
  // issue. What varies is which route it takes and what it says first.
  const safe = isSafe(worktree.safety);
  // Locked is offered, and says up front that forcing will not help.
  //
  // The REASON changed in #798 while the behaviour did not. It used to
  // be that `remove_worktree_forced` never passed git's `--force` at
  // all, so forcing a locked tree failed the same way forcing a dirty
  // one did -- an implementation gap, now closed.
  //
  // What remains is a decision. Git wants `--force --force` for a lock
  // and Headstate passes it once, on purpose: the lock is the only
  // mechanism another process has for saying "I am using this
  // directory", and 13 of 34 worktrees on the reporting machine were
  // locked by agents actively working in them. Double-forcing would
  // tear a directory out from under whatever holds it, so the route is
  // an explicit unlock -- the item directly above, behind a
  // confirmation that names the holder and the age (#775).
  //
  // So git still refuses, and without this line the user confirms a
  // destructive-sounding dialog and gets an error. `forceWarning` owns
  // that sentence for #753's confirmation, so it is reused rather than
  // reworded: two copies of a warning about an unrecoverable action are
  // two chances to drift.
  const locked = worktree.safety.kind === "locked";
  // Three rows are left out.
  //
  // An ORPHAN has no repository for git to run in, so it is removed by
  // an entirely different call -- the Rust side deletes the directory
  // after re-checking. The row's own Delete button already offers that,
  // and a second, wrong route from the menu would be worse than none.
  //
  // The MAIN CHECKOUT is never a removal candidate at all. It is the
  // one row on the page where offering this would be a serious bug
  // rather than a widened gate.
  //
  // PRUNABLE is newly out (#793), and the reason is that the route was
  // simply wrong rather than merely redundant. It used to stay in on the
  // grounds that clearing the stale registration is real work that loses
  // nothing -- true -- but the route it took was
  // `remove_worktree_forced`, which runs `git worktree remove` on a
  // directory that is not there. Wrong verb, and since #798 it now runs
  // that wrong verb with `--force`, which is no better.
  //
  // `git worktree prune` is the right verb and #793 made it reachable:
  // one repository-wide affordance in the header, because prune is
  // repo-wide. So this row is not a dead end -- `instead` below sends
  // the user to the action that exists, which is what the old
  // disabled-button state never did.
  const prunable = worktree.safety.kind === "prunable";
  const removable =
    worktree.safety.kind !== "orphaned" && !prunable && !worktree.is_main;

  return (
    <div ref={ref} className="relative shrink-0" onClick={(e) => e.stopPropagation()}>
      <button
        type="button"
        // Falls back to the path for a DETACHED worktree, which has no
        // branch: the label was "More actions for " with nothing after
        // it, so a screen reader user could not tell two such rows
        // apart -- and this menu now removes directories.
        aria-label={`More actions for ${worktree.branch || worktree.path}`}
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => setOpen((o) => !o)}
        className="rounded p-1 text-[#8b949e] hover:bg-[#21262d] hover:text-[#e6edf3]"
      >
        <MoreHorizontal className="h-4 w-4" aria-hidden="true" />
      </button>

      {open ? (
        <div
          role="menu"
          className="absolute right-0 top-full z-20 mt-1 w-56 rounded border border-[#30363d] bg-[#161b22] p-1 shadow-lg"
        >
          {/* Only once an assessment has been marked. Before that the
              row's own button already IS Claudify, and a menu holding a
              duplicate of the button beside it would be noise. */}
          {assessed ? (
            <>
              {/* The whole point: needing the prompt again is normal.
                  The terminal was closed, the paste was lost, the
                  assessment wants rerunning.

                  With a terminal configured this is TWO items, because
                  they are two different actions and one label cannot
                  honestly describe both (#1126). Without one it stays
                  the single item it has always been -- the second would
                  be a duplicate of the first. */}
              {terminalConfigured ? (
                <button
                  type="button"
                  role="menuitem"
                  className={item}
                  onClick={() => {
                    setOpen(false);
                    onClaudify(worktree);
                  }}
                >
                  <Bot className="h-3 w-3" aria-hidden="true" />
                  Open the Claudify command
                </button>
              ) : null}
              <button
                type="button"
                role="menuitem"
                className={item}
                onClick={() => {
                  setOpen(false);
                  // `onCopyClaudify` and not `onClaudify`: with a
                  // terminal configured the latter LAUNCHES, and this
                  // item says "copy".
                  onCopyClaudify(worktree);
                }}
              >
                <Bot className="h-3 w-3" aria-hidden="true" />
                Copy the Claudify command
              </button>
              {/* Re-LOCKS the force-removal path, so it is the safe
                  direction and needs no confirmation. */}
              <button
                type="button"
                role="menuitem"
                className={item}
                onClick={() => {
                  setOpen(false);
                  onForget(worktree);
                }}
              >
                <RotateCcw className="h-3 w-3" aria-hidden="true" />
                Forget the assessment
              </button>
              <div className="my-1 border-t border-[#30363d]" />
            </>
          ) : null}

          {/* Unlock, offered only on a locked row (#775).

              #753 declined this outright, reasoning that a one-click
              button beside a row invites clearing another process's
              claim without reading it. That was right for its evidence
              and the evidence changed: 20 of 44 worktrees on the
              reporting machine are locked, all by one pid that is alive
              only because it is the parent session, and nothing is
              working in any of them. At 45% of the list, withholding
              the remedy does not protect anyone — it leaves a view that
              cannot be used and sends the user to a terminal to do the
              same thing with less information.

              So the care went into the CONFIRMATION rather than into
              refusing: it names the holder, the age, and what is
              underneath. This item only opens it.

              ABOVE removal, and not styled as destructive, because it
              is neither. It deletes nothing and `git worktree lock`
              puts it back — and it is the action that usually helps,
              since 16 of the 18 classifiable locked worktrees measured
              were merged underneath. Painting it red would put a
              reversible action in the same visual class as the one
              unrecoverable action on the page. */}
          {locked ? (
            <button
              type="button"
              role="menuitem"
              className={itemStacked}
              onClick={() => {
                setOpen(false);
                onUnlock(worktree);
              }}
            >
              <Unlock className="mt-0.5 h-3 w-3 shrink-0" aria-hidden="true" />
              <span className="min-w-0">
                Unlock worktree…
                {/* The lock's own line, so the user reads WHAT they are
                    clearing before the dialog rather than only in it.
                    `safetyReason` already leads with the age and says
                    whether the thing underneath is disposable. */}
                <span className="mt-0.5 block text-[#8b949e]">
                  {safetyReason(worktree.safety)}
                </span>
              </span>
            </button>
          ) : null}

          {/* Removal, on a surface that does not disappear (#770).

              The route differs by verdict and the wording says which:
              a safe row goes to the ordinary confirmation, everything
              else to the one that names the specific loss. Neither is
              silent, and neither is hidden -- the menu's job is to make
              a considered human judgement reachable, not to re-litigate
              the gate that already refused. */}
          {removable ? (
            <button
              type="button"
              role="menuitem"
              className={destructive}
              onClick={() => {
                setOpen(false);
                if (safe) onRemove(worktree);
                else onForce(worktree);
              }}
            >
              <Trash2 className="mt-0.5 h-3 w-3 shrink-0" aria-hidden="true" />
              <span className="min-w-0">
                Remove worktree
                {/* The second line is the whole reason a locked row is
                    offered at all rather than greyed out: the user CAN
                    proceed, and needs to know before clicking that the
                    obstacle is git's and not Headstate's. Everything
                    else gets its verdict restated, so the menu item is
                    never a bare "Remove" divorced from the reason the
                    row was refused in the first place. */}
                {safe ? null : (
                  <span className="mt-0.5 block text-[#8b949e]">
                    {locked ? forceWarning(worktree.safety) : safetyReason(worktree.safety)}
                  </span>
                )}
              </span>
            </button>
          ) : null}

          {/* Where a prunable row's action actually lives (#793).

              Not a button, and that is the point. `git worktree prune`
              is repo-wide -- it takes no path -- so a menu item here
              would clear every stale registration in the repository
              while appearing to act on this one row. Naming the header
              affordance instead keeps the scope honest.

              Present at all because the alternative is the dead end the
              issue reported: a greyed Remove button, exclusion from
              every count and from bulk selection, and a kebab that
              offered "Remove anyway…" routed at the wrong verb. Removing
              that wrong route without saying where the right one is
              would have left the row quieter and no more actionable.

              `role="none"` and not a menuitem: it is explanatory text
              inside a menu, and a keyboard user tabbing onto something
              that does nothing is worse than reading it in passing. */}
          {prunable ? (
            <p role="none" className="px-2 py-1.5 text-xs text-[#8b949e]">
              This worktree's directory is already gone. Use “Prune stale
              registrations” above the list — `git worktree prune` is repository-wide,
              so it clears this one along with the rest.
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
