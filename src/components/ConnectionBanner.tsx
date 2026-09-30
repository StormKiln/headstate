import { useState } from "react";
import { type ConnectionState, useConnectionState } from "@/api/connection";
import { REQUIRED_PROTOCOL_VERSION, desktopTooOld } from "@/lib/protocol";
import { useIsMobile } from "@/lib/useIsMobile";
import { relativeTime } from "@/lib/time";
import { ExternalLink } from "./ExternalLink";
import { LATEST_RELEASE_URL } from "@/lib/repo";
import { SettingsDialog } from "./SettingsDialog";
import type { GitHubAuthAvailability } from "@/api/authAvailability";

import type { SourceSelection } from "../store/sourceSelection";
import type { GitLabQueueSnapshot } from "../api/gitlabQueueState";
import { gitlabQueueSummary } from "../lib/gitlabQueueSummary";

/// Where "update Headstate on your desktop" sends the user: the desktop
/// is what needs replacing, and the phone cannot do that for it.
const DESKTOP_RELEASES = LATEST_RELEASE_URL;

/// `pt-safe` because this is the top-most element of the phone's shell:
/// with `viewport-fit=cover` the page paints into the notch strip, and
/// without the inset the banner's text sits under the Dynamic Island.
/// The inset is 0 on the desktop and on phones without a notch.
const BANNER_CLASS =
  "pt-safe flex w-full shrink-0 items-center gap-2 border-b border-[#30363d] bg-[#161b22] px-4 py-2 text-left text-xs text-[#e6edf3] hover:bg-[#21262d]";

/// One line of text per state, and the dot colour beside it.
///
/// Every line names the desktop where there is one: "unreachable" on
/// its own does not say what is unreachable, and a phone paired with
/// two desktops later will need the name to tell them apart.
function describeState(state: Exclude<ConnectionState, { kind: "local" }>, githubAuthAvailable: GitHubAuthAvailability): {
  text: string;
  dot: string;
} {
  switch (state.kind) {
    case "connected":
      if (githubAuthAvailable !== true) {
        return {
          text: `${state.desktop} · reachable · ${githubAuthAvailable === false ? "GitHub is not refreshing" : "GitHub status unavailable"}`,
          dot: "bg-[#d29922]",
        };
      }
      // The desktop is reachable, so its own poll timestamp is the less
      // interesting half: what the reader wants to know is whether the
      // DATA is current, and that is GitHub's freshness. Both were shown,
      // in two strips, saying almost the same thing (#649); this keeps
      // the useful one and the caller supplies it.
      return {
        text: `${state.desktop} · reachable`,
        dot: "bg-[#3fb950]",
      };
    case "connecting":
      return { text: `Connecting to ${state.desktop}…`, dot: "bg-[#58a6ff]" };
    case "unreachable":
      return {
        text: `${state.desktop} is unreachable · last seen ${
          state.lastPoll ? relativeTime(state.lastPoll) : "never"
        }`,
        dot: "bg-[#d29922]",
      };
    case "revoked":
      return {
        text: `${state.desktop} revoked this phone · pair again`,
        dot: "bg-[#f85149]",
      };
    case "unpaired":
      return { text: "Not paired with a desktop · tap to pair", dot: "bg-[#8b949e]" };
    case "unknown":
      return { text: "Desktop connection status unavailable", dot: "bg-[#8b949e]" };
  }
}

/// The strip at the top of every phone screen: which desktop this
/// phone drives, whether it can be reached, and when it last heard
/// from GitHub.
///
/// Renders nothing on the desktop, where the app IS the desktop and
/// there is no connection to describe. Tapping opens Settings on the
/// Phone topic, which is where pairing lives.
export function ConnectionBanner({ updatedAt = 0, githubAuthAvailable = true, selection = "github", gitlab }: {
  updatedAt?: number; githubAuthAvailable?: GitHubAuthAvailability;
  selection?: SourceSelection; gitlab?: GitLabQueueSnapshot;
} = {}) {
  const isMobile = useIsMobile();
  const state = useConnectionState();
  const [settingsOpen, setSettingsOpen] = useState(false);
  if (!isMobile || state.kind === "local") return null;
  if (state.kind === "connected" && desktopTooOld(state.protocolVersion)) {
    // Reachable, and yet not to be driven: a desktop below the protocol
    // this build requires would answer commands it does not understand
    // with 404s, or worse, with a different meaning. Pairing settings
    // cannot fix that, so the banner is a link to the desktop release
    // rather than a button into Settings. No action beyond this is
    // offered from here; the remote transport is what refuses the
    // commands themselves.
    return (
      <ExternalLink
        href={DESKTOP_RELEASES}
        title="Download the latest desktop Headstate"
        className={BANNER_CLASS}
      >
        <span className="h-2 w-2 shrink-0 rounded-full bg-[#f85149]" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">
          Update Headstate on your desktop · this app needs protocol{" "}
          {REQUIRED_PROTOCOL_VERSION}, {state.desktop} has {state.protocolVersion}
        </span>
        <span className="shrink-0 text-[#8b949e]">Update</span>
      </ExternalLink>
    );
  }
  const githubEnabled = selection !== "gitlab";
  const described = describeState(state, githubEnabled ? githubAuthAvailable : true);
  const gitlabSummary = gitlabQueueSummary(gitlab);
  const connected = state.kind === "connected";
  const githubFreshness = connected && githubEnabled && githubAuthAvailable === true && updatedAt > 0
    ? ` · ${selection === "both" ? "GitHub updated" : "PRs updated"} ${relativeTime(new Date(updatedAt).toISOString())}` : "";
  const line = `${described.text}${githubFreshness}${connected && selection !== "github" ? ` · ${gitlabSummary.text}` : ""}`;
  const dot = connected && selection !== "github" && gitlabSummary.warning ? "bg-[#d29922]" : described.dot;
  return (
    <>
      <button
        type="button"
        onClick={() => setSettingsOpen(true)}
        title="Pairing settings"
        className={BANNER_CLASS}
      >
        <span className={`h-2 w-2 shrink-0 rounded-full ${dot}`} aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">{line}</span>
        <span className="shrink-0 text-[#8b949e]">Pairing</span>
      </button>
      {/* Mounted only while open: Settings reads half a dozen
          preference queries, and the banner is on every screen. */}
      {settingsOpen ? (
        <SettingsDialog open onOpenChange={setSettingsOpen} initialSection="phone" />
      ) : null}
    </>
  );
}
