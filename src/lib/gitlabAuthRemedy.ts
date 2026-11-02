import type { GitLabAuthState } from "@/api/tauri";

/// What a failed GitLab sign-in check can offer. A retry checks the SAME
/// host again, so it is offered only where something outside Headstate
/// (`glab auth login`, the network) can change the answer. When the check
/// ran against a host glab is not signed in to, only the setting can fix
/// it, and a bare retry would fail identically (#1050).
export interface GitLabAuthRemedy {
  /// The one other host glab is signed in to, offered as a one-click save.
  useHost: string | null;
  /// Open Settings › GitLab. Desktop only: the phone has no GitLab topic.
  settings: boolean;
  retry: boolean;
}

export function gitlabAuthRemedy(state: GitLabAuthState | undefined, mobileBuild: boolean): GitLabAuthRemedy {
  // No answer at all (the command itself failed): asking again can work.
  if (state === undefined) return { useHost: null, settings: false, retry: true };
  if (state.ok) return { useHost: null, settings: false, retry: false };
  if (state.issue === "invalidHost") return { useHost: null, settings: !mobileBuild, retry: false };
  const others = (state.signedInHosts ?? []).filter((host) => host !== state.host);
  if (state.issue === "unverified" && others.length > 0) {
    return { useHost: !mobileBuild && others.length === 1 ? others[0] : null, settings: !mobileBuild, retry: false };
  }
  return { useHost: null, settings: !mobileBuild && state.configured === false, retry: true };
}

/// Name the host the GitLab queues actually use. "The host configured in
/// Settings" read as a promise when no host was saved and gitlab.com was
/// being used in its place.
export function gitlabHostLine(host: string | undefined, configured: boolean | undefined): string {
  if (!host) return "GitLab queues use the host configured in Settings on the desktop.";
  if (configured === false) return `GitLab queues use ${host}, the default. No GitLab host is saved in Settings on the desktop.`;
  return `GitLab queues use ${host}, set in Settings on the desktop.`;
}
