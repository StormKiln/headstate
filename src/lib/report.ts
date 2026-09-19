/// A bug report the user can read before it is posted.
///
/// The banner used to state a problem and offer nothing, and the errors
/// that most need reporting are exactly the ones a user cannot diagnose.
/// Both recent diagnoses in this area needed the platform, the build
/// kind, and how the app was launched -- none of which appear in the
/// error string.

/// What the app knows about itself when something fails.
export interface ReportContext {
  version: string;
  platform: string;
  arch: string;
  error: string;
  /// The view the user was on, if known (#1148).
  ///
  /// "It crashed" and "it crashed on Docker" are different reports, and
  /// the user is the one person who cannot easily tell you which
  /// component threw. Optional because a failure during boot has no
  /// view to name, and guessing one would be worse than omitting it.
  view?: string;
  /// Whether the verbose `[diag]` log was being written.
  ///
  /// Carried because it changes what the maintainer's FIRST reply is: a
  /// report with it off gets "please turn this on and try again", and
  /// one with it on means a log already exists. `undefined` means the
  /// setting could not be read, which is not the same as "off" -- the
  /// report says which (#1042).
  diagnostics?: boolean;
  /// A render crash's component stack, if there was one.
  ///
  /// The single most valuable thing to attach to a crash report, and
  /// until now it was `console.error`'d and nothing else -- invisible
  /// on a release build, where nobody has a console open.
  componentStack?: string;
}

/// The longest error text worth including.
///
/// Deliberately short. A long error is more likely to be carrying
/// something -- a quoted query naming repositories, a stack of paths --
/// and no diagnosis so far has needed more than a sentence of it.
const MAX_ERROR = 500;

/// Patterns for the things that must never leave the machine.
///
/// This is a second line of defence, NOT the design. The report is built
/// from named fields that are known to be safe; this exists because one
/// of those fields -- the error string -- is written by code we do not
/// control and can quote anything.
/// Exported for `redaction.mirror.test.ts`, which asserts the Rust copy
/// in `src-tauri/src/redact.rs` still agrees with this table. Not part
/// of the module's own interface -- `scrub` below is.
export const SCRUB_PATTERNS: [RegExp, string][] = [
  // Every token shape gh can hand out. Checked before paths, since a
  // token can appear inside one.
  [/\b(gh[pousr]|github_pat)_[A-Za-z0-9_]+/g, "[redacted]"],
  // A home directory carries a username; a checkout path can name a
  // private project. Both are leaks the privacy guard exists to stop.
  [/(\/Users\/|\/home\/|C:\\Users\\)[^\s"']*/g, "[path]"],
  // A report goes to a PUBLIC issue tracker, which the diagnostic log
  // does not -- the log names repositories on purpose (its PR action
  // lines are an audit trail) and the user chooses who sees it. A
  // report has no such moment, so it scrubs repo names and the Rust
  // table in `src-tauri/src/redact.rs` deliberately does not.
  // `redaction.mirror.test.ts` asserts that asymmetry stays.
  [/\b[A-Za-z0-9][-\w.]*\/[A-Za-z0-9][-\w.]+\b/g, "[repo]"],
];

function scrub(text: string): string {
  return SCRUB_PATTERNS.reduce((acc, [pattern, with_]) => acc.replace(pattern, with_), text);
}

/// The longest component stack worth including.
///
/// Longer than `MAX_ERROR` because a stack is the point of a crash
/// report, and bounded anyway because a deep tree produces hundreds of
/// frames and a GitHub URL has a practical length limit -- past which
/// the form opens truncated or not at all, which is worse than a short
/// stack. The top frames are the useful ones, so this keeps the START.
const MAX_STACK = 2000;

/// The report body, scrubbed and bounded.
export function buildReport(ctx: ReportContext): string {
  const error = scrub(ctx.error).slice(0, MAX_ERROR);
  // Scrubbed like everything else: a component stack carries file paths
  // in a dev build, and the scrub table is the second line of defence
  // precisely for fields written by code we do not control.
  const stack = ctx.componentStack
    ? scrub(ctx.componentStack).slice(0, MAX_STACK)
    : undefined;
  return [
    "## What happened",
    "",
    "Headstate showed this error:",
    "",
    "```",
    error,
    "```",
    "",
    ...(stack
      ? [
          "### Where",
          "",
          "```",
          stack.trim(),
          "```",
          "",
        ]
      : []),
    "## Environment",
    "",
    `- Headstate ${ctx.version}`,
    `- ${ctx.platform} ${ctx.arch}`,
    // Stated only when known. Omitting the line is honest about not
    // knowing; printing "on this view: unknown" is noise, and printing
    // a guess would be a claim.
    ...(ctx.view ? [`- On the ${ctx.view} view`] : []),
    // Three states, not two (#1042). "Could not read the setting" is
    // not "it was off", and a maintainer acts differently on each.
    ...(ctx.diagnostics === undefined
      ? []
      : [`- Diagnostic logging was ${ctx.diagnostics ? "on" : "off"}`]),
    "",
    "## Anything else",
    "",
    "<!-- What you were doing, and whether it happens every time. -->",
    "",
    "<!-- This report was prepared by Headstate. Tokens, file paths and",
    "     repository names are removed automatically -- but please read it",
    "     before posting. -->",
  ].join("\n");
}

/// A prefilled issue form.
///
/// Opens a form rather than submitting: the user is the only one who can
/// confirm nothing sensitive survived scrubbing, and filing publicly on
/// someone's behalf without showing them what it says is not something
/// an app should do. It also avoids needing issue-write scope on a token
/// this app only reads with.
export function issueUrl(body: string): string {
  const base = "https://github.com/pktstorm/headstate/issues/new";
  return `${base}?title=${encodeURIComponent("Error: ")}&body=${encodeURIComponent(body)}`;
}
