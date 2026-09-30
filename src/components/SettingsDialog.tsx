import { useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { toast } from "sonner";
import { revealLog } from "@/api/tauri";
import { ToolVersions } from "./ToolVersions";
import { useIsMobile } from "@/lib/useIsMobile";
import { HelpButton } from "./HelpButton";
import { GetCompanionPanel } from "./GetCompanionPanel";
import { PairPhonePanel } from "./PairPhonePanel";
import { PairedDesktopPanel } from "./PairedDesktopPanel";
import { ALWAYS_OFFERED, VIEWS } from "./ViewSwitcher";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { PairedDevicesList } from "./PairedDevicesList";
import { PhoneNotifyPanel } from "./PhoneNotifyPanel";
import { ClaudeIntegrationsPanel } from "./ClaudeIntegrationsPanel";
import { GitLabHostPanel } from "./GitLabHostPanel";
import {
  useAutostart,
  useNotifyPrefs,
  usePollInterval,
  useCleanupPrefs,
  useRemoteEnabled,
  useUiPrefs,
  useWorktreeDirs,
} from "../api/hooks";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import {
  CLEANUP_GROUPS,
  parentState,
  toggleChild,
  toggleParent,
} from "@/lib/cleanupGroups";
import { STALE_DAY_CHOICES, staleDaysLabel, staleVenvDays } from "@/lib/staleVenv";

/// Matches the backend's own range: `clamp_interval` allows 60s..3600s
/// (`poll.rs`), and the UI previously stopped at 900 -- so a user who
/// wanted a half-hour cadence to conserve rate limit could not pick one
/// even though the backend would have accepted it.
const INTERVALS = [60, 120, 300, 900, 1800, 3600];

function intervalLabel(secs: number): string {
  return secs < 60 ? `${secs}s` : `${secs / 60} min`;
}

/// Settings.
///
/// Values live in SQLite on the Rust side rather than in the webview,
/// because the poll loop and the worktree scanner both read them and
/// neither can see `localStorage`. That also means a write can FAIL -- a
/// path that is not a directory is rejected -- so the error is shown
/// rather than swallowed.
/// The left rail's topics, in the order shown.
///
/// Grouped by what a setting is ABOUT rather than by which struct it
/// lives in: "General" holds the poll interval, window behaviour and
/// keyboard notes, which come from three preference sources and are one
/// subject to a person.
const SECTIONS = [
  { id: "general", label: "General" },
  { id: "repositories", label: "Repositories" },
  { id: "gitlab", label: "GitLab" },
  { id: "notifications", label: "Notifications" },
  { id: "cleanup", label: "Cleanup" },
  { id: "phone", label: "Phone" },
  // Its own topic rather than a row under Views, because the switch is a
  // CAPABILITY and not a view preference -- and because the section carries
  // the hook installer, which edits a file outside this app (#915). Placed
  // before Views so the last entry stays the one that is about nothing but
  // what is on screen.
  { id: "claude", label: "Claude Code" },
  { id: "views", label: "Views" },
] as const;

type SectionId = (typeof SECTIONS)[number]["id"];

export function SettingsDialog({
  open,
  onOpenChange,
  initialSection = "general",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /// Which topic to show first. The status bar's gear opens on
  /// General; the phone's connection banner opens straight on Phone,
  /// since that is the only reason it was tapped.
  initialSection?: SectionId;
}) {
  // Layout, not capability: a desktop window dragged phone-narrow wants
  // the stacked layout too, and it is still a desktop with `gh`.
  const isMobile = useIsMobile();
  const { seconds, set: setInterval } = usePollInterval();
  const { dirs, set: setDirs } = useWorktreeDirs();
  const { prefs, set: setPrefs } = useNotifyPrefs();
  const { prefs: ui, set: setUi } = useUiPrefs();
  const { prefs: cleanup, set: setCleanup } = useCleanupPrefs();
  const { enabled: autostart, set: setAutostart } = useAutostart();
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const { enabled: remote, set: setRemote } = useRemoteEnabled();
  const [remoteError, setRemoteError] = useState<string | null>(null);
  // `null` is the phone's list view; the desktop always has a section
  // selected, because its rail is always visible beside the panel.
  //
  // Opening straight to a section (the banner does, with "phone") still
  // works and still shows that section -- with the back row above it,
  // which is where a user who arrived that way would expect to find
  // the rest of Settings.
  const [section, setSection] = useState<SectionId | null>(initialSection);
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // `null` means "not edited", so the field renders the saved value
  // without an effect copying it into state. Seeding via useEffect meant
  // a setState inside an effect, which cascades renders -- and it fought
  // the query: whenever `dirs` refetched, an in-progress edit would be
  // silently overwritten.
  const value = draft ?? dirs.join("\n");

  const close = () => {
    setDraft(null);
    setError(null);
    onOpenChange(false);
  };

  const save = () => {
    const next = value
      .split("\n")
      .map((d) => d.trim())
      .filter(Boolean);
    setDirs(next).then(
      () => close(),
      (e: unknown) => setError(typeof e === "string" ? e : "Could not save"),
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* The dialog caps its own height at the viewport, and this
          splits it so the BODY scrolls while the buttons stay put.
          Without it a tall settings list pushed OK and Cancel below the
          window edge on an unmaximised window -- not merely hard to
          reach, invisible and unclickable. */}
      {/* FIXED in both dimensions, like macOS System Settings.
          Only the content pane scrolls.

          Width: `max-w-lg` was 32rem, chosen when this was one column.
          The topic menu then took 9rem of it and left ~20rem for
          controls, which squeezed every row -- the poll-interval label
          and its select, the cleanup granularity rows. `max-w-3xl`
          gives the content pane roughly the width it had before the
          menu existed.

          Height: the dialog had a max but no fixed height, so it sized
          to whichever topic was showing and jumped on every switch --
          moving the window under the cursor, so the topic just clicked
          could end up somewhere else. `h-[32rem]` holds it still.

          Sizing to the tallest topic would also stop the jumping, but
          pads every short topic with dead space and changes again the
          moment a setting is added. A fixed frame does not. */}
      {/* Spelt ONCE. This used to carry `sm:max-w-3xl` alongside
          `max-w-3xl` (and `sm:max-w-none` alongside `max-w-none`),
          because the base `DialogContent` carried a `sm:max-w-sm` that
          tailwind-merge keyed separately from a bare `max-w-*` and that
          won above 640px -- so the bare form came out NARROWER than the
          32rem this dialog started at. #1306 moved the base's cap onto
          the callers' own key, so the plain form applies. */}
      {/* On a phone: nearly full height, and the nav rail stacks ABOVE
          the content rather than beside it. `h-[32rem]` is a hard 512px
          slab with 144px of that spent on a fixed-width rail, leaving
          ~214px of content pane at 390px -- not enough for the `w-40`
          selects or the keyboard-shortcut grid inside it, and the
          pairing panel's fingerprint collapsed to an unreadable column.
          This dialog is also where the connection banner sends a phone
          user, so it had to be usable there. */}
      <DialogContent
        className={
          isMobile
            ? "flex h-[calc(100dvh-4rem)] max-w-none flex-col"
            : "flex h-[32rem] max-w-3xl flex-col"
        }
      >
        <DialogTitle>Settings</DialogTitle>

        {/* TWO PANES: topics on the left, the chosen one on the right.

            This was ~445 lines of continuous scroll with seven groups
            separated by hairlines, and it kept growing -- a log button,
            a notification toggle and a cleanup opt-in all landed in it
            recently.

            Every control is preserved verbatim; only where it lives
            changed. The existing tests assert specific labels and keep
            passing rather than being rewritten to match a new layout --
            a reorganisation that hides a control is a regression. */}
        <div
          className={
            isMobile
              ? "-mx-1 flex min-h-0 flex-1 flex-col gap-3 overflow-hidden px-1"
              : "-mx-1 flex min-h-0 flex-1 gap-4 overflow-hidden px-1"
          }
        >
          {/* On the phone, a vertical LIST that pushes to a section --
              the iOS settings pattern -- rather than a strip of tabs
              that scrolls sideways. A horizontal scroller has no
              affordance saying more topics exist, so sections past the
              third were simply undiscoverable (#650).

              Hidden with CSS rather than unmounted, for the same reason
              the panels below are: the buttons must stay in the
              accessibility tree and in the DOM, and the existing tests
              find them by role either way. */}
          <nav
            aria-label="Settings sections"
            className={
              isMobile
                ? `flex shrink-0 flex-col gap-0.5 ${section === null ? "" : "hidden"}`
                : "flex w-36 shrink-0 flex-col gap-0.5 border-r border-[#30363d] pr-2"
            }
          >
            {SECTIONS.filter((s) => s.id !== "gitlab" || !IS_MOBILE_BUILD).map((s) => (
              <button
                key={s.id}
                type="button"
                aria-current={section === s.id ? "page" : undefined}
                onClick={() => setSection(s.id)}
                className={
                  isMobile
                    ? // A row, with the chevron every iOS list row has.
                      // 44pt minimum: this is the primary target here.
                      "flex min-h-11 items-center justify-between rounded px-2 text-left text-sm text-[#e6edf3] hover:bg-[#21262d]"
                    : `rounded px-2 py-1 text-left text-sm ${
                        section === s.id
                          ? "bg-[#1f6feb] text-white"
                          : "text-[#e6edf3] hover:bg-[#21262d]"
                      }`
                }
              >
                {s.label}
                {isMobile ? (
                  <ChevronRight className="size-4 shrink-0 text-[#8b949e]" aria-hidden="true" />
                ) : null}
              </button>
            ))}
          </nav>

          {/* The way back out of a section. Only on the phone, where the
              list replaced the always-visible rail. */}
          {isMobile && section !== null ? (
            <button
              type="button"
              onClick={() => setSection(null)}
              className="flex min-h-11 shrink-0 items-center gap-1 self-start px-1 text-sm text-[#58a6ff]"
            >
              <ChevronLeft className="size-4" aria-hidden="true" />
              Settings
            </button>
          ) : null}

          {/* `min-h-0` is load-bearing here as it was on the old
              scroller: a flex child defaults to min-height:auto and
              refuses to shrink below its content, so without it the
              panel pushes the footer out instead of scrolling. */}
          <div className="min-h-0 flex-1 overflow-y-auto pr-1">
            {/* Rendered always, hidden with CSS -- never unmounted and
                never the `hidden` ATTRIBUTE.
                Unmounting loses a control's state on every tab switch.
                The `hidden` attribute additionally removes the panel
                from the accessibility tree, so a screen reader cannot
                reach a setting until the right topic is clicked -- the
                existing tests caught exactly that, by failing to find
                controls by role. */}
            <div className={section === "general" ? "" : "hidden"}>
        <div className="mt-4 flex flex-col gap-1">
          <div className="flex items-center">
            <label htmlFor="poll-interval" className="text-sm font-medium">
              Check GitHub every
            </label>
            {/* Outside the label, for the same reason as above. */}
            <HelpButton topic="poll-interval" />
          </div>
          <select
            id="poll-interval"
            value={seconds ?? 120}
            onChange={(e) => void setInterval(Number(e.target.value))}
            className="w-40 rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-sm"
          >
            {INTERVALS.map((s) => (
              <option key={s} value={s}>
                {intervalLabel(s)}
              </option>
            ))}
          </select>
          <p className="text-xs text-[#8b949e]">
            Applies immediately. Shorter intervals use more of your GitHub rate limit.
          </p>
        </div>

        {/* Notifications had no off switch anywhere in the app -- the
            only escape was denying permission at the OS level, which the
            poll loop treats as permanent. Nothing in the UI even said
            the app sent them. */}
        <div className="mt-5 flex flex-col gap-2">
          <span className="text-sm font-medium">Window</span>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={ui?.close_hides_to_tray ?? true}
              onChange={() =>
                ui && void setUi({ ...ui, close_hides_to_tray: !ui.close_hides_to_tray })
              }
            />
            Closing the window hides it to the tray
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={autostart}
              onChange={() => {
                setAutostartError(null);
                // Unlike every other setting here this one touches the
                // filesystem and can genuinely fail, so the error is
                // shown rather than swallowed.
                void setAutostart(!autostart).catch((e: unknown) =>
                  setAutostartError(typeof e === "string" ? e : "Could not change this"),
                );
              }}
            />
            Start Headstate at login
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={ui?.announce_updates ?? true}
              onChange={() =>
                ui && void setUi({ ...ui, announce_updates: !ui.announce_updates })
              }
            />
            Tell me when a new version is available
          </label>
          {/* Off by default and phrased for the situation it exists
              for. "Diagnostic logging" alone invites people to turn it
              on speculatively; naming the cost and the use makes it a
              tool you reach for when asked. */}
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={ui?.diagnostic_logging ?? false}
              onChange={() =>
                ui && void setUi({ ...ui, diagnostic_logging: !ui.diagnostic_logging })
              }
            />
            Write a detailed timing log (for diagnosing slowness)
          </label>
          {/* This log is meant to be SENT to someone, so what it
              contains has to be checkable rather than trusted. */}
          <span className="-mt-1 self-start">
            <HelpButton topic="diagnostic-log" />
          </span>
          {ui?.diagnostic_logging ? (
            <>
              {/* "GitHub requests" was accurate until local scans were
                  instrumented too. A user reading the old text had no
                  reason to think this would help with a hanging
                  Virtualenvs page, which is now what it is for. */}
              <p className="text-xs text-[#8b949e]">
                Records how long GitHub requests and local scans take, and the
                writes you make — each by repository and pull request number.
                Never your token, never a local path. Worth a read before you
                send it to anyone.
              </p>
              {/* Where the file IS. Without this the path has to be
                  passed on out of band, which is the friction the
                  checkbox exists to remove. */}
              <button
                type="button"
                onClick={() => {
                  void revealLog().then(
                    (path) => toast.success("Showed the log", { description: path }),
                    (e: unknown) =>
                      toast.error("Could not show the log", {
                        description: typeof e === "string" ? e : undefined,
                      }),
                  );
                }}
                className="self-start rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
              >
                Show the log
              </button>
            </>
          ) : null}
          {autostartError ? (
            <p role="alert" className="text-xs text-[#f85149]">
              {autostartError}
            </p>
          ) : null}
          {/* Beside the log, because both answer the same question:
              something is not working and what do I check (#1154). */}
          <ToolVersions />
        </div>

        {/* Automatic cleanup, which in this build cannot remove
            anything. Presented as what it IS -- a report of what would
            be removed -- rather than as a feature with the acting half
            greyed out, because a switch that suggests it might delete is
            exactly the thing to avoid understating. */}
        <div className="mt-5 flex flex-col gap-1">
          <span className="text-sm font-medium">Keyboard</span>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs text-[#8b949e]">
            {/* The grid said "pull request" twice, and #953 filed that as
                missing functionality rather than wrong documentation
                precisely because it was HONEST: the cursor really did
                only walk the pull request list.

                It now walks the Claude Code sessions list too, so the
                wording follows -- "the highlighted row" for the two keys
                that are no longer PR-only, and `x` stays specific because
                it still is: sessions have no bulk action, so the key does
                nothing there rather than inventing a selection. Saying
                "row" for `x` would make this grid wrong in the direction
                the issue was careful not to be. */}
            {[
              ["j / k", "Move down / up the list"],
              ["Enter", "Open the highlighted row"],
              ["x", "Select the highlighted pull request, for a bulk action"],
              ["/", "Search the list on screen"],
              ["Esc", "Hide the window to the tray"],
            ].map(([keys, what]) => (
              <div key={keys} className="contents">
                <dt className="font-mono text-[#e6edf3]">{keys}</dt>
                <dd>{what}</dd>
              </div>
            ))}
          </dl>
        </div>
            </div>
            {/* Rendered always, hidden with CSS -- never unmounted and
                never the `hidden` ATTRIBUTE.
                Unmounting loses a control's state on every tab switch.
                The `hidden` attribute additionally removes the panel
                from the accessibility tree, so a screen reader cannot
                reach a setting until the right topic is clicked -- the
                existing tests caught exactly that, by failing to find
                controls by role. */}
            <div className={section === "repositories" ? "" : "hidden"}>
        <div className="mt-5 flex flex-col gap-1">
          <div className="flex items-center">
            <label htmlFor="worktree-dirs" className="text-sm font-medium">
              Directories to scan for repositories
            </label>
            {/* OUTSIDE the label: a button nested inside one joins its
                accessible name, so the field would announce as
                "Directories to scan for repositories About scanned
                directories".
                
                Feeds Docker provenance too, which is not guessable from
                a setting that reads as being about worktrees -- and is
                the first thing to check when provenance is empty. */}
            <HelpButton topic="scanned-dirs" />
          </div>
          <textarea
            id="worktree-dirs"
            value={value}
            onChange={(e) => setDraft(e.target.value)}
            rows={3}
            spellCheck={false}
            placeholder="~/code"
            className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-sm"
          />
          {/* Says `~` works, and says the saved form differs from the
              typed one. Both halves earn their words: #945 was the field
              REJECTING its own placeholder, and the fix stores the
              expanded path -- so someone who types `~/code` and sees
              `/Users/me/code` come back needs to know that is the
              intended outcome rather than the app mangling their input. */}
          <p className="text-xs text-[#8b949e]">
            One path per line. Used to find git worktrees. <code>~</code> is expanded and
            saved as the full path.
          </p>
          {error ? (
            <p role="alert" className="text-xs text-[#f85149]">
              {error}
            </p>
          ) : null}
        </div>

        {/* Half the top-level navigation is irrelevant to a PR-only
            user, and both of these lead to an empty screen on first run
            -- Worktrees needs scan directories, Docker needs a running
            daemon. "My pull requests" is deliberately absent: it is the
            default view and the app's premise, so hiding it would leave
            someone with no way back. */}
            </div>
            {/* Rendered always, hidden with CSS -- never unmounted and
                never the `hidden` ATTRIBUTE.
                Unmounting loses a control's state on every tab switch.
                The `hidden` attribute additionally removes the panel
                from the accessibility tree, so a screen reader cannot
                reach a setting until the right topic is clicked -- the
                existing tests caught exactly that, by failing to find
                controls by role. */}
            {!IS_MOBILE_BUILD && section === "gitlab" ? <GitLabHostPanel /> : null}
            <div className={section === "notifications" ? "" : "hidden"}>
        <div className="mt-5 flex flex-col gap-2">
          <span className="text-sm font-medium">Notifications</span>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={prefs?.enabled ?? true}
              onChange={() =>
                prefs && void setPrefs({ ...prefs, enabled: !prefs.enabled })
              }
            />
            Desktop notifications
          </label>
          {/* Nested and disabled rather than hidden when the master
              switch is off: hiding them would make the choices look
              lost, and they are deliberately preserved so turning
              notifications back on restores what was picked. */}
          <div className="ml-6 flex flex-col gap-2">
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.ci_failed ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, ci_failed: !prefs.ci_failed })
                }
              />
              CI starts failing
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.conflicted ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, conflicted: !prefs.conflicted })
                }
              />
              Merge conflicts appear
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.ready_to_review ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, ready_to_review: !prefs.ready_to_review })
                }
              />
              A pull request becomes ready for your review
            </label>
            {/* #789. Beside `ready_to_review` because both are about a
                pull request arriving, and deliberately worded to say
                what makes them different: this one fires whatever
                state the pull request is in, where that one waits for
                it to be green and assigned to you. */}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.new_pr ?? true}
                onChange={() => prefs && void setPrefs({ ...prefs, new_pr: !prefs.new_pr })}
              />
              A pull request appears, whatever state it is in
            </label>
          </div>
          {/* The machine's own health: the battery alerts (#720) and the
              CPU one (#791).

              Until now these notified UNCONDITIONALLY, with only the
              threshold below to adjust when -- so a user who wanted pull
              request notifications and not machine ones had no way to
              say so, because the master switch turned off both or
              neither. #789 gives them categories.

              Under the same master switch, because it is the same
              interruption from the user's side. Two categories rather
              than one, because "is this machine about to die" and "is it
              burning cores for no reason" are different questions and
              wanting one without the other is reasonable.

              Three battery CONDITIONS under one checkbox, though: low
              charge, draining fast and draining while plugged in are one
              subject to a person -- something is wrong with this
              machine's power. */}
          <span className="mt-3 text-sm font-medium">This machine</span>
          <div className="ml-6 flex flex-col gap-2">
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.health_battery ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, health_battery: !prefs.health_battery })
                }
              />
              Battery problems
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.health_cpu ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, health_cpu: !prefs.health_cpu })
                }
              />
              The CPU is busy with nothing in particular
            </label>
            {/* Says what the CPU alert actually looks for, because
                otherwise it reads as "tell me when my machine is busy"
                -- which nobody wants and which is not what it does.
                The aggregate shape is the whole point: ten runaway
                processes each look unremarkable in a sorted list. */}
            <p className="text-xs text-[#8b949e]">
              Sustained CPU use that no single process accounts for — usually
              several runaway or orphaned processes rather than one busy program.
            </p>
            {/* #979. Its own category rather than folded into the two
                above, on the same test `health_cpu` was split from
                `health_battery` by: "a session I was relying on died" and
                "the machine is unwell" are different questions, and
                wanting one without the other is reasonable.

                Defaults ON, which is new-field-defaults-OFF reversed and
                is argued rather than copied -- see `poll.rs`'s field doc.
                The short version is the sentence below: the sweep that
                produces this signal does not run unless the Claude Code
                integration is on, and that defaults off, so this cannot
                interrupt anyone who did not ask for it. */}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!(prefs?.enabled ?? true)}
                checked={prefs?.claude_crashed ?? true}
                onChange={() =>
                  prefs && void setPrefs({ ...prefs, claude_crashed: !prefs.claude_crashed })
                }
              />
              A Claude Code session died
            </label>
            {/* Names the second gate, because a user who has this checked
                and the integration off would otherwise wait for a
                notification that cannot arrive. */}
            <p className="text-xs text-[#8b949e]">
              A session that stopped without ending cleanly — announced once, when it is first
              noticed. Only while the Claude Code integration above is on.
            </p>
          </div>
          {/* A number, not a checkbox, because the only question worth
              asking about LOW CHARGE is WHEN: everyone wants to know
              their laptop is about to die, and they disagree only about
              how much warning they want. The other two conditions --
              draining fast, draining while plugged in -- have no
              threshold to set, because a plugged-in machine losing
              charge is worth saying at any speed. */}
          <label className="mt-1 flex items-center gap-2 text-sm">
            <span>Warn when battery charge falls below</span>
            <input
              type="number"
              min={5}
              max={90}
              className="w-16 rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-sm tabular-nums"
              // Also disabled when battery notifications are off: a
              // threshold for an alert that cannot fire is a control
              // that does nothing, and leaving it live would imply the
              // alert was still coming.
              disabled={!(prefs?.enabled ?? true) || !(prefs?.health_battery ?? true)}
              // `|| 25` renders the stored 0 as the default it actually
              // resolves to in Rust, rather than showing a 0 that reads
              // as "alert at zero percent" -- which is what a user
              // seeing it would reasonably conclude the app will do.
              value={ui?.battery_low_percent || 25}
              onChange={(e) => {
                const next = Number(e.target.value);
                if (ui && Number.isFinite(next)) {
                  void setUi({ ...ui, battery_low_percent: next });
                }
              }}
              aria-label="Battery charge warning threshold, percent"
            />
            <span>%</span>
          </label>
          {/* Says which number this is. "Battery health" normally means
              capacity relative to design -- a different figure entirely,
              shown on the System Health page -- and a threshold labelled
              only "battery" would be read as either. */}
          <p className="text-xs text-[#8b949e]">
            Charge, not capacity. Headstate also warns when the charge is
            falling unusually fast, or falling while the machine is plugged
            in — neither needs a threshold. Never computed across a period
            the app was not running.
          </p>
          {/* "newly breaks" was accurate while every notification was
              breakage. The ready-to-review one is good news, so the
              wording is about the TRANSITION rather than the direction. */}
          <p className="text-xs text-[#8b949e]">
            Only when something newly changes — never repeated for a pull request
            already in that state, and never on first launch.
          </p>
          {/* The PHONE's notifications (#789), on the phone only.

              Its own panel rather than extra checkboxes in the one
              above, because they are a different device's settings. The
              desktop's `get_notify_prefs` is `Class::Local` on the
              remote surface -- the phone cannot read or write it, and
              correctly so: which notifications a desktop shows at that
              desktop is decided there.

              It is also right on the merits. The two devices are in
              different places, and wanting CI failures on the laptop you
              are working at and only new pull requests on the phone in
              your pocket is a reasonable thing to want that one shared
              setting could not express. */}
          {IS_MOBILE_BUILD && <PhoneNotifyPanel />}
        </div>

            </div>
            {/* Rendered always, hidden with CSS -- never unmounted and
                never the `hidden` ATTRIBUTE.
                Unmounting loses a control's state on every tab switch.
                The `hidden` attribute additionally removes the panel
                from the accessibility tree, so a screen reader cannot
                reach a setting until the right topic is clicked -- the
                existing tests caught exactly that, by failing to find
                controls by role. */}
            <div className={section === "cleanup" ? "" : "hidden"}>
        <div className="flex flex-col gap-2 border-t border-[#30363d] pt-4">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-semibold text-[#e6edf3]">Automatic cleanup</h3>
            <HelpButton topic="auto-cleanup" />
          </div>
          <p className="text-xs text-[#8b949e]">
            Reports what it would reclaim. This build never removes anything
            automatically — you review the list and act on it yourself.
          </p>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={cleanup?.enabled ?? false}
              onChange={() =>
                cleanup && void setCleanup({ ...cleanup, enabled: !cleanup.enabled })
              }
            />
            Report what could be reclaimed
          </label>
          {cleanup?.enabled ? (
            <>
              {/* GROUPED: a parent per category, with the specific
                  claims beneath it. These were seven flat checkboxes,
                  which made "branches" and "artifacts" look like the
                  same kind of thing and left no room for the ones that
                  needed adding (#493).

                  A child is a separate claim about what may be deleted
                  with nobody watching -- not a detail of its parent --
                  which is why each has its own stored field. */}
              {CLEANUP_GROUPS.map((g) => {
                const state = parentState(cleanup, g);
                return (
                  <div key={g.key as string} className="ml-6">
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={state === "on"}
                        // A parent with only some children on is
                        // neither: rendering it as plain "on" would
                        // misstate what runs unattended.
                        ref={(el) => {
                          if (el) el.indeterminate = state === "mixed";
                        }}
                        onChange={() =>
                          void setCleanup({ ...cleanup, ...toggleParent(cleanup, g) })
                        }
                      />
                      {g.label}
                      {g.pending ? (
                        <span className="rounded bg-[#21262d] px-1.5 py-0.5 text-[10px] text-[#8b949e]">
                          not yet acted on
                        </span>
                      ) : null}
                    </label>
                    {g.children.map((c) => (
                      <label
                        key={c.key as string}
                        className="ml-6 mt-1 flex items-start gap-2 text-sm"
                      >
                        <input
                          type="checkbox"
                          className="mt-1"
                          checked={Boolean(cleanup[c.key])}
                          onChange={() =>
                            void setCleanup({ ...cleanup, ...toggleChild(cleanup, g, c.key) })
                          }
                        />
                        <span>
                          <span className="block">{c.label}</span>
                          <span className="block text-xs text-[#8b949e]">{c.hint}</span>
                        </span>
                      </label>
                    ))}
                  </div>
                );
              })}
            </>
          ) : null}
        </div>

        {/* `stale_venv_days`, which had no control at all (#957).

            Here rather than under Repositories because it is about what
            gets RECLAIMED, which is this section's subject -- and beside
            the automatic-cleanup block above because both answer "what is
            this app allowed to consider disposable".

            Outside the `cleanup?.enabled` gate deliberately: the
            threshold governs the Artifacts page's Stale badge and its
            checkboxes whether or not the unattended pass is on, so hiding
            it behind that switch would hide the number that labels rows a
            user is looking at right now.

            A SELECT, not a number field. `poll::stale_venv_days` clamps
            to 30..3650 and its comment says why the floor matters --
            "this number gates a delete once the opt-in above is on, so
            the floor is what stops a typo in Settings from making live
            work selectable". A free-text field would have to
            re-implement that clamp on the way in, and a field that
            silently rewrites 7 to 30 leaves the user believing a
            threshold the app does not have. The poll interval's
            `INTERVALS` is the same choice for the same reason. */}
        <div className="mt-5 flex flex-col gap-1 border-t border-[#30363d] pt-4">
          <h3 className="text-sm font-semibold text-[#e6edf3]">Virtualenvs</h3>
          <label className="flex items-center gap-2 text-sm">
            Call a virtualenv stale after
            <select
              className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-sm"
              // Resolved, never the raw stored value: `stale_venv_days`
              // defaults to 0 and every existing install therefore holds
              // one. Rendering that 0 would show a threshold of zero days
              // -- "everything is stale" -- which is the exact misreading
              // the field's own doc comment forbids. `staleVenvDays` is
              // the same `0 ⇒ 90` the backend applies.
              value={staleVenvDays(ui?.stale_venv_days)}
              onChange={(e) => {
                const next = Number(e.target.value);
                // Writes the NUMBER, not 0-for-default. Picking 90 stores
                // 90, so the value shown is the value stored and a later
                // change to the default cannot silently move a threshold
                // the user chose. 0 remains the "never set" reading for
                // an install that has not touched this.
                if (ui && Number.isInteger(next)) {
                  void setUi({ ...ui, stale_venv_days: next });
                }
              }}
              aria-label="Days idle before a virtualenv counts as stale"
            >
              {STALE_DAY_CHOICES.map((d) => (
                <option key={d} value={d}>
                  {staleDaysLabel(d)}
                </option>
              ))}
            </select>
          </label>
          {/* Says what the number actually DOES, on both sides. Labelling
              and deleting are two different consequences of one setting,
              and a user moving it wants to know it widens what a click can
              remove -- not only what a badge says. */}
          <p className="text-xs text-[#8b949e]">
            Sets the Stale badge on the Artifacts page, and the threshold the
            removal re-checks before deleting anything. Orphaned virtualenvs —
            ones whose project directory is gone — are unaffected.
          </p>
        </div>

        {/* Every one of these already worked and none was mentioned
            anywhere in the UI. Escape is the notable one: it hides the
            whole window to the tray, which is genuinely surprising the
            first time someone presses it to dismiss a menu. */}
            </div>
            {/* Rendered always, hidden with CSS, for the reasons given
                on the panels above. */}
            <div className={section === "phone" ? "" : "hidden"}>
        {/* The companion app's switch. Off by default and phrased for
            what it does -- opens a port -- rather than as a feature
            name, so nobody turns it on to see what happens. Pairing
            and the paired-device list follow it below.

            Desktop only. Every command behind these three -- the
            toggle's `get_remote_enabled`/`set_remote_enabled`, the QR's
            `issue_pairing_token`, the list's `list_paired_devices` and
            `revoke_paired_device` -- is `Class::Local`, and deliberately
            so: who may pair with a desktop is a decision made AT that
            desktop. On the phone they were refused before reaching the
            wire, so this whole panel rendered inert. */}
        {IS_MOBILE_BUILD ? <PairedDesktopPanel /> : <>
        <div className="mt-5 flex flex-col gap-2">
          <span className="text-sm font-medium">Phone</span>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={remote}
              onChange={() => {
                setRemoteError(null);
                // Binds a port and, the first time, writes to the
                // keychain; either can refuse. Like autostart, the
                // error is shown and the box reflects what happened.
                void setRemote(!remote).catch((e: unknown) =>
                  setRemoteError(typeof e === "string" ? e : "Could not change this"),
                );
              }}
            />
            Allow phone connections
          </label>
          <p className="text-xs text-[#8b949e]">
            Lets the Headstate companion app reach this desktop on port 41919.
            Only phones you have paired are let in; anything else is refused
            before it can send a request.
          </p>
          {remoteError ? (
            <p role="alert" className="text-xs text-[#f85149]">
              {remoteError}
            </p>
          ) : null}
        </div>
        <GetCompanionPanel />
        <PairPhonePanel />
        <PairedDevicesList />
        </>}
            </div>
            {/* Rendered always and hidden with CSS, like every panel here:
                unmounting would drop the status query and refetch it on
                every topic switch, and the `hidden` attribute would take
                the install buttons out of the accessibility tree. */}
            <div className={section === "claude" ? "" : "hidden"}>
              <ClaudeIntegrationsPanel prefs={ui} setPrefs={setUi} />
            </div>
            {/* Rendered always, hidden with CSS -- never unmounted and
                never the `hidden` ATTRIBUTE.
                Unmounting loses a control's state on every tab switch.
                The `hidden` attribute additionally removes the panel
                from the accessibility tree, so a screen reader cannot
                reach a setting until the right topic is clicked -- the
                existing tests caught exactly that, by failing to find
                controls by role. */}
            <div className={section === "views" ? "" : "hidden"}>
        <div className="mt-5 flex flex-col gap-2">
          <span className="text-sm font-medium">Views</span>
          {/* Derived from `VIEWS`, not a second list. The hand-written
              one here carried four of the nine views, so `my-prs`,
              `branches`, `artifacts`, `packages` and `claude-md` could
              not be hidden at all -- and nothing said so, because a
              partial list looks exactly like a complete one (#675).

              `ALWAYS_OFFERED` is filtered out rather than rendered
              disabled: `ViewSwitcher` shows those whatever is stored,
              so a checkbox for one would be a control that appears to
              work and silently does nothing. A toggle that cannot
              change anything is worse than no toggle. */}
          {VIEWS.filter(({ id }) => !ALWAYS_OFFERED.has(id)).map(({ id, label }) => (
            <label key={id} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={!(ui?.hidden_views ?? []).includes(id)}
                onChange={() => {
                  if (!ui) return;
                  const hidden = ui.hidden_views.includes(id)
                    ? ui.hidden_views.filter((v) => v !== id)
                    : [...ui.hidden_views, id];
                  void setUi({ ...ui, hidden_views: hidden });
                }}
              />
              {label}
            </label>
          ))}
        </div>

            </div>
          </div>

        </div>

        {/* Outside the scrolling region, so OK and Cancel are always
            reachable however long the list grows. */}
        <div className="mt-5 flex shrink-0 justify-end gap-2">
          <button
            type="button"
            onClick={close}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]"
          >
            Cancel
          </button>
          {section !== "gitlab" ? (
            <button
              type="button"
              onClick={save}
              className="rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#1a7f37]"
            >
              Save
            </button>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
