import type { Branch, Deletable } from "@/types/pr";

/// Where a deletion should happen.
///
/// A tracked branch exists in two places, and #473 was the bug of not
/// asking: it filed tracked branches under "local", deleted the local
/// ref, and left the remote branch alive -- which then reappeared in
/// the list as remote-only, so the user believed the cleanup had
/// worked.
export type Scope = "local" | "remote" | "both";

/// The two backend calls a scope implies, as branch names.
///
/// `local` names go to `deleteBranches`, `remote` names to
/// `deleteRemoteBranches`. Returned separately because they are two
/// different operations against two different things -- one removes a
/// ref here, the other pushes to a remote everyone shares.
export interface Targets {
  local: string[];
  remote: string[];
}

/// Which scopes can apply to this selection.
///
/// Offering a scope with nothing to act on is how a user ends up
/// clicking "remote" and being told nothing happened. A selection of
/// only local-only branches has no remote side at all.
/// A `gone` branch (#1139) has a local side and NO remote side: its
/// upstream is configured but the remote no longer has it, which is
/// exactly what makes it safe to delete locally. Both predicates below
/// already handle it correctly -- `!== "remote"` includes it, and the
/// remote arm requires `tracked` -- and the tests pin that, because the
/// dangerous version of this change would have typed it `tracked` and
/// offered a push against a ref that is not there.
export function scopesFor(selected: Branch[]): Scope[] {
  const hasLocalSide = selected.some((b) => b.location !== "remote");
  // A tracked branch's remote side is its upstream; a remote-only
  // branch IS the remote side.
  const hasRemoteSide = selected.some(
    (b) => b.location === "remote" || (b.location === "tracked" && b.upstream !== null),
  );
  const both = selected.some((b) => b.location === "tracked" && b.upstream !== null);

  const out: Scope[] = [];
  if (hasLocalSide) out.push("local");
  if (hasRemoteSide) out.push("remote");
  if (both) out.push("both");
  return out;
}

/// Split a selection into the names each backend call receives.
///
/// A remote-only branch is named `origin/foo` and that whole string is
/// what `delete_remote` expects. A tracked branch is named `foo`
/// locally and its remote side is its `upstream` -- passing the local
/// name to the remote call would delete the wrong thing or nothing.
export function targetsFor(selected: Branch[], scope: Scope): Targets {
  const local: string[] = [];
  const remote: string[] = [];

  for (const b of selected) {
    const wantLocal = scope === "local" || scope === "both";
    const wantRemote = scope === "remote" || scope === "both";

    if (wantLocal && b.location !== "remote") local.push(b.name);

    if (wantRemote) {
      if (b.location === "remote") remote.push(b.name);
      else if (b.location === "tracked" && b.upstream !== null) remote.push(b.upstream);
    }
  }
  return { local, remote };
}

/// What a scope actually DOES, in one sentence, for a confirmation.
///
/// Lifted out of `BranchesPage`'s `DeleteScopeDialog` in #845, where it
/// was a local `describe` closure. The PR detail view deletes a remote
/// branch too -- `deleteBranch(..., true)` -- and shipped with no
/// confirmation at all, so it needed this sentence and could not reach
/// it. Copying the words into a second component is what lets the two
/// surfaces disagree about the one operation a reflog cannot undo, which
/// is the same argument `scopeLabel` below is here for: the warning is
/// carried by the WORDS, so there has to be exactly one set of them.
///
/// Deliberately phrased for the OPERATION rather than for a selection
/// size, so a single-branch confirmation and a 562-branch one read the
/// same. The remote sentence is the load-bearing one: it is the only
/// scope no local reflog can undo.
export function scopeEffect(scope: Scope): string {
  switch (scope) {
    case "local":
      return "Removes the branch here. Recoverable from the reflog.";
    case "remote":
      return "Pushes a deletion to the remote. Everyone loses it, and no local reflog can undo that.";
    case "both":
      return "Removes it here and on the remote. The remote half cannot be undone.";
  }
}

/// Human wording for a scope, used on the confirm button.
///
/// Remote deletion says so plainly: it pushes to a remote everyone
/// shares and no local reflog can undo it. That warning used to be
/// carried by a separate red button, so it has to be carried by the
/// words now that there is one control.
export function scopeLabel(scope: Scope, t: Targets): string {
  const n = (xs: string[]) => `${xs.length} branch${xs.length === 1 ? "" : "es"}`;
  switch (scope) {
    case "local":
      return `Delete ${n(t.local)} locally`;
    case "remote":
      return `Delete ${n(t.remote)} on the remote`;
    case "both":
      return `Delete ${n(t.local)} locally and on the remote`;
  }
}

/// Why a branch is or is not deletable, in words. (Moved from
/// `BranchesPage.tsx` so that file exports only components, #1965.)
///
/// The whole reason `Deletable` is a tagged union rather than a boolean:
/// "not merged: 3 commits" and "checked out in ../foo" send the user to
/// different places, and a greyed-out checkbox sends them nowhere.
export function deletableReason(d: Deletable): string {
  switch (d.kind) {
    case "merged":
      return d.how === "ancestor" ? "Merged" : "Merged (squashed)";
    case "defaultBranch":
      return "Default branch";
    case "checkedOut":
      return `Checked out in ${d.path}`;
    case "unmerged":
      // Three arms for two states of one number (#967). `.unwrap_or(0)`
      // on the Rust side used to render this verbatim as "Not merged — 0
      // commits not on the default branch": the warning and the
      // reassurance that cancels it, in one sentence, beside a delete
      // checkbox — and on a git older than 2.41 that was EVERY branch.
      //
      // The count is the only thing telling the user what deleting costs,
      // so when it is absent the row says so rather than naming a number.
      // "Not merged" itself is still asserted: that part was measured.
      return d.ahead === null
        ? "Not merged — commit count unavailable (git 2.41 or newer is needed to count)"
        : `Not merged — ${d.ahead} commit${d.ahead === 1 ? "" : "s"} not on the default branch`;
    case "pending":
      return "Checking…";
    case "unknown":
      return d.reason;
  }
}
