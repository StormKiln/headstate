import { toast } from "sonner";
import { useActiveFilters } from "@/store/filters";
import { ExternalLink } from "./ExternalLink";
import { getVersion } from "@tauri-apps/api/app";
import { latestRelease } from "../api/tauri";
import { UpdateDialog } from "./UpdateDialog";
import { useUiPrefs } from "../api/hooks";
import { Settings } from "lucide-react";
import { useEffect, useState } from "react";
import {
  usePollError,
  usePollInterval,
  usePollState,
  useRemovalProgress,
  useCancelUpdateRun,
  useUpdateProgress,
} from "../api/hooks";
import { relativeTime } from "../lib/time";
import { useIsMobile } from "../lib/useIsMobile";
import { IS_MOBILE_BUILD } from "../lib/target";
import { SettingsDialog } from "./SettingsDialog";

const CHOICES = [60, 120, 300, 900];

function label(secs: number): string {
  return secs < 60 ? `${secs}s` : `${secs / 60}m`;
}

/// A thin strip along the bottom: what the app is doing, and when it last
/// heard from GitHub.
///
/// Deliberately quiet. The window is small and the list is the point, so
/// this is one line of muted text, not a second header.
///
/// It used to know only "fetching" and "Up to date", on the reasoning
/// that errors go to the banner. That holds only while a banner actually
/// appears -- and in #190 none did, so this line confidently asserted
/// everything was fine while both PR views sat empty. A status bar that
/// can only ever say "fine" is not a status bar.
///
/// It still shows STATE, not messages: the banner keeps the detail and
/// the retry. This just stops claiming success it cannot vouch for.
/// How often to re-ask GitHub for the newest release.
///
/// Daily. Releases ship every few days at most, and the request is one
/// unauthenticated call -- so this is far below any rate concern while
/// still being fast enough that nobody sits three versions behind.
const UPDATE_CHECK_MS = 24 * 60 * 60 * 1000;

export function StatusBar({ updatedAt }: { updatedAt: number }) {
  const state = usePollState();
  const pollError = usePollError();

  // "Never succeeded" and "stale after a failure" are different. A green
  // dot beside "Updated 3 hours ago" is defensible; a green dot with no
  // successful fetch at all is not.
  const status = pollError
    ? updatedAt > 0
      ? ("stale" as const)
      : ("failed" as const)
    : state === "fetching"
      ? ("fetching" as const)
      // A suppressed transient failure emits neither poll-error nor
      // prs-updated, so without this the bar showed a green "Up to date"
      // beside a stale timestamp -- the exact bug #191 was written to
      // eliminate, at smaller scale.
      : state === "retrying"
        ? ("retrying" as const)
        : ("ok" as const);

  const DOT = {
    fetching: "bg-[#58a6ff]",
    ok: "bg-[#3fb950]",
    retrying: "bg-[#d29922]",
    stale: "bg-[#d29922]",
    failed: "bg-[#f85149]",
  } as const;

  // Every label names its SUBJECT: this bar speaks for the pull request
  // poll and for nothing else.
  //
  // It used to say a bare "Up to date", which is a claim about the app. A
  // user watching the stats backfill fail on the PR Stats page read the
  // green dot beside the yellow banner as a contradiction (#1115) -- and
  // it was not one. The bar was correct about pull requests while a
  // different subsystem was failing, with nothing on screen to say the
  // two had different subjects.
  //
  // Naming the subject is cheaper than a second indicator and keeps the
  // detail where it belongs: the stats page states its own condition.
  const TEXT = {
    fetching: "Checking GitHub…",
    ok: "PRs up to date",
    retrying: "Retrying…",
    stale: "Could not refresh PRs",
    failed: "Could not reach GitHub",
  } as const;
  const { seconds, set } = usePollInterval();
  const [settingsOpen, setSettingsOpen] = useState(false);
  // The poll cadence is the desktop's setting, and Settings > General
  // carries it too. On a phone-width bar it is the one thing that can
  // go: with it, "Up to date" and the timestamp each broke onto two
  // lines.
  const isMobile = useIsMobile();

  // From the built binary, not package.json: the release workflow stamps
  // the version from the git tag at build time and never commits it, so
  // the repo reads 0.1.0 while a release reads 2.0.1. Asking Tauri gets
  // the number the user actually installed -- which is the whole point of
  // showing it, since it is what they would quote in a bug report.
  const [version, setVersion] = useState<string | null>(null);
  const [newer, setNewer] = useState<string | null>(null);
  const { prefs: ui } = useUiPrefs();
  // Subscribed here, not only on the Worktrees page, so the count
  // survives navigating away from it.
  const removal = useRemovalProgress();
  const updating = useUpdateProgress();
  const cancelRun = useCancelUpdateRun();
  // The repo a run would belong to: runs are per-repository, and
  // the status bar shows the selected one.
  const repo = useActiveFilters().repo;
  // Which version's announcement has been dismissed. localStorage, not
  // the settings table: it is a transient acknowledgement of one
  // release, not a preference, and it is meaningless on another machine.
  const [dismissed, setDismissed] = useState<string | null>(() =>
    localStorage.getItem("headstate-update-dismissed"),
  );
  useEffect(() => {
    if (dismissed !== null) localStorage.setItem("headstate-update-dismissed", dismissed);
  }, [dismissed]);
  useEffect(() => {
    let live = true;
    getVersion().then(
      (v) => live && setVersion(v),
      // Non-fatal: a missing version line is better than a broken bar.
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  // At startup, then daily, and whenever the window is shown again.
  //
  // This used to run ONCE per mount, which is wrong for an app that
  // hides to the tray instead of quitting (`lib.rs` intercepts
  // CloseRequested and calls `window.hide()`). A machine left running
  // never asked again: it took one release and then stayed on it while
  // two more shipped, with no way for the user to discover them.
  //
  // The visibility trigger matters as much as the timer: a laptop asleep
  // for a week should learn about an update when its owner comes back to
  // it, not up to a day later.
  //
  // A failure is silent: a missing update hint is better than a broken
  // status bar, and the app works perfectly well without knowing.
  useEffect(() => {
    // Desktop only, and not merely as an optimisation.
    //
    // `latest_release` is `Class::Read` and therefore allowlisted, so on
    // the phone it succeeded -- and returned the newest DESKTOP release,
    // which the phone then compared against its OWN version and offered
    // to install over itself. The dialog it opened says "Installing
    // replaces the app and restarts it" and calls
    // `@tauri-apps/plugin-updater`, which is not in the mobile crate at
    // all, so Install rejected with a raw plugin error.
    //
    // Both halves are wrong on a phone: the comparison is against the
    // wrong product, and the remedy is one Apple prohibits. Mobile
    // updates come from the App Store and TestFlight.
    if (IS_MOBILE_BUILD) return;
    let live = true;
    const ask = () =>
      latestRelease().then(
        (v) => live && setNewer(v),
        () => {},
      );

    void ask();
    const timer = setInterval(() => void ask(), UPDATE_CHECK_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") void ask();
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      live = false;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  return (
    <>
    {/* Per VERSION, not per launch: nagging about a release someone has
        already declined is how a notice trains people to dismiss it
        unread. `announce_updates` turns the dialog off entirely without
        touching the status-bar hint, which stays either way. */}
    {newer && (ui?.announce_updates ?? true) ? (
      <UpdateDialog
        version={newer}
        open={dismissed !== newer}
        onDismiss={() => setDismissed(newer)}
      />
    ) : null}
    <div className="pb-safe flex shrink-0 items-center gap-3 border-t border-[#30363d] bg-[#0d1117] px-4 py-1.5 text-xs text-[#8b949e]">
      {/* Hidden on the phone, where `ConnectionBanner` carries the same
          two facts at the top of the screen (#649). Two strips saying
          "last polled X ago" cost a phone more list than they inform,
          and the banner is the one that can also say the desktop is
          unreachable -- which makes GitHub freshness moot.

          Only this pair moves. The progress counter, its cancel button,
          the version and the settings entry point below have no second
          home and stay on both. */}
      {isMobile ? null : (
        <>
          <span className="flex items-center gap-1.5">
            <span className={`h-1.5 w-1.5 rounded-full ${DOT[status]}`} aria-hidden="true" />
            <span className={status === "failed" ? "text-[#f85149]" : undefined}>
              {TEXT[status]}
            </span>
          </span>

          {/* `dataUpdatedAt`, not `isFetching`: the tray path advances the
              former on both routes but never flips the latter. */}
          {updatedAt > 0 ? (
            <span>Updated {relativeTime(new Date(updatedAt).toISOString())}</span>
          ) : null}
        </>
      )}

      {/* Bulk worktree removal reported progress only on the Worktrees
          page's own button -- but the work runs on the backend and
          outlives that page, so navigating away made a running batch
          look like it had stopped. The status bar is the surface that
          persists across views, which makes it the honest place for
          work that does. */}
      {/* `aria-live` on the CONTAINER, which is always mounted, rather
          than on the message, which is not. A live region has to exist
          before the text appears or the first announcement is missed --
          the one that matters most, since it is the one saying work
          started. `polite` because a removal count must not interrupt
          whatever the user is reading. */}
      {/* A background update run is the same case, and more so: it
          returns immediately by design (#495), so the wizard that
          started it is already closed. It said nothing at all until the
          run finished -- minutes, on a large selection. Sharing one
          live region because the two cannot overlap in practice and
          two adjacent counters would be worse than one. */}
      <span aria-live="polite" className="text-[#58a6ff]">
        {removal
          ? `Removing worktrees — ${removal.done} of ${removal.total}`
          : updating
            ? `Updating packages — ${updating.done} of ${updating.total}`
            : ""}
      </span>
      {/* Cancel sits beside the count rather than in the wizard, for
          the same reason the count does: the wizard is already closed.
          Only for updates -- a worktree removal is a sequence of quick
          deletions with nothing useful to stop halfway. */}
      {updating && repo ? (
        <button
          type="button"
          onClick={() => {
            void cancelRun(repo).then(
              // Not "stopped": it stops after the package it is on, and
              // `update-run-done` is what reports that it actually has.
              () => toast.info("Stopping after the current package…"),
              (e: unknown) =>
                toast.error("Could not stop the run", {
                  description: typeof e === "string" ? e : undefined,
                }),
            );
          }}
          className="tap-target rounded px-2 text-[#8b949e] hover:text-[#e6edf3]"
        >
          Cancel
        </button>
      ) : null}

      {isMobile ? (
        <span className="ml-auto" />
      ) : (
      <label className="ml-auto flex items-center gap-1.5">
        <span>Check every</span>
        <select
          value={seconds ?? 120}
          onChange={(e) => void set(Number(e.target.value))}
          aria-label="Poll interval"
          className="rounded border border-[#30363d] bg-[#161b22] px-1.5 py-0.5 text-xs text-[#e6edf3]"
        >
          {CHOICES.map((s) => (
            <option key={s} value={s}>
              {label(s)}
            </option>
          ))}
        </select>
      </label>
      )}

      {/* Beside the settings button rather than next to the poll state:
          this is an identity, not a status, and it is what a user quotes
          in a bug report. Rendered only once known, so the bar does not
          flash a placeholder on every launch. */}
      {version ? (
        <span className="tabular-nums" title={`Headstate ${version}`}>
          v{version}
        </span>
      ) : null}
      {/* Unobtrusive on purpose: an update is worth knowing about, not
          worth interrupting for. */}
      {newer ? (
        <ExternalLink
          href="https://github.com/pktstorm/headstate/releases/latest"
          className="text-[#58a6ff] hover:underline"
          title={`Headstate ${newer} is available`}
        >
          v{newer} available
        </ExternalLink>
      ) : null}

      <button
        type="button"
        onClick={() => setSettingsOpen(true)}
        aria-label="Settings"
        title="Settings"
        className="tap-target flex items-center justify-center rounded hover:bg-[#161b22]"
      >
        <Settings className="h-3.5 w-3.5" aria-hidden="true" />
      </button>

      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
    </>
  );
}
