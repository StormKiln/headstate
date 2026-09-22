import { useState } from "react";
import { toast } from "sonner";
import { useClaudeHookInventory, useClaudeHooks } from "@/api/hooks";
import { EffectiveSettingsPanel } from "./EffectiveSettingsPanel";
import { ConfigHealthPanel } from "./ConfigHealthPanel";
import { claudeRevealPath, type ClaudeHooksStatus, type UiPrefs } from "@/api/tauri";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import {
  PLACEHOLDER,
  PRESETS,
  brokenTemplateWarning,
  templateProblem,
} from "@/lib/terminalTemplate";

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
  const { status, install, reinstall, uninstall, reread } = useClaudeHooks();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const enabled = prefs?.claude_integrations_enabled ?? false;

  /// The template as TYPED, which is not the saved value until blur.
  ///
  /// Seeded from prefs and deliberately not re-synced on every render:
  /// overwriting the field from props while someone is typing in it is
  /// the classic controlled-input bug, and `useUiPrefs` refetches.
  const [terminal, setTerminal] = useState(prefs?.terminal_command ?? "");
  const problem = templateProblem(terminal);
  /// A template that PARSES but cannot run anything (#1302).
  ///
  /// Separate from `problem`, and shown even though the template is
  /// well-formed, because the two need different handling: a malformed
  /// template is refused at save, while this one is already saved in
  /// the prefs of every user who clicked the old macOS preset. Blocking
  /// the save would strand them -- they cannot clear a field they are
  /// not editing -- so this warns and leaves the value alone.
  const broken = problem === null ? brokenTemplateWarning(terminal) : null;

  /// Persist the template, unless it is obviously broken.
  ///
  /// Refusing to SAVE a bad one rather than saving it and failing at
  /// launch: the error belongs where the mistake was made. The typed
  /// text stays in the field so it can be corrected -- discarding it
  /// would punish a typo by deleting the work.
  const saveTerminal = (next?: string) => {
    const value = (next ?? terminal).trim();
    if (!prefs) return;
    if (value !== "" && templateProblem(value) !== null) return;
    if (value === (prefs.terminal_command ?? "")) return;
    void setPrefs({ ...prefs, terminal_command: value });
  };
  const line = statusLine(status);
  // The file the refusal is about, and the gate on all three of #961's
  // controls. `null` for every state that read the file successfully.
  const brokenPath = refusalPath(status);

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

      {/* Desktop only: `claude_launch_*` is `Class::Local`, so a phone
          would get a refusal from a control that looked available. */}
      {IS_MOBILE_BUILD ? null : (
        <div className="mt-3 flex flex-col gap-1">
          <span className="text-sm font-medium">Terminal</span>
          <p className="text-xs text-[#8b949e]">
            Set this and the Claudify and Resume buttons open your terminal instead of
            copying. Leave it empty and they copy, exactly as they do now — Headstate
            never guesses a terminal, because there is no reliable way to know which one
            you use.
          </p>
          <input
            type="text"
            value={terminal}
            spellCheck={false}
            // Not saved on every keystroke: a half-typed template is
            // not a preference, and writing one would make the buttons
            // launch something broken between two keys.
            onChange={(e) => setTerminal(e.target.value)}
            onBlur={() => saveTerminal()}
            onKeyDown={(e) => e.key === "Enter" && saveTerminal()}
            // A shape that actually runs a command. The old placeholder
            // was `open -a Terminal {command}`, which #1302 established
            // runs nothing -- suggesting it in the empty field taught
            // the broken form to anyone who typed their own.
            placeholder={`gnome-terminal -- bash -lc ${PLACEHOLDER}`}
            aria-label="Terminal command"
            aria-invalid={problem !== null}
            className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-xs text-[#e6edf3]"
          />
          {/* Said while typing rather than at launch: the alternative
              is a button that does nothing and a user with no idea
              why. */}
          {problem ? (
            <p className="text-xs text-[#f85149]" role="alert">
              {problem}
            </p>
          ) : broken ? (
            <p className="text-xs text-[#d29922]" role="alert">
              {broken}
            </p>
          ) : (
            <p className="text-xs text-[#8b949e]">
              <code>{PLACEHOLDER}</code> is replaced with the command to run.
            </p>
          )}
          <div className="mt-1 flex flex-wrap gap-1">
            {PRESETS.map((p) => (
              <button
                key={p.label}
                type="button"
                onClick={() => {
                  setTerminal(p.template);
                  saveTerminal(p.template);
                }}
                className="rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#8b949e] hover:bg-[#21262d] hover:text-[#e6edf3]"
              >
                {p.label}
              </button>
            ))}
          </div>
        </div>
      )}

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
        {/* #961: the controls the refusal's own instruction requires.
            "Fix the JSON by hand, then install" names a button that is
            `disabled` in this state -- correctly, since a malformed file
            must not offer the button whose whole job is to refuse -- so
            after the hand-edit there was nothing on screen that re-read
            the file. `refetchOnWindowFocus` is not an answer: it is
            invisible, and it never fires for a user who does not leave
            the window.

            Rendered only for `cannot_tell`. In the other three states the
            file was read, the prose below already names it, and a
            re-check button beside a working status is furniture.

            NOT inside the `IS_MOBILE_BUILD` branch below, and that is
            deliberate: `claude_hooks_status` is `Class::Read` and
            `copyText` is frontend-only, so both work on the phone. Only
            the reveal is `Class::Local` and it is gated on its own. */}
        {/* `void reread()`, not `reread` itself: `invalidateQueries` returns
            a promise and `onReread` is a `() => void`, so passing it bare
            hands a promise to a handler that will not await it. Nothing here
            wants the result -- the re-read's outcome arrives as a new
            `status`, which is the whole point of `invalidateQueries` over
            `setQueryData` -- so discarding it explicitly is the honest
            spelling rather than widening the prop's type to hide it. */}
        {brokenPath !== null ? (
          <RefusalActions path={brokenPath} onReread={() => void reread()} />
        ) : null}
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
                  // The reason on hover as well as beside the button, per
                  // `RevealButton`'s argument (#919, #961): the visible
                  // text is what a keyboard or screen-reader user gets,
                  // the title is what a mouse user reaching for a greyed
                  // control looks for. The disabled Install had neither,
                  // so the amber paragraph above and the dead button
                  // below were two things the reader had to connect
                  // themselves.
                  title={
                    brokenPath !== null
                      ? `Headstate cannot read ${brokenPath}, so it will not write to it.`
                      : undefined
                  }
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
            {/* The VISIBLE reason the button above is dead (#961). One
                short line, not only a `title` -- the same case
                `RevealButton` makes at length for the reveal buttons two
                views over. Worded as a statement about the file rather
                than a repeat of the amber paragraph's instruction: the
                paragraph says what to do, this says why the control next
                to it cannot. */}
            {brokenPath !== null ? (
              <p className="text-xs text-[#8b949e]">
                Install is unavailable while that file cannot be read — Headstate refuses
                to write to a settings file it could not parse, rather than overwriting
                what another tool put there.
              </p>
            ) : null}
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
      <HookInventorySection enabled={enabled} />
      <EffectiveSettingsPanel enabled={enabled} />
      <ConfigHealthPanel enabled={enabled} />
    </div>
  );
}

/// The three things a user with a broken settings file needs (#961).
///
/// The panel's own message ends "Fix the JSON by hand, then install", and
/// every one of those words named something the user could not do: the file
/// was a path interpolated into a sentence, there was no re-check after the
/// hand-edit, and Install was `disabled`. Two of the three gaps are closed
/// here and the third -- the reason Install is dead -- is stated beside the
/// button itself, where a reader looking at a greyed control will find it.
///
/// # Why "Check again" and not "enable Install"
///
/// Install stays disabled, which is the load-bearing half of the
/// three-state status: a malformed file must not offer the button whose
/// whole job is to refuse. "Check again" re-reads and lets the status
/// decide -- so a fixed file turns into an enabled Install by the status
/// changing, which is the only way that button should ever become live.
///
/// It is also not greyed by the integrations switch, deliberately. The
/// panel already argues that greying the install controls "would imply the
/// switch disables them", and a panel that cannot re-check while the view
/// is off is the same dead end that argument names.
///
/// # Three controls, three surface classes
///
/// | control | class | phone |
/// |---|---|---|
/// | Check again | `claude_hooks_status` is `Read` | yes |
/// | Copy path | no command at all -- `copyText` | yes |
/// | Reveal | `claude_reveal_path` is `Class::Local` | NO |
///
/// So only the reveal is gated, and on `IS_MOBILE_BUILD` rather than
/// `useIsMobile()`: a desktop user who drags the window narrow still has a
/// Finder. The sentence in its place is not decoration -- an absent button
/// with no explanation leaves the reader unable to tell "this app has no
/// such action" from "not from here", which is the distinction
/// `RevealButton` exists to preserve.
///
/// Copy is the one that matters most on the phone, and the reason it is
/// rendered there rather than hidden with the reveal: a path the user can
/// put in a message to themselves is the only one of the three that helps
/// when the Mac with the broken file is in another room.
function RefusalActions({
  path,
  onReread,
}: {
  /// The settings file the refusal names. Never `null` here -- the caller
  /// gates on `refusalPath` -- so this component has no absent case and
  /// does not invent wording for one.
  path: string;
  /// `useClaudeHooks`'s `reread`. The same invalidation the three writes
  /// use in their `.finally`, so a hand-edit is judged by exactly the code
  /// an install would have run.
  onReread: () => void;
}) {
  const copy = async () => {
    const failure = await copyText(path);
    if (failure) {
      // The REASON. `copyText` distinguishes an insecure context from a
      // rejected write and the two have different remedies, so "could not
      // copy" alone would leave the user with nothing to do -- which is
      // the defect this whole component is fixing, reproduced one level
      // down.
      toast.error(`Could not copy the path: ${failure}`);
      return;
    }
    toast.success("Copied the settings file path.");
  };

  const reveal = () => {
    void claudeRevealPath(path).then(
      () => {},
      (e: unknown) => {
        // Shown rather than swallowed, like every other refusal on this
        // panel. `claude_reveal_path` falls back to returning the path
        // where revealing is unsupported, so a rejection here is a real
        // failure and not a platform gap.
        toast.error(typeof e === "string" ? e : "Could not reveal the settings file");
      },
    );
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={onReread}
          className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
        >
          Check again
        </button>
        <button
          type="button"
          onClick={() => void copy()}
          className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
        >
          Copy path
        </button>
        {IS_MOBILE_BUILD ? null : (
          <button
            type="button"
            onClick={reveal}
            className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
          >
            Reveal in Finder
          </button>
        )}
      </div>
      {IS_MOBILE_BUILD ? (
        <p className="text-xs text-[#8b949e]">
          Revealing the file needs a Finder this phone cannot see, so it is opened at that
          Mac. The path above copies here.
        </p>
      ) : null}
      <p className="text-xs text-[#8b949e]">
        Check again re-reads the file without writing to it, so a hand-fix shows up here
        as soon as it is saved.
      </p>
    </div>
  );
}

/// Every hook in the file, ours and everyone else's (#1127).
///
/// Collapsed by default. The question it answers -- "what is actually
/// wired into my sessions" -- is one a user asks when something is
/// wrong, not on every visit to Settings, and `useClaudeHookInventory`
/// does not read the file until this is opened.
function HookInventorySection({ enabled }: { enabled: boolean }) {
  const [open, setOpen] = useState(false);
  const { data, error } = useClaudeHookInventory(enabled && open);

  if (!enabled) return null;

  return (
    <div className="mt-2 border-t border-[#21262d] pt-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="tap-target text-xs text-[#58a6ff] hover:underline"
      >
        {open ? "Hide" : "Show"} every hook in this file
      </button>
      {open ? (
        error ? (
          // The refusal's own sentence, never an empty table. A file
          // that could not be parsed is not a file with no hooks, and
          // rendering nothing would say the second.
          <p role="alert" className="mt-2 text-xs text-[#f85149]">
            {String(error instanceof Error ? error.message : error)}
          </p>
        ) : !data ? (
          <p className="mt-2 text-xs text-[#8b949e]">Reading…</p>
        ) : data.events.length === 0 ? (
          <p className="mt-2 text-xs text-[#8b949e]">
            This file configures no hooks at all.
          </p>
        ) : (
          <div className="mt-2 space-y-2">
            {data.events.map((ev) => (
              <div key={ev.event}>
                <p className="text-xs font-semibold text-[#e6edf3]">{ev.event}</p>
                <ul className="mt-0.5 space-y-0.5">
                  {ev.matchers.map((m, i) => (
                    <li
                      key={`${ev.event}:${i}`}
                      className={`text-[11px] ${m.ours ? "text-[#3fb950]" : "text-[#8b949e]"}`}
                    >
                      {/* Ours is marked rather than merely coloured:
                          colour alone is not an answer for a reader who
                          cannot see it. */}
                      <span className="mr-1">{m.ours ? "[Headstate]" : "[other]"}</span>
                      {/* An ABSENT matcher means "every tool", which is
                          a different statement from a pattern that
                          happens to be empty. */}
                      <span className="mr-1 text-[#8b949e]">
                        {m.matcher === null ? "(every tool)" : m.matcher}
                      </span>
                      <code className="break-all">{m.commands.join(" ; ")}</code>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )
      ) : null}
    </div>
  );
}
