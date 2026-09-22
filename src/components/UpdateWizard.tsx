import { useMemo, useState } from "react";
import { toast } from "sonner";
import type { Ecosystem, Outdated } from "@/types/pr";
import { applyUpdatesInBackground } from "@/api/tauri";
import { branchNameError, derivedBranchName, soleEcosystem } from "@/lib/branchName";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/// Ecosystems the backend refuses, with the reason shown to the user.
///
/// Swift is excluded for the same reason the CHECK excludes it: nothing
/// reports what is outdated for Xcode-managed dependencies, so there is
/// no version to move to. Stated in the UI rather than silently omitted,
/// because a package that quietly cannot be selected reads as a bug.
///
/// Must match `packages::apply::supported`. Terraform was missing here
/// while the backend refused it -- so every Terraform row was
/// selectable, counted toward the button, and failed at apply time with
/// an error the user could have been told about before clicking.
const CANNOT_APPLY: Partial<Record<Ecosystem, string>> = {
  swift: "Swift packages must be updated in Xcode or Package.swift.",
  terraform:
    "Terraform provider versions are a constraint in your .tf source, not something a lockfile edit can change.",
  // Cargo was here until #559. It is applicable now because the backend
  // stopped trying to use `cargo add` -- which on a workspace severs
  // `workspace = true` inheritance, edits the wrong crate, and rewrites
  // constraint styles -- and edits the declaring manifest directly.
};

/// Applies dependency updates in a fresh worktree.
///
/// Phase 1: it does NOT push and does NOT open a pull request. The
/// deliverable is a worktree the user inspects themselves, which is
/// deliberate — what these package managers actually do to a checkout is
/// the thing being found out, and adding an irreversible step on top of
/// an unknown is how a small update becomes a bad pull request.
export function UpdateWizard({
  repo,
  packages,
  open,
  onOpenChange,
}: {
  repo: string;
  packages: Outdated[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /// null means "use the derived name". Only a deliberate edit sets it,
  /// so the field keeps tracking the selection until the user takes it
  /// over -- and a name typed then abandoned does not silently persist.
  const [branchEdit, setBranchEdit] = useState<string | null>(null);


  /// Identifies a ROW, not a package.
  ///
  /// This was `ecosystem:name`, which is not unique: one repository
  /// declares the same dependency in several manifests, so every row
  /// for that name shared one checkbox -- ticking one ticked them all,
  /// and React saw duplicate keys. `manifest` is what actually
  /// distinguishes them.
  ///
  /// `project` is part of it too: a monorepo declares the same crate in
  /// two projects under the same manifest NAME (`Cargo.toml
  /// [dependencies]` in both `src-tauri` and `src-mobile`), which
  /// without it is one key for two rows -- the same collision `manifest`
  /// was added to fix, one level up.
  const key = (p: Outdated) =>
    `${p.ecosystem}:${p.project ?? ""}:${p.manifest}:${p.name}`;

  const { applicable, blocked } = useMemo(() => {
    const applicable: Outdated[] = [];
    const blocked: Outdated[] = [];
    for (const p of packages) {
      // A row whose latest EQUALS its current is not an update.
      //
      // `registry::enrich` leaves a provider at `latest == current` with
      // `bump: "unknown"` when its registry lookup fails -- deliberately,
      // so one unreachable host cannot turn a repository into a false
      // all-clear. But the wizard rendered that identically to a real
      // update, offering "2.8.0 → 2.8.0" as something to apply.
      if (p.bump === "unknown" && p.current === p.latest) {
        blocked.push(p);
        continue;
      }
      (CANNOT_APPLY[p.ecosystem] ? blocked : applicable).push(p);
    }
    return { applicable, blocked };
  }, [packages]);

  // The instant the generated name is stamped with (#797), fixed when the
  // dialog opens rather than read on each render.
  //
  // The name now ends in a UTC timestamp, which is what makes two runs in
  // one repository unique -- but a stamp re-read per render would tick
  // visibly in a field the user may be about to edit, and every keystroke
  // elsewhere would move it. Worse, the name shown would not be the name
  // sent: `run` reads this same value, so freezing it is what makes the
  // prediction BINDING.
  //
  // Re-taken on each open, not once per mount, so a dialog left closed
  // for an hour does not stamp its next run with an hour-old time.
  const [openedAt, setOpenedAt] = useState(() => new Date());
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setOpenedAt(new Date());
  }

  const selectedRows = useMemo(
    () => applicable.filter((p) => selected.has(key(p))),
    [applicable, selected],
  );
  const derivedName = useMemo(
    () =>
      derivedBranchName(
        selectedRows.map((p) => p.name),
        soleEcosystem(selectedRows.map((p) => p.ecosystem)),
        openedAt,
      ),
    [selectedRows, openedAt],
  );
  const branchError = branchNameError(branchEdit ?? derivedName);

  const run = () => {
    const requests = selectedRows
      .map((p) => ({
        name: p.name,
        version: p.latest,
        ecosystem: p.ecosystem,
        // Carried through so the backend edits the manifest in THIS
        // project rather than at the worktree root.
        project: p.project ?? "",
      }));
    // Unreachable through the UI -- the button is disabled with an
    // empty selection -- and kept as a guard for any other caller.
    if (requests.length === 0) return;

    // CLOSED on click, not when the run finishes.
    //
    // The run is a package-manager command per package, so a large
    // selection took minutes -- and the modal sat over the app on one
    // unchanging "Applying…" for all of it (#495). The work does not
    // need the dialog: it continues regardless of what is on screen,
    // and the outcome arrives on `update-run-done`.
    onOpenChange(false);
    setSelected(new Set());
    setBranchEdit(null);
    toast.info(`Updating ${requests.length} package${requests.length === 1 ? "" : "s"}…`, {
      description: "A worktree is being prepared and a pull request assembled.",
    });

    // The DERIVED name is sent, not `undefined` (#797).
    //
    // Passing `undefined` let the backend derive its own, which was
    // harmless while the name was a pure function of the package list --
    // both sides computed the same string. It is not harmless now that
    // the name carries a timestamp: two clock reads a second apart give
    // two different names, so the field would show one branch and the run
    // would create another. Sending what was shown makes the prediction
    // binding rather than advisory, and `valid_branch_name` validates it
    // on arrival exactly as it validates a real override.
    applyUpdatesInBackground(repo, requests, branchEdit ?? derivedName).catch((e: unknown) => {
      toast.error("Could not start the update run", {
        description: typeof e === "string" ? e : undefined,
      });
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* `max-w-none` as well as the `w-[min(...)]`: this is the one
          dialog that sizes itself with `w-*` rather than a `max-w-*`,
          so it never overrode `DialogContent`'s default cap and was
          held at 384px instead of the 46rem it asks for -- the
          package rows (name, current, available, a checkbox) were
          rendering in a third of their width. #1306 reported this
          dialog as UNAFFECTED for exactly that reason: it passes no
          `max-w-*`, so nothing looked wrong at the call site. Measured
          in Chrome at 1280px: 384px before, 736px after. */}
      <DialogContent className="max-h-[80vh] w-[min(46rem,92vw)] max-w-none overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Update packages in a worktree</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-[#8b949e]">
          Creates a new worktree, applies the updates there, and leaves it for you to
          review. Nothing is pushed and no pull request is opened.
        </p>

        {(
          <>
            {applicable.length > 1 ? (
              <div className="flex items-center gap-3 border-b border-[#30363d] pb-2 text-xs">
                <button
                  type="button"
                  onClick={() => setSelected(new Set(applicable.map(key)))}
                  className="text-[#4493f8] hover:underline"
                >
                  Select all {applicable.length}
                </button>
                {selected.size > 0 ? (
                  <button
                    type="button"
                    onClick={() => setSelected(new Set())}
                    className="text-[#8b949e] hover:text-[#e6edf3]"
                  >
                    Clear
                  </button>
                ) : null}
              </div>
            ) : null}
            <div className="space-y-1">
              {applicable.map((p) => {
                const k = key(p);
                return (
                  <label
                    key={k}
                    className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 text-sm hover:bg-[#161b22]"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(k)}
                      onChange={(e) => {
                        setSelected((prev) => {
                          const next = new Set(prev);
                          if (e.target.checked) next.add(k);
                          else next.delete(k);
                          return next;
                        });
                      }}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[#e6edf3]">{p.name}</span>
                      {/* Which manifest, so two rows for one dependency
                          are told apart. Without it the list showed the
                          same name twice with different versions and no
                          way to attribute either to a file. */}
                      <span className="block truncate text-[10px] text-[#6e7681]">
                        {p.manifest}
                      </span>
                    </span>
                    <span className="shrink-0 text-xs text-[#8b949e]">
                      {p.current} → {p.latest}
                    </span>
                  </label>
                );
              })}
              {applicable.length === 0 && (
                <p className="text-sm text-[#8b949e]">Nothing here can be updated automatically.</p>
              )}
            </div>

            {/* Stated, not hidden. A package that silently cannot be
                selected reads as a bug in the list. */}
            {blocked.length > 0 && (
              <div className="mt-3 border-t border-[#30363d] pt-3">
                <p className="mb-1 text-xs font-medium text-[#8b949e]">
                  Cannot be updated here
                </p>
                {blocked.map((p) => (
                  <p key={key(p)} className="text-xs text-[#8b949e]">
                    <span className="text-[#e6edf3]">{p.name}</span> —{" "}
                    {CANNOT_APPLY[p.ecosystem] ??
                      "The latest version could not be determined, so there is nothing to update to."}
                  </p>
                ))}
              </div>
            )}

            {/* #409: the name auto-populates and stays overridable.
                It tracks the selection until the user edits it, so a
                field they never touched cannot go stale against what
                they picked. */}
            <label className="mt-4 block text-xs">
              <span className="block text-[#8b949e]">Branch</span>
              <input
                type="text"
                value={branchEdit ?? derivedName}
                onChange={(e) => setBranchEdit(e.target.value)}
                spellCheck={false}
                className="mt-1 w-full rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-xs text-[#e6edf3]"
              />
              {/* Said as it is typed. The backend validates again --
                  this is convenience, that is the gate. */}
              {branchError ? (
                <span className="mt-1 block text-[#f85149]">{branchError}</span>
              ) : null}
            </label>

            <div className="mt-4 flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={() => onOpenChange(false)}
                className="rounded border border-[#30363d] px-3 py-1 text-xs text-[#e6edf3] hover:bg-[#161b22]"
              >
                Cancel
              </button>
              <button
                type="button"
                disabled={selected.size === 0 || branchError !== null}
                onClick={run}
                className="flex items-center gap-1 rounded border border-[#238636]/40 px-3 py-1 text-xs text-[#3fb950] hover:bg-[#238636]/10 disabled:opacity-50"
              >
                {`Apply ${selected.size} update${selected.size === 1 ? "" : "s"}`}
              </button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/// What the run actually did.
///
/// Reports the resolved constraint next to the requested version because
/// they genuinely differ: npm rewrites a pinned `4.17.21` request into
