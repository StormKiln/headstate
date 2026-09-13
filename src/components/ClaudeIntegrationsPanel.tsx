import { useState } from "react";
import { toast } from "sonner";
import { useClaudeHooks } from "@/api/hooks";
import type { ClaudeHooksStatus, UiPrefs } from "@/api/tauri";
import { IS_MOBILE_BUILD } from "@/lib/target";

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

/// Settings › Claude Code.
///
/// Two controls with deliberately different meanings, and the distinction is
/// the thing this panel has to communicate rather than merely implement.
///
/// # The switch hides; it does NOT uninstall
///
/// Settled in epic #910 §6 and restated in #915. Toggling the integrations
/// off for an afternoon must not uninstall the hooks, because that would
/// create a PERMANENT hole in the session history: every session started
/// while it was off would be unrecorded, and no later re-enable can recover
/// it. The transcript backstop recovers that a session existed, but never its
/// process id and never why it started — those exist in no other source.
///
/// So the switch means "hide the view and stop consuming", and install /
/// uninstall is its own explicit control. The consequence has to be said out
/// loud rather than implied, and this panel says it: a disabled Headstate
/// still leaves a hook installed that writes to disk. A user who reads "off"
/// as "nothing is running" would be wrong, and finding that out from a file
/// they did not expect is worse than being told.
///
/// # Why the panel stays visible when the switch is off
///
/// Greyed, not hidden -- per the epic. It is where the switch lives, so
/// hiding it would remove the only way back. The install controls stay
/// usable while it is off, deliberately: someone who has turned the view off
/// for a while still has a legitimate reason to remove the hook, and a
/// disabled panel that cannot uninstall would be a dead end.
export function ClaudeIntegrationsPanel({
  prefs,
  setPrefs,
}: {
  /// From `useUiPrefs`. Passed in rather than read here, because
  /// `SettingsDialog` already holds it and two subscriptions to the same
  /// query would let the checkbox and the rest of the dialog disagree for a
  /// render.
  prefs: UiPrefs | undefined;
  setPrefs: (prefs: UiPrefs) => Promise<void>;
}) {
  const { status, install, reinstall, uninstall } = useClaudeHooks();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const enabled = prefs?.claude_integrations_enabled ?? false;
  const line = statusLine(status);

  // Every one of these edits a file outside this app and can genuinely
  // refuse, so the error is SHOWN. The same rule autostart and the phone
  // switch follow, and for a stronger reason here: the refusal text is the
  // only warning a user with a broken settings file will ever get.
  const run = (
    what: "install" | "reinstall" | "uninstall",
    action: () => Promise<{ added?: string[]; replaced?: string[]; removed?: string[]; was_absent?: boolean }>,
  ) => {
    setBusy(true);
    setError(null);
    void action().then(
      (result) => {
        setBusy(false);
        if (what === "uninstall") {
          toast.success(
            result.was_absent
              ? "There was nothing of Headstate's to remove"
              : `Removed the hook from ${(result.removed ?? []).join(" and ")}`,
          );
          return;
        }
        // A replacement is REPORTED, per §5.4: silently reverting someone's
        // hand-edit is its own defect, and this toast is where they find out.
        const replaced = result.replaced ?? [];
        toast.success(what === "install" ? "Installed the hook" : "Reinstalled the hook", {
          description:
            replaced.length > 0
              ? `Replaced the existing Headstate entry under ${replaced.join(" and ")}.`
              : undefined,
        });
      },
      (e: unknown) => {
        setBusy(false);
        setError(typeof e === "string" ? e : "Could not change the hooks");
      },
    );
  };

  return (
    <div className="mt-5 flex flex-col gap-2">
      <span className="text-sm font-medium">Claude Code</span>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={enabled}
          onChange={() =>
            prefs &&
            void setPrefs({
              ...prefs,
              claude_integrations_enabled: !prefs.claude_integrations_enabled,
            })
          }
        />
        Show Claude Code sessions
      </label>
      {/* Greyed further when the switch is off -- the epic asks for a greyed
          section, and THIS is the part the switch actually governs: a
          description of a view that is not currently offered.

          Deliberately not the install controls below. Greying those would
          imply the switch disables them, which is the exact conflation §6
          settled against -- and someone who turned the view off months ago
          still has a legitimate reason to remove the hook. A panel that
          cannot uninstall while disabled is a dead end. */}
      <p className={`text-xs ${enabled ? "text-[#8b949e]" : "text-[#6e7681]"}`}>
        Lists the sessions on this machine, which of them are still running, and the
        command to resume one that is not.
        {enabled ? null : " Hidden while this is off."}
      </p>

      {/* THE thing this panel exists to say. Shown whatever the switch is
          set to, because it is true whatever the switch is set to -- and it
          is the sentence that stops "off" being read as "nothing is
          running". */}
      <p className="text-xs text-[#8b949e]">
        Turning this off hides the view and stops Headstate reading the session log. It
        does <strong className="font-medium text-[#e6edf3]">not</strong> remove the hook —
        that is the separate control below, on purpose: a hook removed for an afternoon
        leaves a permanent gap in the history, because a session&rsquo;s process id can
        never be recovered afterwards.
      </p>

      {/* The install controls. Not on the phone: all three commands are
          Class::Local and would be refused before reaching the wire, so
          rendering them there would be three buttons that cannot work.
          The STATUS is a Read and is shown on both. */}
      <div className="mt-3 flex flex-col gap-2 border-t border-[#30363d] pt-3">
        <span className="text-sm font-medium">The session hook</span>
        <p
          className={`text-xs ${line.problem ? "text-[#d29922]" : "text-[#8b949e]"}`}
          role={line.problem ? "alert" : undefined}
        >
          {line.text}
        </p>
        {IS_MOBILE_BUILD ? (
          <p className="text-xs text-[#8b949e]">
            Installing and removing the hook edits a configuration file shared with other
            tools, so it is done at that Mac rather than from here.
          </p>
        ) : (
          <>
            <div className="flex flex-wrap gap-2">
              {/* Install and Reinstall are the SAME operation and share one
                  code path (§5.3). Two buttons because they are two
                  intentions -- "set this up" and "repair this" -- and only
                  one of them is the right thing to offer at a time. */}
              {status?.state === "installed" || status?.state === "stale" ? (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => run("reinstall", reinstall)}
                  className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50"
                >
                  Reinstall
                </button>
              ) : (
                <button
                  type="button"
                  // Disabled for `cannot_tell` and while the first read is
                  // in flight. This is the load-bearing half of the
                  // three-state status: a malformed file must not offer the
                  // button whose whole job is to refuse.
                  disabled={busy || !status || status.state === "cannot_tell"}
                  onClick={() => run("install", install)}
                  className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50"
                >
                  Install
                </button>
              )}
              {/* Offered whenever something of ours might be there -- which
                  includes `stale`, since a stale entry is exactly the one a
                  user might want gone rather than repaired. Not offered for
                  `cannot_tell`, because we genuinely do not know. */}
              {status?.state === "installed" || status?.state === "stale" ? (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => run("uninstall", uninstall)}
                  className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50"
                >
                  Remove
                </button>
              ) : null}
            </div>
            <p className="text-xs text-[#8b949e]">
              Adds one entry to <code>~/.claude/settings.json</code>, beside whatever is
              already there. Anything another tool put in that file is left alone, and a
              file Headstate cannot read is refused rather than rewritten.
            </p>
          </>
        )}
        {error ? (
          <p role="alert" className="text-xs text-[#f85149]">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
