import type { ClaudeHooksStatus } from "@/api/tauri";

/// What the Claude Code hooks status says, as sentences and predicates.
/// Apart from `ClaudeIntegrationsPanel.tsx` so that file exports only
/// components (#1965), which is what lets dev hot-reload swap it in place.

/// One sentence saying what the file says, and what to do about it.
///
/// Kept as a pure function of the status so it can be tested without
/// rendering, and so the four states cannot collapse into three: the whole
/// point of the third and fourth being separate is that "cannot tell" and
/// "not installed" have opposite remedies.
export function statusLine(status: ClaudeHooksStatus | undefined): {
  /// What to tell the user.
  text: string;
  /// Whether this is a problem they have to act on. Drives the colour, and
  /// `alert` on the element, so a screen reader hears it.
  problem: boolean;
} {
  if (!status) return { text: "Checking ~/.claude/settings.json…", problem: false };
  switch (status.state) {
    case "installed":
      return {
        text: `Installed. Claude Code runs ${status.command} when a session starts and ends.`,
        problem: false,
      };
    case "not_installed":
      return {
        text:
          "Not installed. Sessions are still listed from the transcripts Claude Code " +
          "writes, but without a process id — so Headstate cannot tell a running " +
          "session from one that was killed.",
        problem: false,
      };
    case "stale":
      return { text: `Needs reinstalling: ${status.detail}`, problem: true };
    case "cannot_tell":
      // The refusal's own sentence, because it is the only actionable thing
      // we have -- and for the malformed case it is the ONLY warning the
      // user will ever get that their hooks are dead.
      return { text: refusalText(status), problem: true };
  }
}

/// A refusal as a sentence, with the fix in it.
function refusalText(r: Extract<ClaudeHooksStatus, { state: "cannot_tell" }>): string {
  switch (r.kind) {
    case "malformed":
      return (
        `${r.path} is not valid JSON (${r.detail}). Headstate will not rewrite it. ` +
        "Claude Code silently ignores a settings file it cannot parse, so every hook " +
        "in this file — including any you rely on from other tools — is already doing " +
        "nothing. Fix the JSON by hand, then install."
      );
    case "hooks_not_an_object":
      return (
        `${r.path} has a "hooks" key that is ${r.found} rather than an object. ` +
        "Headstate will not overwrite it — fix it by hand, then install."
      );
    case "matcher_not_understood":
      return (
        `${r.path} has an entry under "${r.event}" that Headstate does not understand, ` +
        "so it cannot tell whether it is its own. Fix it by hand, then install."
      );
    case "io":
      return `Could not read ${r.path}: ${r.detail}`;
  }
}

/// The settings file the current status is ABOUT, or `null` (#961).
///
/// A function rather than an inline narrowing, because the four refusal
/// kinds are a union and only the `cannot_tell` arm has a path at all: the
/// `installed`, `not_installed` and `stale` states are answers about a file
/// that was read successfully, so there is no path to offer and no reason
/// to offer one -- the panel already says `~/.claude/settings.json` in
/// prose there.
///
/// Kept as a pure function of the status, like `statusLine` above and for
/// the same reason: it is the predicate that decides whether three controls
/// render, so it should be testable without a DOM.
export function refusalPath(status: ClaudeHooksStatus | undefined): string | null {
  return status?.state === "cannot_tell" ? status.path : null;
}
