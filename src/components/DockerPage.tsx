import { isCancelled } from "@/lib/cancelled";
import { ActingOnDesktop } from "./ActingOnDesktop";
import { useState } from "react";
import { toast } from "sonner";
import {
  useDockerDiskUsage,
  useDockerImages,
  useDockerState,
  useDockerVolumes,
  usePruneCache,
  useRemoveImages,
  useDockerBuilds,
  useRemoveVolume,
} from "../api/hooks";
import { dockerRestart, dockerRunningContainers, dockerStart } from "../api/tauri";
import {
  formatDockerSize,
  imageName,
  imageState,
  imageTone,
  isStale,
  isSuperseded,
} from "../lib/docker";
import { HelpButton } from "./HelpButton";
import {
  buildForImage,
  cachePercent,
  cacheTone,
  formatDuration,
  recentCacheHealth,
} from "../lib/buildJoin";
import { relativeTime } from "../lib/time";
import { useIsMobile } from "../lib/useIsMobile";

import type { DanglingVolume, DockerBuild, DockerImage } from "../types/pr";
import { QueryError, errorMessage } from "./QueryError";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// One line of the disk summary.
///
/// Declared here rather than inside `DiskSummary`: a component created
/// during render is a new type on every pass, so React remounts it and
/// resets its state each time.
function UsageRow({
  label,
  size,
  extra,
}: {
  label: string;
  size: number;
  extra?: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline gap-2">
      <span className="w-28 shrink-0 text-[#8b949e]">{label}</span>
      <span className="tabular-nums">{formatDockerSize(size || null)}</span>
      {extra}
    </div>
  );
}

/// Where the disk actually went.
///
/// Images are only part of it: measured on a real machine, 17.35GB of
/// images sat beside 4.65GB of build cache and a 4.74GB orphaned volume.
/// A page that listed only images would leave most of the waste
/// invisible, so the summary leads.
function DiskSummary({
  onPrune,
  builds,
  buildsFailed,
}: {
  onPrune: () => void;
  /// For the cache-health figure. Empty is ordinary -- a machine that
  /// has never built anything has no health to report, and the row
  /// simply omits it.
  ///
  /// True of a genuinely empty history and FALSE of a rejected one, which
  /// is why `buildsFailed` rides alongside (#963).
  builds: DockerBuild[];
  /// Whether the build-history call REJECTED, as opposed to answering
  /// with nothing.
  ///
  /// The two have opposite meanings and the same `[]`: one says the
  /// cache has no health to report, the other says we could not ask. The
  /// figure this row exists to show is, in this module's own words, "the
  /// number the Builds page existed to show", so it must not vanish with
  /// nothing said.
  buildsFailed: boolean;
}) {
  const { data: du, isError } = useDockerDiskUsage(true);
  const cacheHealth = recentCacheHealth(builds);
  if (isError) {
    // Vanishing silently is a wrong answer by omission: this panel leads
    // the page and is the argument for the feature existing.
    return (
      <div className="rounded-md border border-[#30363d] p-3 text-xs text-[#8b949e]">
        Could not read Docker disk usage.
      </div>
    );
  }
  if (!du) return null;

  return (
    <div className="rounded-md border border-[#30363d] p-3 text-xs">
      <UsageRow label="Images" size={du.images_bytes} />
      <UsageRow
        label="Build cache"
        size={du.build_cache_bytes}
        extra={
          <>
            {/* The number the Builds page existed to show, next to the
                cache it describes. A cold build is not a problem in
                itself; a target that USED to be warm and is not any
                more means something invalidated the cache. Sitting
                beside "clear" also makes the trade legible -- clearing
                is what turns this number cold. */}
            {/* The ERROR arm first, ALWAYS before the empty arm (#846,
                #963). `cacheHealth` is `null` on a rejection too -- the
                builds list is left at its `[]` default -- so without this
                ordering the figure simply disappeared and the reader was
                told nothing, which invites reading the last number they
                saw as still true.

                A one-line note in the cache-health SLOT rather than an
                error panel: the disk figures beside it are independently
                healthy and this is the partial-answer case. No retry, for
                `useArtifacts`' reason -- `retry: false` is right for these
                local calls, and the missing `buildx` plugin a second call
                would find is the same one it found first. */}
            {buildsFailed ? (
              <span role="status" className="text-[#8b949e]">
                Could not read the build history
              </span>
            ) : cacheHealth ? (
              <span
                className={cacheTone(cacheHealth.percent)}
                title={`Across the last ${cacheHealth.count} builds, weighted by steps`}
              >
                {cacheHealth.percent}% cached
              </span>
            ) : null}
            {!buildsFailed && cacheHealth ? (
              <HelpButton topic="build-cache" />
            ) : null}
            {du.build_cache_bytes > 0 ? (
              <button
                type="button"
                onClick={onPrune}
                // Still a text link rather than a red button, and the
                // ellipsis is what changed (#852).
                //
                // Recoverable, so dressing it in the red reserved for the
                // unrecoverable actions on this page would teach the user
                // to discount both -- the argument `WorktreesPage` makes
                // about its unlock dialog. What WAS wrong is that a bare
                // link fired it immediately: `reclaim.rs` says "Cache is
                // not waste, it is speed: real build durations went
                // 7m8s -> 1m20s -> 56.9s as the cache warmed… this is a
                // trade the user must make knowingly", and a link you
                // cannot un-click is not a knowing trade.
                //
                // The ellipsis says a question comes first, as everywhere
                // else in the app that a control opens a dialog.
                className="text-[#58a6ff] hover:underline"
              >
                clear…
              </button>
            ) : null}
          </>
        }
      />
      <UsageRow label="Volumes" size={du.volumes_bytes} />
    </div>
  );
}

function ImageRow({
  img,
  onRemove,
  removing,
  builds,
  buildsFailed,
}: {
  img: DockerImage;
  onRemove: (img: DockerImage) => void;
  removing: boolean;
  /// Every known build, for the commit join. Empty is ordinary -- a
  /// project that tags by version rather than by commit matches nothing
  /// here.
  ///
  /// Ordinary for an EMPTY history, not for a rejected one (#963), which
  /// is why `buildsFailed` rides alongside: an expanded row showing no
  /// provenance is indistinguishable from an image that genuinely has
  /// none.
  builds: DockerBuild[];
  /// Whether the build-history call rejected rather than answering.
  buildsFailed: boolean;
}) {
  const [open, setOpen] = useState(false);
  // Only once the row is expanded: the join is cheap, but the builds
  // themselves are not fetched at all unless something needs them.
  const build = open ? buildForImage(img, builds) : null;
  // Computed once rather than three times in the JSX below, and as
  // `number | null` so the absent case is a branch rather than a
  // fabricated 0 (#963).
  const cached = build === null ? null : cachePercent(build);
  // `repository:tag`, not the tag alone.
  //
  // Docker reports Repository and Tag separately and `tags` holds only
  // the TAG, so a row for `<registry>/octocat-api:latest` used to render
  // as `latest`. On a machine with 31 images that is 31 rows saying
  // `latest` with nothing to tell them apart -- and the repository, the
  // one part that says what the image IS, was on the type the whole
  // time. This fix previously landed only on the untagged branch.
  const name = imageName(img);
  // The cells, built once. The desktop lays them out on one line; the
  // phone puts the name beside its action and the state, age and size
  // on a line beneath.
  const isMobile = useIsMobile();
  const nameCell = (
      /* The whole row toggles, so the hit target is the row rather
          than a chevron the user has to aim at. */
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="min-w-0 flex-1 truncate text-left font-mono text-[#e6edf3] hover:text-white"
      >
        {name}
        {img.tags.length > 1 ? (
          <span className="ml-2 text-xs text-[#8b949e]">+{img.tags.length - 1} more</span>
        ) : null}
        {/* The commit subject is what makes an eight-hex tag mean
            something. Without it the row is a hash and a size. */}
        {img.origin ? (
          <span className="ml-2 text-xs text-[#8b949e]">{img.origin.subject}</span>
        ) : null}
      </button>
  );
  const facts = (
    <>
      <span className={`shrink-0 text-xs ${imageTone(img)}`}>{imageState(img)}</span>
      {img.created ? (
        <span className="shrink-0 text-xs text-[#8b949e]">{relativeTime(img.created)}</span>
      ) : null}
      <span className={isMobile ? "shrink-0 tabular-nums text-xs text-[#8b949e]" : "w-20 shrink-0 text-right tabular-nums text-xs text-[#8b949e]"}>
        {formatDockerSize(img.size_bytes)}
      </span>
    </>
  );
  const removeButton = (
      <button
        type="button"
        disabled={img.in_use !== false || removing}
        onClick={() => onRemove(img)}
        title={
          img.in_use === true
            ? "A running container is using this image"
            : img.in_use === null
              ? "Could not check whether a container is using this image"
              : "Remove this image"
        }
        className={`shrink-0 rounded border px-2 py-0.5 text-xs ${
          img.in_use !== false || removing
            ? "border-[#30363d] text-[#8b949e] opacity-50"
            : "border-[#f85149]/40 text-[#f85149] hover:bg-[#f85149]/10"
        }`}
      >
        {removing ? "Removing…" : "Remove"}
      </button>
  );

  return (
    <div className="border-b border-[#30363d] px-4 py-2.5 text-sm last:border-b-0">
      {isMobile ? (
        <div className="flex flex-col gap-1">
          <div className="flex items-baseline gap-3">
            {nameCell}
            {removeButton}
          </div>
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">{facts}</div>
        </div>
      ) : (
      <div className="flex items-baseline gap-3">
      {nameCell}
      {facts}
      {removeButton}
      </div>
      )}
      {/* The long fields, kept out of the collapsed row. The problem
          was never density -- it was that the identifying detail had
          nowhere to live. */}
      {open ? (
        <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 border-t border-[#30363d] pt-2 text-xs text-[#8b949e]">
          <dt>Image ID</dt>
          <dd className="font-mono">{img.id}</dd>
          <dt>Repository</dt>
          <dd className="font-mono">{img.repository}</dd>
          {img.tags.length > 0 ? (
            <>
              <dt>Tags</dt>
              <dd className="font-mono">{img.tags.join(", ")}</dd>
            </>
          ) : null}
          {img.created ? (
            <>
              <dt>Created</dt>
              <dd>{img.created}</dd>
            </>
          ) : null}
          {img.origin ? (
            <>
              <dt>Commit</dt>
              <dd className="font-mono">
                {img.origin.commit} — {img.origin.subject}
              </dd>
              <dt className="inline-flex items-center">
                Built from
                {/* Empty provenance looks identical whether the image
                    was pulled or the app is looking in the wrong
                    place -- which it was, silently, until #344. */}
                <HelpButton topic="image-provenance" />
              </dt>
              <dd className="font-mono">{img.origin.context ?? img.origin.repo_path}</dd>
              <dt>Branch</dt>
              <dd>{img.origin.merged ? "merged" : "still open"}</dd>
            </>
          ) : (
            <>
              <dt>Origin</dt>
              {/* Says WHY there is nothing to show. "unknown" alone
                  reads as a missing feature rather than a fact about
                  this image. */}
              <dd>
                could not be traced to a commit — nothing in the local repos matched
                its tag
              </dd>
            </>
          )}
          {/* The Builds page folded in here (#326).
              
              A build's duration and cache ratio are facts about the
              IMAGE it produced, and on their own page they were a log
              nobody could act on. Matched on the commit: `buildx
              history` records a VCS Revision and these images are
              tagged with it, so the key was already on both sides --
              no new field was needed.
              
              Absent freely: a project that tags by version rather than
              by commit matches nothing here, which is ordinary rather
              than an error. */}
          {/* ERROR arm before the absent arm (#963). A rejected history
              leaves `builds` empty, so `build` is `null` for every image
              and the row showed no provenance at all -- identical to an
              image that genuinely has none, which is what the "Absent
              freely" note above is about and which is true only of a
              history that ANSWERED. */}
          {buildsFailed ? (
            <>
              <dt>Build</dt>
              <dd className="text-[#8b949e]">Build history could not be read</dd>
            </>
          ) : build ? (
            <>
              <dt>Build</dt>
              <dd>
                {formatDuration(build.duration_secs)}
                {/* The cache ratio only when there IS one (#963).
                    `cachePercent` now returns `null` for a build whose
                    step counts buildx did not report, instead of the `0`
                    that read as "nothing was cached" -- the alarm this
                    figure exists to raise. */}
                {cached !== null ? (
                  <span className={`ml-2 ${cacheTone(cached)}`}>{cached}% cached</span>
                ) : null}
              </dd>
            </>
          ) : null}
        </dl>
      ) : null}
    </div>
  );
}

export function DockerPage() {
  const { data: state } = useDockerState();
  const up = state?.kind === "running";
  const { data: images, isLoading, isError, error, refetch } = useDockerImages(up);
  const { data: volumes } = useDockerVolumes(up);
  const removeImages = useRemoveImages();
  // For the build join on an expanded row (#326). Fetched with the
  // page rather than per row: one listing serves every image, and the
  // Builds page it replaces fetched exactly this.
  //
  // `isError` DESTRUCTURED, not discarded (#963). This was the literal
  // `= []` shape of #846: `docker buildx history ls` fails for ordinary
  // reasons on somebody else's machine -- the `buildx` plugin absent or
  // too old (it is a plugin, not part of the daemon), the history feature
  // disabled, a permission wall on the socket, the 20s ceiling in
  // `docker/cli.rs` -- and an empty array was then read as a MEASUREMENT:
  // "this machine has never built anything". `DiskSummary` one query over
  // already handles its own `isError` correctly and states why: "Vanishing
  // silently is a wrong answer by omission."
  const { data: builds = [], isError: buildsFailed } = useDockerBuilds(up);
  const removeVolume = useRemoveVolume();
  const prune = usePruneCache();

  const [pending, setPending] = useState<DockerImage | null>(null);
  /// The volume awaiting confirmation, or null.
  ///
  /// A Dialog rather than `window.confirm`: the native dialog does not
  /// work in the Tauri webview -- it returned falsy, so the handler took
  /// its early return and the click did nothing at all, with no error.
  /// Every other destructive path on this page already uses Dialog.
  const [pendingVolume, setPendingVolume] = useState<DanglingVolume | null>(null);
  /// Whether the bulk confirmation is open.
  ///
  /// A BOOLEAN, not `null | "stale" | "superseded"` (#852). It used to name
  /// which set the dialog was for, and that is what came apart: `"stale"`
  /// was written at its only call site, so the `"superseded"` member was
  /// unreachable, while `bulkSet` followed the checkbox -- and the dialog's
  /// title and reassurance keyed off this rather than off the expression
  /// that decides the membership.
  ///
  /// Narrowed rather than left in place once the copy moved to
  /// `includeWider`, for the reason the store's `panel` axis was removed in
  /// the same issue: "a value still in the union is one a component can
  /// set", so an unreachable member is a trap for the next caller who
  /// believes setting it does something. WHICH set is being removed is
  /// `includeWider`'s answer and only ever was; this is just open or closed.
  const [bulkOpen, setBulkOpen] = useState(false);
  // Whether the confirmation is showing the WIDE set. Reset each time
  // the dialog opens, so a previous session's choice cannot silently
  // widen a later removal.
  const [includeWider, setIncludeWider] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [restartOpen, setRestartOpen] = useState<string[] | null>(null);
  /// Whether the build-cache prune is awaiting confirmation (#852).
  ///
  /// This page asserts the invariant it was breaking, at `pendingVolume`
  /// just above: "Every other destructive path on this page already uses
  /// Dialog." The cache prune was reached from a bare text link that fired
  /// `prune("168h")` on click.
  ///
  /// Recoverable, so not data loss -- but `reclaim.rs` is explicit about
  /// the cost: "Cache is not waste, it is speed: real build durations went
  /// 7m8s -> 1m20s -> 56.9s as the cache warmed… this is a trade the user
  /// must make knowingly." A link you cannot un-click does not let anyone
  /// make a trade knowingly, which is a miscalibrated affordance rather
  /// than a missing safety gate -- and the remedy for both is the same.
  const [pruning, setPruning] = useState(false);

  // Docker being off is NORMAL, not an error. An empty image list would
  // say "your machine is clean" when the truth is we could not ask.
  if (state && !up) {
    const notInstalled = state.kind === "not_installed";
    // Unknown carries the REAL message -- a 20s timeout, a broken
    // context -- and used to fall into the "not running" branch,
    // discarding the detail and offering a Start button that cannot
    // help because Docker is already running.
    //
    // A refused socket is NOT one of those: `cli.rs` classifies it as
    // `PermissionDenied` before `Unknown` is ever reached, and its fix
    // is nothing like "start Docker", so it gets its own screen.
    //
    // This branch used to ALSO run `/permission denied/i` over
    // `state.detail` and print the docker-group remedy a second time
    // (#1230). That regex was a discarded type guessed back from prose:
    // the variant it was re-deriving already existed one branch up,
    // produced by `is_permission_denied` in `docker/cli.rs`. Two
    // classifiers for one distinction is how they drift -- and this
    // pair already had: `is_permission_denied` requires the message to
    // name docker as well ("permission denied: /etc/shadow" is not
    // Docker's, and its own test pins that), where the bare regex here
    // did not, so the remedy could be printed for a permission error
    // that had nothing to do with the docker group.
    if (state.kind === "permission_denied") {
      return (
        <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
          <p className="text-sm font-semibold text-[#e6edf3]">Docker refused the connection</p>
          <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
            Your user is probably not in the <code>docker</code> group. Run{" "}
            <code>sudo usermod -aG docker $USER</code>, then log out and back in.
          </p>
        </div>
      );
    }
    if (state.kind === "unknown") {
      return (
        <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
          <p className="text-sm font-semibold text-[#e6edf3]">Could not talk to Docker</p>
          <p className="mx-auto mt-2 max-w-xl break-words text-sm text-[#8b949e]">
            {state.detail}
          </p>
        </div>
      );
    }
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
        <p className="text-sm font-semibold text-[#e6edf3]">
          {notInstalled ? "Docker was not found" : "Docker is not running"}
        </p>
        <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
          {notInstalled
            ? "Install Docker Desktop, or set HEADSTATE_DOCKER to the docker binary."
            : "Start it to see images and reclaim disk space."}
        </p>
        {!notInstalled ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              dockerStart().then(
                () => {
                  setBusy(false);
                  toast.success("Starting Docker…");
                },
                (e: unknown) => {
                  setBusy(false);
                  toast.error("Could not start Docker", {
                    description: typeof e === "string" ? e : undefined,
                  });
                },
              );
            }}
            className="mt-4 rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22] disabled:opacity-50"
          >
            {busy ? "Starting…" : "Start Docker"}
          </button>
        ) : null}
      </div>
    );
  }

  const shown = images ?? [];
  const stale = shown.filter(isStale);
  // Every superseded image we can prove is unused -- a SUPERSET of
  // `stale`, and on a real machine a much larger one: `isStale` also
  // requires that the image was attributed to a branch and that the
  // branch merged, which excludes everything nothing could attribute.
  const superseded = shown.filter(isSuperseded);
  // What the confirmation is actually about.
  const bulkSet = includeWider ? superseded : stale;
  const bulkBytes = bulkSet.reduce((n, i) => n + i.size_bytes, 0);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <span className="inline-flex items-center font-semibold">
          Images
          {/* Two near-identical words decide what a bulk removal takes,
              and the difference was already reported as unreadable. */}
          <HelpButton topic="image-states" />
        </span>
        <span className="text-[#8b949e]">{shown.length}</span>
        {stale.length > 0 ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setIncludeWider(false);
              setBulkOpen(true);
            }}
            className="rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10 disabled:opacity-50"
          >
            Remove {stale.length} stale image{stale.length === 1 ? "" : "s"}
          </button>
        ) : null}
        {/* ONE entry point, not two. Measured on a real machine:
            18 stale against 19 superseded -- two buttons one apart in
            count, doing near-identical things. The wide set is now an
            opt-in inside the confirmation, where the images are already
            listed individually, rather than a competing button whose
            difference the reader had to work out by subtraction. */}
        <button
          type="button"
          // The rejection branch matters rather than being a formality,
          // which is why `no-misused-promises` flagging this was worth
          // acting on (#892). Listing what a restart would stop is how
          // this button asks for confirmation; if the query fails the
          // dialog silently never opens and the click reads as dead, so
          // the failure has to be said out loud. Reported the way every
          // other failure on this page is.
          onClick={() => {
            dockerRunningContainers().then(setRestartOpen, (e: unknown) => {
              toast.error("Could not check what is running", {
                description: typeof e === "string" ? e : undefined,
              });
            });
          }}
          className="ml-auto text-xs text-[#8b949e] hover:text-[#e6edf3]"
        >
          Restart Docker
        </button>
      </div>

      {/* ASKS, rather than pruning (#852). The handler is now `setState`;
          the pruning itself moved into the dialog's confirm button. */}
      <DiskSummary builds={builds} buildsFailed={buildsFailed} onPrune={() => setPruning(true)} />

      <div className="rounded-md border border-[#30363d]">
        {isLoading ? (
          <div className="px-4 py-12 text-center text-sm text-[#8b949e]">Reading images…</div>
        ) : isError ? (
          // NOT an empty list. On a disk-cleanup tool "No images." reads
          // as "your machine is clean" when the truth is we could not
          // ask -- the exact pattern this codebase forbids.
          <QueryError
            title="Could not read Docker images"
            message={errorMessage(error)}
            onRetry={() => void refetch()}
          />
        ) : shown.length === 0 ? (
          <div className="px-4 py-12 text-center text-sm text-[#8b949e]">No images.</div>
        ) : (
          shown.map((img) => (
            <ImageRow
              key={img.id}
              img={img}
              removing={removing === img.id}
              onRemove={setPending}
              builds={builds}
              buildsFailed={buildsFailed}
            />
          ))
        )}
      </div>

      {volumes && volumes.length > 0 ? (
        <div className="rounded-md border border-[#30363d] p-3 text-xs">
          <p className="mb-2 font-semibold text-[#e6edf3]">Volumes attached to nothing</p>
          {/* Individually, never bulk: a dangling volume can hold a
              database someone still wants. A wrongly deleted image costs
              a rebuild; this costs data. */}
          {volumes.map((v) => (
            <div key={v.name} className="flex items-baseline gap-3 py-0.5">
              <span className="flex-1 truncate font-mono">{v.name}</span>
              <span className="tabular-nums text-[#8b949e]">{formatDockerSize(v.size_bytes || null)}</span>
              <button
                type="button"
                onClick={() => setPendingVolume(v)}
                className="rounded border border-[#f85149]/40 px-2 py-0.5 text-[#f85149] hover:bg-[#f85149]/10"
              >
                Remove
              </button>
            </div>
          ))}
        </div>
      ) : null}

      {/* The build-cache prune's confirmation (#852).

          Deliberately NOT styled as a destructive confirmation -- no red
          title, no `#da3633` confirm. Nothing is lost that cannot be
          rebuilt, and dressing a recoverable action in the colour reserved
          for the unrecoverable ones would teach the user to discount both;
          `WorktreesPage` makes exactly that argument about its unlock
          dialog. The caution here is a DIFFERENT one -- the next build
          will be slow -- and it is stated in those words.

          What it does carry is the measurement from `reclaim.rs`: "real
          build durations went 7m8s -> 1m20s -> 56.9s as the cache warmed".
          A number the user can weigh is the difference between a knowing
          trade and a click, which is what that file asks for. */}
      {pruning ? (
        <Dialog open onOpenChange={(o) => !o && setPruning(false)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Clear cache older than 7 days?</DialogTitle>
            <ActingOnDesktop />
            {/* Says the SCOPE, which the link could not. "clear" beside a
                total invites reading it as "clear all of that", and the
                call is `prune("168h")` -- the last week is deliberately
                kept, because it is the part making today's builds fast. */}
            <p className="mt-3 text-sm text-[#e6edf3]">
              Entries from the last 7 days are kept — those are the ones making
              today’s builds fast. Everything older goes.
            </p>
            <p className="mt-2 text-sm text-[#8b949e]">
              Cache is not waste, it is speed. On a real project, build times fell
              from 7m08s to 1m20s to 56.9s as the cache warmed; the next build after
              clearing pays that back from cold. Nothing is lost — it is rebuilt.
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setPruning(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  setPruning(false);
                  // A week, not everything: the last day's cache is what
                  // makes today's builds fast, while a month-old entry is
                  // unlikely to be helping any current work.
                  void prune("168h").then(
                    (freed) =>
                      toast.success(
                        `Freed ${formatDockerSize(freed || null)} of build cache`,
                      ),
                    (e: unknown) => {
                      // Silent when the user dismissed the biometric
                      // prompt: they declined, nothing was pruned, and
                      // reporting their own decision back as a failure is
                      // noise.
                      if (isCancelled(e)) return;
                      toast.error("Could not clear the build cache", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                // The NEUTRAL confirm, matching the unlock dialog rather
                // than the removals: this is the button for an action
                // whose cost is time.
                className="rounded border border-[#30363d] bg-[#21262d] px-3 py-1.5 text-sm font-medium text-[#e6edf3] hover:bg-[#30363d]"
              >
                Clear the older cache
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {pendingVolume ? (
        <Dialog open onOpenChange={(o) => !o && setPendingVolume(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Delete this volume?</DialogTitle>
            <ActingOnDesktop />
            <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">
              {pendingVolume.name}
            </p>
            {/* Kept from the original wording: a volume's contents are
                not regenerable, which is why volumes are confirmed one
                at a time and never bulk-pruned. */}
            <p className="mt-2 text-sm text-[#8b949e]">
              {formatDockerSize(pendingVolume.size_bytes || null)} · its contents cannot
              be recovered.
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setPendingVolume(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const target = pendingVolume;
                  setPendingVolume(null);
                  removeVolume(target.name).then(
                    () => toast.success(`Removed ${target.name}`),
                    (e: unknown) => {
                      if (isCancelled(e)) return;
                      toast.error(`Could not remove ${target.name}`, {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Delete
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {pending ? (
        <Dialog open onOpenChange={(o) => !o && setPending(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Remove this image?</DialogTitle>
            <ActingOnDesktop />
            <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">
              {pending.repository}
              {pending.tags.length > 0 ? `:${pending.tags.join(", :")}` : ""}
            </p>
            <p className="mt-2 text-sm text-[#8b949e]">
              {formatDockerSize(pending.size_bytes)} · {imageState(pending)}
              {pending.origin ? ` · ${pending.origin.subject}` : ""}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setPending(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const target = pending;
                  setPending(null);
                  setRemoving(target.id);
                  removeImages([target.id]).then(
                    (outcomes) => {
                      setRemoving(null);
                      const failed = outcomes.find((o) => o.error !== null);
                      if (failed) {
                        toast.error("Could not remove the image", {
                          description: failed.error ?? undefined,
                        });
                      } else {
                        toast.success(`Removed ${formatDockerSize(target.size_bytes)}`);
                      }
                    },
                    (e: unknown) => {
                      setRemoving(null);
                      if (isCancelled(e)) return;
                      toast.error("Could not remove the image", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Remove
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {bulkOpen ? (
        <Dialog open onOpenChange={(o) => !o && setBulkOpen(false)}>
          <DialogContent className="max-w-2xl">
            {/* Keyed off `includeWider`, NOT `bulkOpen` (#852).

                `bulkOpen` is assigned `"stale"` at its ONLY call site, so
                the `"superseded"` arm of both of these was dead code while
                `bulkSet` -- the thing actually being removed -- follows the
                checkbox below. Ticking "Also remove N superseded images
                whose branch is still open" therefore widened the set and
                left the title reading "Remove 3 stale images?" over a list
                of 19, with the reassurance still claiming "its branch has
                merged": false for exactly the images the checkbox added,
                and the sentence beneath the checkbox already says so.

                One expression decides the membership and the words now.
                That is the point -- `bulkSet` is `includeWider ?
                superseded : stale`, so keying the copy off anything else
                is how the two came apart in the first place. `bulkOpen` is
                now a plain boolean for the same reason the `panel` axis was
                removed in this issue: once it stopped deciding the wording
                it was read only for truthiness, and an unreachable union
                member is a trap for the next caller who believes setting it
                does something. */}
            <DialogTitle>
              Remove {bulkSet.length} {includeWider ? "superseded" : "stale"} image
              {bulkSet.length === 1 ? "" : "s"}?
            </DialogTitle>
            <ActingOnDesktop />
            {/* The wider set needs a DIFFERENT sentence, not a louder
                one: some of its images belong to branches that are
                still open, and the dialog has to say so plainly rather
                than reuse the reassurance that fits the narrow set.

                `:656` already stated that requirement; what it lacked was
                a condition that could ever be true. */}
            <p className="mt-2 text-sm text-[#8b949e]">
              Reclaims {formatDockerSize(bulkBytes || null)}.{" "}
              {includeWider
                ? "Each one has been replaced by a newer build and nothing is running it — but some belong to branches that are still open, so check the list."
                : "Every one is superseded and its branch has merged, so nothing will want it again."}
            </p>
            {/* The wider set as an OPT-IN, and only when it actually
                covers more. Named with its difference so the choice has
                a stated reason rather than leaving the reader to
                subtract two counts. */}
            {superseded.length > stale.length ? (
              <label className="mt-3 flex items-center gap-2 text-sm text-[#d29922]">
                <input
                  type="checkbox"
                  checked={includeWider}
                  onChange={() => setIncludeWider((v) => !v)}
                />
                Also remove {superseded.length - stale.length} superseded image
                {superseded.length - stale.length === 1 ? "" : "s"} whose branch is
                still open or could not be traced
              </label>
            ) : null}
            <ul className="mt-3 max-h-64 overflow-y-auto font-mono text-xs text-[#8b949e]">
              {bulkSet.map((i) => (
                <li key={i.id} className="py-0.5">
                  {i.repository}:{i.tags[0] ?? i.id.slice(0, 12)} — {formatDockerSize(i.size_bytes)}
                </li>
              ))}
            </ul>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setBulkOpen(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const targets = bulkSet.map((i) => i.id);
                  const freed = bulkBytes;
                  setBulkOpen(false);
                  setBusy(true);
                  removeImages(targets).then(
                    (outcomes) => {
                      setBusy(false);
                      const failed = outcomes.filter((o) => o.error !== null);
                      if (failed.length === 0) {
                        toast.success(`Removed ${outcomes.length}, freeing ${formatDockerSize(freed)}`);
                      } else {
                        toast.error(`${failed.length} of ${outcomes.length} could not be removed`, {
                          description: failed.map((f) => f.error).join("\n"),
                        });
                      }
                    },
                    (e: unknown) => {
                      setBusy(false);
                      toast.error("The removal could not run", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Remove {bulkSet.length} image{bulkSet.length === 1 ? "" : "s"}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      {restartOpen !== null ? (
        <Dialog open onOpenChange={(o) => !o && setRestartOpen(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Restart Docker?</DialogTitle>
            <ActingOnDesktop />
            {/* Name what will be stopped: the user may have a database
                up that they would otherwise notice the hard way. */}
            <p className="mt-3 text-sm text-[#8b949e]">
              {restartOpen.length === 0
                ? "No containers are running."
                : `${restartOpen.length} running container${
                    restartOpen.length === 1 ? "" : "s"
                  } will be stopped: ${restartOpen.join(", ")}`}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setRestartOpen(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  setRestartOpen(null);
                  setBusy(true);
                  dockerRestart().then(
                    () => {
                      setBusy(false);
                      toast.success("Docker restarted");
                    },
                    (e: unknown) => {
                      setBusy(false);
                      toast.error("Could not restart Docker", {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Restart
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}
