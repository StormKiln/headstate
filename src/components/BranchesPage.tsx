import { useMemo, useState } from "react";
import { Questionnaire } from "@shadcn/react/questionnaire";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useBranchDeleteProgress, useBranchScan, useBranches } from "@/api/hooks";
import { deleteBranches, deleteRemoteBranches } from "@/api/tauri";
import { useActiveFilters } from "@/store/filters";
import type { Branch, Deletable } from "@/types/pr";
import { type Scope, scopeEffect, scopeLabel, scopesFor, targetsFor } from "@/lib/branchDelete";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// Why a branch is or is not deletable, in words.
///
/// The whole reason `Deletable` is a tagged union rather than a boolean:
/// "not merged: 3 commits" and "checked out in ../foo" send the user to
/// different places, and a greyed-out checkbox sends them nowhere.
export function reason(d: Deletable): string {
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

const isDeletable = (b: Branch) => b.deletable.kind === "merged";

/// How long ago, in the coarse terms this decision actually needs.
function ago(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const days = Math.floor((Date.now() - then) / 86_400_000);
  if (days < 1) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

export function BranchesPage() {
  const { repo } = useActiveFilters();
  const { data, isLoading, error } = useBranches(repo ?? undefined);
  // The scan in flight, for the COLD visit. The query is still the
  // authority — `data` replaces these rows the moment it resolves, and
  // nothing streamed ever reaches the delete path.
  const scan = useBranchScan(repo ?? undefined);
  const deleting = useBranchDeleteProgress(repo ?? undefined);
  const qc = useQueryClient();
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [asking, setAsking] = useState(false);

  // Streamed rows are shown ONLY until the scan's own answer arrives,
  // and `data` wins outright when it does. Merging the two would mean
  // a row on screen whose verdict came from neither in full.
  const branches = useMemo(() => data ?? scan.branches, [data, scan.branches]);
  const deletable = useMemo(() => branches.filter(isDeletable), [branches]);

  // What is on screen is incomplete while the query has not resolved.
  // Stated unconditionally rather than inferred from whether rows are
  // still arriving: a stream that DIED stops arriving too, and #701
  // was exactly this class of bug — a partial answer sitting there
  // reading as a complete one.
  const streaming = !data && scan.branches.length > 0;

  // The selected BRANCHES, not their names: deciding where a deletion
  // can happen needs each one's location and upstream, which a name
  // alone does not carry. #473 came from splitting on names.
  const chosen = useMemo(
    () => branches.filter((b) => picked.has(b.name)),
    [picked, branches],
  );
  const scopes = useMemo(() => scopesFor(chosen), [chosen]);

  const toggle = (name: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (!next.delete(name)) next.add(name);
      return next;
    });

  const report = (outcomes: { name: string; error: string | null }[]) => {
    const failed = outcomes.filter((o) => o.error !== null);
    const done = outcomes.length - failed.length;
    if (done > 0) toast.success(`Deleted ${done} branch${done === 1 ? "" : "es"}`);
    // Each refusal names the branch AND the reason. A count alone
    // ("3 failed") tells the user nothing they can act on.
    for (const f of failed) {
      toast.error(`Could not delete ${f.name}`, { description: f.error ?? undefined });
    }
  };

  /// Perform a deletion at the chosen scope.
  ///
  /// "Both" is TWO backend calls against two different things, and
  /// they are reported together but not merged: a tracked branch whose
  /// local ref went and whose remote push failed must say so, or the
  /// user is left believing the same thing #473 made them believe.
  const run = (scope: Scope) => {
    if (!repo) return;
    const t = targetsFor(chosen, scope);
    if (t.local.length === 0 && t.remote.length === 0) return;

    setBusy(true);
    setAsking(false);
    // REMOTE FIRST, and SEQUENTIALLY.
    //
    // These ran concurrently, and for a tracked branch both halves name
    // the same branch -- so the local delete removed the ref while the
    // remote half was still re-checking it, and the remote half then
    // reported "no longer exists" for a deletion that would have
    // worked (#492).
    //
    // Remote leads because it is the half that cannot be undone: if it
    // fails, the local ref is still there to try again from. The
    // reverse order loses the only remaining reference to the commits.
    // SAID, not silent. Every branch is re-checked against a fresh
    // scan before deletion, which is seconds of git on a large
    // repository -- and the page used to sit unchanged throughout, so
    // the first sign anything had happened was a burst of toasts a
    // minute later.
    const total = t.local.length + t.remote.length;
    toast.info(`Deleting ${total} branch${total === 1 ? "" : "es"}…`, {
      description: "Each is re-checked before it is removed.",
    });
    (async () => {
      const remote =
        t.remote.length > 0 ? await deleteRemoteBranches(repo, t.remote) : [];
      const local = t.local.length > 0 ? await deleteBranches(repo, t.local) : [];
      return [...remote, ...local];
    })().then(
      (outcomes) => {
        setBusy(false);
        setPicked(new Set());
        report(outcomes);
        void qc.invalidateQueries({ queryKey: ["branches", repo] });
      },
      (e: unknown) => {
        setBusy(false);
        toast.error("Deletion failed", {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  if (!repo) {
    return <p className="text-sm text-[#8b949e]">Select a repository to see its branches.</p>;
  }
  // Only while NOTHING has arrived. Once the listing frame lands the
  // page renders every row instead, with the verdicts filling in — the
  // listing is one `for-each-ref` and arrives in well under a second,
  // where the classification it precedes takes ten (#657).
  if (isLoading && scan.branches.length === 0) {
    // Named rather than shown as a bare spinner: silence reads as a
    // hang. The wording is now about THIS window, which is the one
    // `for-each-ref` before the listing frame — measured at 257ms on a
    // 582-branch repository, where the whole scan is 10.5s. Promising
    // "around ten seconds" here would describe the wrong wait: the
    // rows arrive almost at once and the verdicts are what take it.
    return (
      <p className="text-sm text-[#8b949e]">
        Reading branches… the list appears first, and each branch’s merge state fills in
        after it. The full check takes around ten seconds on a large repository.
      </p>
    );
  }
  if (error) {
    return (
      <p className="text-sm text-[#f85149]">
        {typeof error === "string" ? error : "Could not read branches for this repository."}
      </p>
    );
  }

  return (
    <div className="space-y-3 text-sm">
      <p className="text-[#8b949e]">
        {branches.length} branch{branches.length === 1 ? "" : "es"} · {deletable.length} merged
      </p>

      {/* The count, and the whole reason the desktop sends a total up
          front. A stream that dies at 47 stops here at "47 of 512" —
          visibly short — where a page fed only verdicts would look
          exactly like one that had received them all. It is a status
          line, not a spinner: it says what is known, so a stalled scan
          is legible rather than silent. */}
      {/* ALWAYS MOUNTED, with the styling on the inner element so an idle
          region draws nothing (#852).

          `StatusBar` states the rule: "`aria-live` on the CONTAINER, which
          is always mounted, rather than on the message, which is not. A
          live region has to exist before the text appears or the first
          announcement is missed -- the one that matters most, since it is
          the one saying work started." This banner was the conditional
          form, and its first announcement is the one that says a
          ten-second classification has begun and which rows cannot be
          acted on yet -- precisely the announcement worth having. */}
      <div role="status">
        {streaming ? (
          <p className="rounded border border-[#9e6a03] bg-[#9e6a03]/10 px-2 py-1 text-xs text-[#d29922]">
            Still scanning — {scan.classified} of {scan.total ?? branches.length} branches
            classified. Rows still marked “Checking…” have no verdict yet and cannot be
            selected for deletion.
          </p>
        ) : (
          ""
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {/* Visible while it runs, not just a disabled button, and it
            says WHICH PHASE (#724).

            A deletion re-checks every branch against a fresh scan
            before touching anything, and that re-check is the slow
            half — ~64ms per branch, so a 562-branch batch spent
            minutes there before the first ref came off. This chip
            previously read "Deleting…" throughout, which was a lie for
            most of the wait and, worse, unchanging: the report was ten
            minutes with no way to tell a slow batch from a hung one.

            So the checking phase says it is checking and counts the
            SCAN's branches, and the deleting phase counts the batch
            and shows refusals as they happen. `busy` alone is the
            fallback for the gap before the first frame arrives. */}
        {/* The progress chip has MOVED, to after the button group (#852).
            See its comment down there: ordering is #817's remedy, and a
            chip whose text grows continuously sat upstream of a button
            that deletes refs. */}
        {/* Selects exactly what ticking every deletable row by hand
            would select -- the same `merged` gate, not a second
            definition that could drift from it.

            Disabled while the scan is still running. "All" over a
            partial list means "all of the ones that happen to have
            arrived", and a control whose label says otherwise is how a
            user ends up believing they have swept a repository they
            have swept half of. Individual merged rows stay selectable
            throughout: those verdicts are each complete. */}
        <button
          type="button"
          disabled={busy || streaming || deletable.length === 0}
          onClick={() => setPicked(new Set(deletable.map((b) => b.name)))}
          title={streaming ? "Available once every branch has been classified" : undefined}
          className="rounded border border-[#30363d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-40"
        >
          Select all {deletable.length} merged
        </button>
        {picked.size > 0 ? (
          <>
            <span className="text-xs text-[#8b949e]">{picked.size} selected</span>
            {/* ONE button. Where the deletion happens is a question the
                modal asks, because a tracked branch exists in two
                places and two side-by-side buttons could not express
                "both" -- they filed it under "local" and left the
                remote branch alive (#473). */}
            <button
              type="button"
              disabled={busy || scopes.length === 0}
              onClick={() => setAsking(true)}
              className="rounded border border-[#30363d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-40"
            >
              Delete {picked.size}…
            </button>
            <button
              type="button"
              onClick={() => setPicked(new Set())}
              className="text-xs text-[#8b949e] hover:text-[#e6edf3]"
            >
              Clear
            </button>
          </>
        ) : null}

        {/* The progress chip, AFTER every button -- #817's remedy applied
            (#852).

            Its text grows continuously and by a lot: "Deleting…" becomes
            "Checking 562 branches before deleting — 317 checked" becomes
            "Deleting 412 of 562 — 30 refused". Upstream of "Delete
            {n}…" in a `flex-wrap` row, each of those widened the row and
            pushed the destructive button sideways or onto a second line.

            #852 grants this was MITIGATED -- the button is `disabled`
            while the chip shows, so it could not be misclicked -- and
            that is exactly why the fix is ordering rather than anything
            cleverer: the defect was never a misclick, it was a control
            that visibly jumps, and `WorktreesPage` settles the general
            case: "nothing upstream of Remove changes width when the
            button appears… by construction rather than by tuning."

            After `Clear` rather than between the buttons, so the whole
            button group is upstream of it and nothing the chip does can
            reach any of them.

            ALWAYS MOUNTED now, holding an empty string when idle, which
            is the other half of #852. `StatusBar` states the rule: "A live
            region has to exist before the text appears or the first
            announcement is missed -- the one that matters most, since it
            is the one saying work started." This `role="status"` element
            was created by the same render that first gave it text, so the
            announcement it existed to make was the one never heard -- on a
            flow that runs `git branch -D` across hundreds of refs and
            takes MINUTES. The border and padding move onto the text, so
            an idle region draws nothing.

            `role="status"` rather than `aria-live="polite"`: it is what
            this element already had, and the two are equivalent here --
            `role="status"` carries an implicit `aria-live="polite"`. */}
        <span role="status" className="text-xs text-[#8b949e]">
          {busy ? (
            <span className="rounded border border-[#30363d] bg-[#161b22] px-2 py-1">
              {deleting === null
                ? "Deleting…"
                : deleting.phase === "checking"
                  ? // Nothing has been deleted yet, and the wording has
                    // to say so: a user who reads "deleting" here and
                    // cancels believes refs are already gone.
                    `Checking ${deleting.total} branch${deleting.total === 1 ? "" : "es"} before deleting — ${deleting.done} checked`
                  : `Deleting ${deleting.done} of ${deleting.total}${
                      // Named while it runs, not saved for the summary. A
                      // batch losing thirty branches to refusals is worth
                      // knowing before the other five hundred go by.
                      deleting.failed > 0 ? ` — ${deleting.failed} refused` : ""
                    }`}
            </span>
          ) : (
            ""
          )}
        </span>
      </div>

      <DeleteScopeDialog
        open={asking}
        onOpenChange={setAsking}
        chosen={chosen}
        scopes={scopes}
        busy={busy}
        onConfirm={run}
      />

      <ul className="space-y-1">
        {branches.map((b) => {
          const can = isDeletable(b);
          return (
            <li
              key={`${b.location}:${b.name}`}
              // `overflow-hidden`, the third part of the #818 remedy
              // (#852): a cell that overflows its share otherwise draws
              // straight through the bordered box.
              className="flex items-center gap-3 overflow-hidden rounded border border-[#30363d] px-3 py-2"
            >
              <input
                type="checkbox"
                aria-label={b.name}
                checked={picked.has(b.name)}
                disabled={!can}
                onChange={() => toggle(b.name)}
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-[#e6edf3]">{b.name}</span>
                  {/* `gone` is styled apart from the other three
                      (#1139). It used to render as "local", which is the
                      OPPOSITE claim: local means the work exists only on
                      this machine, gone means it was pushed, merged and
                      the remote branch cleaned up. One is a reason to
                      keep a branch and the other is the safest thing on
                      the page to delete, and they shared a word.

                      The title names the dead upstream, because "gone"
                      invites "gone from where". */}
                  <span
                    className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] ${
                      b.location === "gone"
                        ? "bg-[#d29922]/15 text-[#d29922]"
                        : "bg-[#21262d] text-[#8b949e]"
                    }`}
                    title={
                      b.location === "gone" && b.upstream
                        ? `${b.upstream} no longer exists on the remote`
                        : undefined
                    }
                  >
                    {b.location}
                  </span>
                </div>
                {/* The reason is shown for EVERY row, not only the
                    blocked ones: "Merged (squashed)" is the evidence
                    that makes a bulk deletion safe to confirm.

                    A pending row is dimmed and italic, so an
                    unanswered check reads as unfinished rather than as
                    a finding. It sits in the same place a verdict will,
                    which is what makes the fill-in legible. */}
                <div
                  className={`text-xs ${
                    can
                      ? "text-[#3fb950]"
                      : b.deletable.kind === "pending"
                        ? "italic text-[#6e7681]"
                        : "text-[#8b949e]"
                  }`}
                >
                  {reason(b.deletable)}
                </div>
              </div>
              {/* The `truncate` here used to be INERT (#852).

                  A `shrink-0` parent is sized to its content, so the
                  inner `truncate` had no narrower box to truncate into --
                  it could never fire, and a long author name simply
                  widened the column and squeezed the branch name and its
                  verdict beside it. The declared intent was right; the
                  mechanism was missing.

                  A bounded width is what makes it real. `max-w-32`
                  rather than a fixed `w-32`: the column is two short
                  lines (a relative date and a username) and is usually
                  narrower than the cap, so a fixed width would hold dead
                  space on every row to bound the occasional long one.
                  `shrink-0` stays -- this is the trailing metadata column
                  and `min-w-0 flex-1` beside it is the name, which is
                  the cell that should keep the room, exactly as #818
                  settled for the worktree row.

                  And a `title` on the author, so the clipped tail
                  survives somewhere: "who last touched this branch" is
                  the question the cell answers, and half a username does
                  not answer it. */}
              <div className="max-w-32 shrink-0 text-right text-xs text-[#8b949e]">
                <div>{ago(b.committed)}</div>
                <div className="truncate" title={b.author}>
                  {b.author}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/// Asks WHERE to delete, rather than making the user pick a button
/// that guesses.
///
/// The scope question exists because a tracked branch is two things.
/// Before this, the view offered "local" and "remote" as separate
/// controls and quietly filed tracked branches under local -- deleting
/// the ref here and leaving the branch on the remote, which then
/// reappeared as remote-only (#473).
function DeleteScopeDialog({
  open,
  onOpenChange,
  chosen,
  scopes,
  busy,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  chosen: Branch[];
  scopes: Scope[];
  busy: boolean;
  onConfirm: (s: Scope) => void;
}) {
  // Defaults to the safest option that applies. Local deletion is
  // recoverable from the reflog; a remote deletion is not, so it is
  // never what a distracted Enter press does.
  const [chosenScope, setScope] = useState<Scope | null>(null);
  // A held choice that no longer applies must not survive: selecting a
  // tracked branch, choosing "both", then changing the selection to
  // remote-only branches would otherwise leave "both" active and send
  // names to a call that cannot use them.
  const scope: Scope =
    chosenScope !== null && scopes.includes(chosenScope)
      ? chosenScope
      : (scopes[0] ?? "local");

  // `scopeEffect` from `lib/branchDelete`, not a local closure (#845).
  //
  // The PR detail view deletes a remote branch too and shipped with no
  // confirmation at all; giving it one meant it needed this sentence,
  // and a copy of it there would let the two surfaces drift about the
  // one operation a reflog cannot undo. Same reason `scopeLabel` lives
  // beside it: the warning is carried by the words.
  const describe = scopeEffect;

  const targets = targetsFor(chosen, scope);
  const label = scopeLabel(scope, targets);
  // Saying so in the confirm state, not only in the option text: with
  // one control instead of two, the words are what carry the warning
  // the separate red button used to.
  const touchesRemote = targets.remote.length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogTitle>
          Delete {chosen.length} branch{chosen.length === 1 ? "" : "es"}
        </DialogTitle>
        <Questionnaire.Root>
          <Questionnaire.Item name="scope" required>
            <Questionnaire.Title className="mt-2 text-sm text-[#e6edf3]">
              Where should these be deleted?
            </Questionnaire.Title>
            <Questionnaire.Choices className="mt-3 space-y-2">
              {scopes.map((s) => (
                <Questionnaire.Choice
                  key={s}
                  value={s}
                  checked={scope === s}
                  onChange={() => setScope(s)}
                  className="flex cursor-pointer items-start gap-2 rounded border border-[#30363d] p-2 hover:bg-[#161b22]"
                >
                  <Questionnaire.ChoiceInput className="mt-1" />
                  <span>
                    <Questionnaire.ChoiceLabel className="block text-sm text-[#e6edf3]">
                      {s === "local"
                        ? "Locally only"
                        : s === "remote"
                          ? "On the remote only"
                          : "Both here and on the remote"}
                    </Questionnaire.ChoiceLabel>
                    <span className="block text-xs text-[#8b949e]">{describe(s)}</span>
                  </span>
                </Questionnaire.Choice>
              ))}
            </Questionnaire.Choices>
          </Questionnaire.Item>
        </Questionnaire.Root>

        <p className="mt-3 text-xs text-[#8b949e]">
          Each branch is re-checked before deletion, so anything that changed since
          the scan is skipped.
        </p>

        <div className="mt-4 flex items-center gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => onConfirm(scope)}
            className={`rounded px-3 py-1 text-xs disabled:opacity-40 ${
              touchesRemote
                ? "border border-[#f85149]/40 text-[#f85149] hover:bg-[#f85149]/10"
                : "border border-[#30363d] text-[#e6edf3] hover:bg-[#21262d]"
            }`}
          >
            {label}
          </button>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="text-xs text-[#8b949e] hover:text-[#e6edf3]"
          >
            Cancel
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
