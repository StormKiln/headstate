import { HardDrive } from "lucide-react";
import type { ArtifactKind } from "@/types/pr";
import { useFilters, useActiveFilters } from "@/store/filters";
import { useArtifacts, useArtifactSizes, useVenvs, useVenvSizes } from "@/api/hooks";
import { current } from "@/lib/ariaCurrent";
import { NarrowQueryError } from "./QueryError";
import { ViewSwitcher } from "./ViewSwitcher";
import { formatSize } from "@/lib/worktrees";

/// The venv "kind", which is not an `ArtifactKind` -- virtualenvs are a
/// tool cache rather than build output, and giving them a fake kind in
/// that enum would put them behind the manifest rules that decide what
/// counts as an artifact.
export const VENV_GROUP = "poetry-venvs";

/// A group id: either a build-artifact kind or the venv group.
export type ArtifactGroup = ArtifactKind | typeof VENV_GROUP;

export const GROUP_LABEL: Record<ArtifactGroup, string> = {
  cargo_target: "Rust targets",
  node_modules: "Node modules",
  terraform: "Terraform providers",
  dotnet_build: ".NET build output",
  build_output: "Build output",
  [VENV_GROUP]: "Poetry virtualenvs",
};

/// One sidebar entry per kind actually FOUND, largest first.
///
/// Dynamically populated rather than a fixed list: a machine with no
/// Terraform should not have a Terraform entry that opens an empty page,
/// and the ordering puts the biggest reclaimable group where the eye
/// starts.
export function ArtifactSidebar({ reviewingCount }: { reviewingCount: number }) {
  const filters = useActiveFilters();
  const { setFilter } = useFilters();
  // Reads the SAME query keys the page does, so both render from one
  // fetch rather than the sidebar triggering a second scan. Passing the
  // data down from App instead would mean lifting the whole artifacts
  // state out of the page for one label.
  // `isLoading` and `isError` on BOTH scans (#846).
  //
  // This column had zero loading or error handling: the `= []` defaults
  // made a rejected scan produce an empty `groups` map, so the sidebar
  // rendered one row -- "Everything" -- and nothing else. Identical to a
  // machine with no build output and no virtualenvs, and on a
  // disk-cleanup tool that reads as "your machine is clean" when the
  // truth is it could not look.
  //
  // Both scans, because this column is the only place the two appear
  // together: the groups are artifact kinds PLUS the venv group, so a
  // venv failure silently removes a row the page can still show and an
  // artifact failure silently removes five. One arm reports whichever
  // failed, by name, so the missing rows are accounted for rather than
  // merely absent.
  const {
    data: artifacts = [],
    isLoading: artifactsLoading,
    isError: artifactsFailed,
    refetch: refetchArtifacts,
  } = useArtifacts(true);
  const {
    data: venvs = [],
    isLoading: venvsLoading,
    isError: venvsFailed,
    refetch: refetchVenvs,
  } = useVenvs(true);
  const { sizes } = useArtifactSizes(artifacts, artifacts.length > 0);
  const { sizes: venvSizes } = useVenvSizes(venvs, venvs.length > 0);

  const groups = new Map<ArtifactGroup, { count: number; bytes: number }>();
  for (const a of artifacts) {
    const g = groups.get(a.kind) ?? { count: 0, bytes: 0 };
    g.count += 1;
    g.bytes += sizes.get(a.path) ?? 0;
    groups.set(a.kind, g);
  }
  if (venvs.length > 0) {
    groups.set(VENV_GROUP, {
      count: venvs.length,
      bytes: venvs.reduce((n, v) => n + (venvSizes.get(v.path) ?? 0), 0),
    });
  }

  // Largest first, so the group worth acting on leads. Ties break on
  // label so the order is stable across rescans rather than shuffling
  // as sizes arrive.
  const entries = [...groups.entries()].sort(
    ([ka, a], [kb, b]) => b.bytes - a.bytes || GROUP_LABEL[ka].localeCompare(GROUP_LABEL[kb]),
  );

  const rowClass = (active: boolean) =>
    `flex w-full items-center justify-between rounded px-3 py-2 text-sm ${
      active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
    }`;

  // `current` from `@/lib/ariaCurrent` (#852, shared in #977): these rows
  // SCOPE the artifacts view rather than navigating to a different page.

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={{ "to-review": reviewingCount }} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <button
          type="button"
          onClick={() => setFilter("repo", undefined)}
          aria-current={current(!filters.repo)}
          className={rowClass(!filters.repo)}
        >
          <span className="flex items-center gap-2">
            <HardDrive className="h-4 w-4 shrink-0" aria-hidden="true" />
            Everything
          </span>
        </button>
        {/* BETWEEN "Everything" and the groups, which is where the
            missing rows would have been (#846).

            Names WHICH scan failed rather than saying "something went
            wrong": the two feed different rows, and a user told only that
            a scan failed cannot tell whether the virtualenv group is
            absent because it failed or because there are none. `failed`
            is checked before `loading` because a retry leaves both true
            for a moment, and flipping to "Looking…" mid-retry would read
            as the error having resolved itself. */}
        {artifactsFailed || venvsFailed ? (
          // The message names WHICH scan failed, and the retry re-runs only
          // what actually failed. `NarrowQueryError` takes both as props for
          // exactly that reason -- re-running a scan that succeeded would
          // throw away a result the page is still rendering, and the
          // virtualenv scan is 26 seconds.
          <NarrowQueryError
            message={`Could not scan for ${
              artifactsFailed && venvsFailed
                ? "build output or virtualenvs"
                : artifactsFailed
                  ? "build output"
                  : "virtualenvs"
            }.`}
            onRetry={() => {
              if (artifactsFailed) void refetchArtifacts();
              if (venvsFailed) void refetchVenvs();
            }}
          />
        ) : artifactsLoading || venvsLoading ? (
          // A HOLDING message, not a diagnosis. An empty group list
          // before the scans answer is "we have not looked yet", and
          // saying anything about the machine's contents here would be
          // the `RepoPickerSidebar` mistake.
          <p className="px-3 py-2 text-xs text-[#8b949e]">Looking for reclaimable space…</p>
        ) : null}
        {entries.map(([kind, { count, bytes }]) => (
          <button
            type="button"
            key={kind}
            onClick={() => setFilter("repo", kind)}
            aria-current={current(filters.repo === kind)}
            className={rowClass(filters.repo === kind)}
          >
            <span className="truncate">{GROUP_LABEL[kind]}</span>
            {/* The SIZE, not the count: this view exists to reclaim
                space, and 3 directories holding 61 GB matter more than
                90 holding 2. */}
            <span className="ml-2 shrink-0 tabular-nums">
              {bytes > 0 ? formatSize(bytes) : count}
            </span>
          </button>
        ))}
      </div>
    </nav>
  );
}
