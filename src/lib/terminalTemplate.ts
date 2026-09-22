/// What the app can say about a terminal template before saving it.
///
/// Rust is the authority: `claude::launch::Template::parse` is what
/// actually runs, and it re-validates every launch. This exists so the
/// settings field can say "this will not work" while the user is typing
/// rather than at the moment they press a button and nothing happens.
///
/// Deliberately a SUBSET of the Rust rules, not a reimplementation. It
/// checks the two things a user gets wrong -- a missing placeholder and
/// an unbalanced quote -- and says nothing about the rest. A second full
/// parser in another language would be a second thing to keep in step,
/// and the failure mode of drifting is this one silently accepting a
/// template Rust then refuses.

/// The placeholder the built command is substituted into.
///
/// Must match `claude::launch::PLACEHOLDER`.
export const PLACEHOLDER = "{command}";

/// A short reason the template is unusable, or null if nothing is
/// obviously wrong with it.
///
/// Null does NOT mean "valid" -- it means this check found nothing. The
/// empty string is null too, because empty means "no terminal
/// configured", which is the default and not an error.
export function templateProblem(raw: string): string | null {
  const t = raw.trim();
  if (t === "") return null;

  const count = t.split(PLACEHOLDER).length - 1;
  if (count === 0) return `Add ${PLACEHOLDER} where the command should go.`;
  if (count > 1) return `${PLACEHOLDER} may only appear once.`;

  // The program is argv[0] and gets EXECUTED; a command line substituted
  // there would be run as a program name.
  if (t.startsWith(PLACEHOLDER)) return `${PLACEHOLDER} cannot be the program name.`;

  // Counted outside quotes only, the way `split_words` reads them, so
  // an apostrophe inside a double-quoted path is not a false alarm.
  let quote: string | null = null;
  for (let i = 0; i < t.length; i += 1) {
    const c = t[i];
    if (c === "\\") {
      i += 1;
      continue;
    }
    if (quote === null && (c === "'" || c === '"')) quote = c;
    else if (c === quote) quote = null;
  }
  if (quote !== null) return "There is an unclosed quote.";

  return null;
}

/// Ready-made templates for the terminals people actually use.
///
/// Offered rather than detected. `claudify_command` records why nothing
/// is guessed: macOS has no default-terminal concept, so a machine with
/// both Terminal.app and iTerm gives no way to know which the user
/// wants. Picking from a list is the user answering that question once.
/// The two macOS presets drive the app with AppleScript rather than
/// `open -a` (#1302).
///
/// `open -a Terminal {command}` and `open -a iTerm {command}` -- the
/// presets these replace -- could not run a command at all. `open -a`
/// takes FILE PATHS, so it treated the whole `cd … && claude …` line as
/// a filename, printed "The file … does not exist" and exited 1 without
/// opening anything. Both shipped; the iTerm one is what #1302 was
/// reported against.
///
/// `osascript` is the smallest thing that actually works. Each was run
/// on a real machine against a canary file before being written here,
/// which is the only check that would have caught the bug being fixed:
/// the broken `open -a` templates were plausible strings and passed
/// every string-level test in the repository.
///
/// The first `-e` opens an `on run argv` handler, so the command
/// arrives as a POSITIONAL ARGUMENT rather than being pasted into the
/// script text. That matters for the same reason `Template::render`
/// keeps it in one argv slot: a brief full of quotes and `$(…)` reaches
/// the shell byte-for-byte, and nothing in it is ever parsed as
/// AppleScript.
export const PRESETS: { label: string; template: string }[] = [
  {
    label: "Terminal (macOS)",
    template: `/usr/bin/osascript -e 'on run argv' -e 'tell application "Terminal"' -e 'do script (item 1 of argv)' -e 'activate' -e 'end tell' -e 'end run' ${PLACEHOLDER}`,
  },
  {
    label: "iTerm (macOS)",
    template: `/usr/bin/osascript -e 'on run argv' -e 'tell application "iTerm"' -e 'activate' -e 'tell (create window with default profile) to tell current session to write text (item 1 of argv)' -e 'end tell' -e 'end run' ${PLACEHOLDER}`,
  },
  { label: "GNOME Terminal", template: `gnome-terminal -- bash -lc ${PLACEHOLDER}` },
  { label: "Konsole", template: `konsole -e bash -lc ${PLACEHOLDER}` },
  { label: "WezTerm", template: `wezterm start -- bash -lc ${PLACEHOLDER}` },
];

/// Templates that are known not to run a command, and why (#1302).
///
/// A user who picked the old macOS preset has the broken string SAVED
/// in their prefs. Replacing the preset list above does not rewrite it:
/// they would keep pressing Run and keep seeing nothing, and the error
/// the launcher now raises would tell them the terminal "exited
/// immediately" without saying that their setting is the cause.
///
/// Detect-and-warn rather than silent migration. Rewriting a user's
/// stored setting behind their back is the kind of thing this app
/// should not do -- the template is a field they typed into, some users
/// will have edited it, and a migration that guessed wrong would be
/// undebuggable. Naming the problem next to the field, with a preset
/// one click away, leaves the change theirs to make.
export function brokenTemplateWarning(raw: string): string | null {
  const t = raw.trim();
  if (t === "") return null;
  // `open -a <App> {command}`, however it is spelled: an absolute path
  // to `open` or a bare `open`, and any application name.
  if (/(^|\/)open(\s+-[a-zA-Z]+)*\s+-a\b/.test(t) && t.includes(PLACEHOLDER)) {
    return "`open -a` takes file paths, not commands, so this opens a terminal without running anything. Pick a preset below.";
  }
  return null;
}
