//! Opening the user's own terminal on a command this app already built.
//!
//! Every Claude action in Headstate ended at the clipboard: copy a
//! string, switch to a terminal, paste -- while the app already knew the
//! working directory, the session id and the binary path (#1126).
//!
//! # What the clipboard argument actually said
//!
//! `commands::claudify_command` and [`super::sessions`] both record why
//! nothing is spawned: macOS has no default-terminal concept (no
//! LaunchServices handler, so a machine with both Terminal.app and iTerm
//! gives no way to know which the user wants), and on Linux
//! `x-terminal-emulator` is Debian-only.
//!
//! That rules out GUESSING a terminal. It does not rule out asking the
//! user once which one they use, which is all this module does. With no
//! template configured there is no launch path at all and the clipboard
//! remains the only route -- so the old rationale still holds wherever
//! it was true, and this is not a replacement for it.
//!
//! # The template is config, not input
//!
//! [`Template`] is a string the user typed into settings holding a
//! `{command}` placeholder. Headstate substitutes the already-built
//! shell line into it and runs the result.
//!
//! This is deliberately NOT a sandbox. A user who can edit their own
//! settings can already run anything as themselves, so a template that
//! runs an arbitrary program is not an escalation -- it is the feature.
//! What matters is the property the clipboard path already guarantees
//! and this must not lose: **the line that ends up running is the line
//! the button said it would run**. `resume_command` shell-quotes the
//! path and the session id precisely so a directory named `$(whoami)`
//! cannot inject; substituting that quoted string into a template keeps
//! its quoting intact, and [`Template::render`] never re-quotes or
//! unescapes it.
//!
//! # Argv, not a shell
//!
//! The template is split into argv on unquoted whitespace and spawned
//! directly -- it is NOT passed to `sh -c`. The command string lands in
//! exactly one argv slot, whatever it contains, so no amount of shell
//! metacharacter in a session id or a path can become a second word.
//! Handing the whole thing to a shell would undo `resume_command`'s
//! quoting one level up and is the obvious way to reintroduce the bug
//! it was written to fix.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// The placeholder a template must contain.
///
/// Spelled out rather than implied by position: a template that simply
/// appended the command would work for `Terminal.app` and silently
/// produce the wrong argv for anything taking its own trailing flags.
pub const PLACEHOLDER: &str = "{command}";

/// Why a launch did not happen.
///
/// Every arm is a DIFFERENT remedy, which is why this is an enum and not
/// a `String`: "you have not set a terminal up" sends the user to
/// settings, "the directory is gone" does not, and telling them the
/// wrong one costs them the time it takes to find out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchError {
    /// No template configured. The caller should not have offered the
    /// button at all, so this is the backstop rather than the message a
    /// user is expected to see.
    NotConfigured,
    /// The template is set but unusable, with the reason.
    BadTemplate { why: String },
    /// The working directory the command would `cd` into is gone.
    ///
    /// Refused rather than launched: a terminal that opens and
    /// immediately fails its own `cd` leaves the user in an unrelated
    /// directory with a Claude session about to start there. The
    /// clipboard path states this as a caveat and lets the user decide;
    /// spawning on their behalf has to be stricter, because they are
    /// not reading the line before it runs.
    CwdMissing { path: String },
    /// The terminal program could not be started.
    Spawn { why: String },
    /// The program started and then exited non-zero almost at once, so
    /// no terminal was opened (#1302).
    ///
    /// Distinct from [`Self::Spawn`], which is "the binary could not be
    /// executed at all". This one ran and then refused -- `open -a iTerm
    /// 'cd … && claude'` is the case that motivated it: `open` takes
    /// file paths, so it reports "the file … does not exist" on stderr
    /// and exits 1, having opened nothing. Before this arm existed the
    /// launcher called that a success and the user saw nothing happen.
    ExitedImmediately {
        /// The exit status as the OS reported it, for the case where
        /// the program printed nothing at all.
        status: String,
        /// Whatever the program wrote to stderr, trimmed. Often the
        /// only part that says what is actually wrong.
        stderr: String,
    },
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(
                f,
                "No terminal is configured. Set one in Settings to open commands directly."
            ),
            Self::BadTemplate { why } => write!(f, "The configured terminal is unusable: {why}"),
            Self::CwdMissing { path } => write!(
                f,
                "The directory this command would open in is gone ({path}), so nothing was launched."
            ),
            Self::Spawn { why } => write!(f, "Could not start the configured terminal: {why}"),
            Self::ExitedImmediately { status, stderr } => {
                // The stderr line is what actually names the problem
                // ("The file … does not exist"), so it leads when there
                // is one. The status is the fallback for a program that
                // failed silently, which would otherwise render as an
                // error with no content at all.
                write!(
                    f,
                    "The configured terminal exited immediately ({status}) without opening a window"
                )?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
        }
    }
}

/// A validated launcher template.
///
/// Parsed once so the failure is reported when the user SAVES a broken
/// template rather than when they later press a button and nothing
/// happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The program, argv[0].
    program: String,
    /// The remaining argv, each either a literal or the placeholder.
    args: Vec<Arg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Arg {
    Literal(String),
    /// The slot the built command goes into, with whatever literal text
    /// surrounds it in the same word (`-e{command}` is one argv entry).
    Command {
        prefix: String,
        suffix: String,
    },
}

impl Template {
    /// Parse and validate, or say what is wrong with it.
    pub fn parse(raw: &str) -> Result<Self, LaunchError> {
        let words = split_words(raw).map_err(|why| LaunchError::BadTemplate { why })?;
        let mut words = words.into_iter();
        let program = words.next().ok_or_else(|| LaunchError::BadTemplate {
            why: "it is empty".to_string(),
        })?;
        if program.contains(PLACEHOLDER) {
            // The program is what gets EXECUTED. Substituting a shell
            // line there runs the command string as a program name,
            // which fails confusingly, and with a template like
            // `{command}` alone would be an attempt to exec the whole
            // `cd ... && claude ...` line as one binary.
            return Err(LaunchError::BadTemplate {
                why: format!("{PLACEHOLDER} cannot be the program name"),
            });
        }

        let mut args = Vec::new();
        let mut seen = false;
        for w in words {
            match w.split_once(PLACEHOLDER) {
                Some((prefix, suffix)) => {
                    if suffix.contains(PLACEHOLDER) || seen {
                        return Err(LaunchError::BadTemplate {
                            why: format!("{PLACEHOLDER} appears more than once"),
                        });
                    }
                    seen = true;
                    args.push(Arg::Command {
                        prefix: prefix.to_string(),
                        suffix: suffix.to_string(),
                    });
                }
                None => args.push(Arg::Literal(w)),
            }
        }
        if !seen {
            // Without the placeholder the terminal would open on
            // nothing -- which looks like it worked, and is the worst
            // outcome available: the user believes Claude is starting.
            return Err(LaunchError::BadTemplate {
                why: format!("it does not contain {PLACEHOLDER}"),
            });
        }
        Ok(Self { program, args })
    }

    /// The argv this template produces for `command`.
    ///
    /// Separated from spawning so the exact argv can be asserted in a
    /// test without running anything -- the property that matters here
    /// is what lands in which slot, and a test that had to spawn a real
    /// terminal to check it would not run in CI.
    pub fn render(&self, command: &str) -> (String, Vec<String>) {
        let argv = self
            .args
            .iter()
            .map(|a| match a {
                Arg::Literal(s) => s.clone(),
                // The command goes in WHOLE, never re-split. This is the
                // line that keeps `resume_command`'s quoting intact.
                Arg::Command { prefix, suffix } => format!("{prefix}{command}{suffix}"),
            })
            .collect();
        (self.program.clone(), argv)
    }
}

/// Split a template into words, honouring quotes.
///
/// A terminal template needs quoting for the same reason a shell line
/// does -- `/Applications/My Terminal.app/…` has a space in it -- but it
/// is NOT a shell: no variable expansion, no substitution, no operators.
/// Only the two quote forms and a backslash escape, which is the
/// smallest grammar that can express a path with a space.
fn split_words(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Escapes the next character anywhere, including inside
                // quotes, so a literal quote is expressible at all.
                match chars.next() {
                    Some(n) => {
                        cur.push(n);
                        has = true;
                    }
                    None => return Err("it ends with a trailing backslash".to_string()),
                }
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                // An empty quoted string is still a word: `foo "" bar`
                // is three arguments, and dropping the middle one would
                // shift every later argument left.
                has = true;
            }
            c if Some(c) == quote => quote = None,
            c if quote.is_none() && c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if quote.is_some() {
        return Err("it has an unclosed quote".to_string());
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// Launch `command` in the configured terminal.
///
/// `cwd` is the directory the command will `cd` into, checked before
/// anything is spawned -- see [`LaunchError::CwdMissing`]. `None` means
/// the command carries no `cd` (an unanchored resume), which is not an
/// error: the caveat the UI already shows covers it.
///
/// Returns once the terminal has been STARTED, not once it exits. The
/// child is deliberately not waited on: a terminal lives for as long as
/// the user keeps it open, and waiting would hang the command for
/// minutes or hours.
/// The shell line that starts `claude` on a prompt, in a directory.
///
/// # Why this is built in Rust, next to `resume_command`
///
/// #1292 Claudifies an advice finding: the payload is `Finding.brief`,
/// which `Finding::new` renders at construction precisely so "a `Finding`
/// built by hand could carry a `brief` that names a different subject
/// than its `subject` field". Building the command line here keeps that
/// guarantee end to end -- for that path the frontend passes a repository
/// path and an index, never a command and never a prompt, so there is no
/// point at which TypeScript could compose a line the backend then runs.
///
/// That is the same rule `claude_launch_worktree` states for itself:
/// accepting a command string from the caller would make this "run
/// whatever you are given in a terminal", which is a different and much
/// larger capability. **No caller ever supplies a command string.**
///
/// # The one path that accepts a PROMPT: a pull request's Claudify (#1455)
///
/// `commands::claudify_pr_command`, `claude_launch_pr` and
/// `claude_launch_pr_preview` take the prompt as text, composed by
/// `src/lib/agentPrompt.ts` from the pull request fetch -- which the
/// backend does not hold, so it cannot rebuild the prompt itself the way
/// it looks up a brief. That is a weaker guarantee than #1292's, and it
/// is bounded by these limits, each enforced in Rust:
///
/// - The prompt occupies ONE single-quoted argv slot of `claude`, built
///   here by `prompt_command`; it never becomes shell.
/// - The directory is not trusted: `commands::pr_checkout` re-validates
///   it against the LIVE worktree scan (scanned, `origin` identity equal
///   to the pull request's repository, not bare).
/// - The launch and its preview are `Class::Local`, so no remote caller
///   -- a paired phone included -- can launch; only the desktop webview
///   can. The phone-reachable `claudify_pr_command` returns a string and
///   runs nothing.
/// - An empty prompt, or one with a NUL byte, is refused.
/// - The whole argv is previewed in the terms dialog before it runs
///   (#1214).
///
/// # How a multi-line Markdown prompt reaches `claude`, verified
///
/// The open question in #1292 was whether it can at all: `claude
/// --resume <id>` is the only line this repository had ever built, and a
/// brief is multi-line Markdown thick with backticks, `$(...)` and
/// apostrophes quoted out of the user's own files.
///
/// It can, and nothing has to be mangled to make it fit. `claude`'s
/// usage line is `claude [options] [command] [prompt]` -- the prompt is
/// a POSITIONAL ARGUMENT, so it is one argv slot and newlines in it are
/// just bytes. There is no length limit short enough to matter and no
/// escaping for Markdown to trip over.
///
/// Three layers each have to keep it in one piece, and each does:
///
/// 1. [`super::sessions::shell_quote`] wraps the brief in single quotes,
///    where every character except `'` is literal. A brief containing
///    `$(whoami)` is text, not a substitution -- the same property
///    `resume_command` relies on for the session id.
/// 2. The shell the template names (`bash -lc`, and the `-e`/`--` forms)
///    parses that quoted word back into exactly one argument, newlines
///    included.
/// 3. [`Template::render`] puts the whole command string into ONE argv
///    slot and never re-splits it, which is the invariant this module's
///    header calls "the line that keeps `resume_command`'s quoting
///    intact".
///
/// So the prompt `claude` receives is byte-for-byte the brief the
/// clipboard would have carried. That is the bar: Copy and Run must hand
/// over the same text, or the button that runs it is lying about what it
/// runs.
///
/// # The `open -a` presets that could not run anything (#1302)
///
/// This paragraph used to say that `open -a Terminal {command}` does not
/// run a command at all -- `open -a` takes FILE PATHS, so it hands
/// Terminal.app a filename -- and that repairing it was not this
/// function's job. The same defect was in the `iTerm` preset, uncovered
/// by that note, and it is what #1302 was reported against: the user
/// pressed Run and saw nothing at all.
///
/// Both presets now drive the app with `osascript` instead, and
/// [`watch_briefly`] makes an instant failure an error the user can
/// read rather than a silent success. Writing the defect down was not
/// enough to keep it from shipping twice, so the note has been replaced
/// by the fix.
///
/// The `cd` is always present and always quoted: a prompt is worthless
/// in the wrong repository, and unlike a resume there is no "wherever
/// you run it" fallback that would still be correct.
pub fn prompt_command(prompt: &str, cwd: &str) -> String {
    use super::sessions::shell_quote;
    format!("cd {} && claude {}", shell_quote(cwd), shell_quote(prompt))
}

pub fn launch(template: &str, command: &str, cwd: Option<&str>) -> Result<(), LaunchError> {
    if template.trim().is_empty() {
        return Err(LaunchError::NotConfigured);
    }
    let parsed = Template::parse(template)?;

    if let Some(dir) = cwd {
        // `is_dir` and not `exists`: a path that became a FILE is not
        // somewhere a `cd` can land either, and reporting "gone" for it
        // is closer to true than launching into a failure.
        if !Path::new(dir).is_dir() {
            return Err(LaunchError::CwdMissing {
                path: dir.to_string(),
            });
        }
    }

    let (program, args) = parsed.render(command);
    let mut child = Command::new(&program)
        .args(&args)
        // Captured so an instant failure has something to SAY. `open`'s
        // "The file … does not exist" goes here, and inheriting the
        // stream would send it to a stdout nobody is reading -- this
        // app has no console -- which is how the failure stayed
        // invisible.
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| LaunchError::Spawn {
            why: format!("{program}: {e}"),
        })?;

    watch_briefly(&mut child)
}

/// How long to give a launcher to fail before calling it launched.
///
/// The number is a judgment, so it is written down rather than inlined:
/// long enough that a program which is going to fail instantly has
/// finished doing so (`open` reports a bad path in single-digit
/// milliseconds), short enough that a user who pressed a button does not
/// notice the wait.
///
/// It is NOT a timeout on the terminal. See [`watch_briefly`].
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// Report a child that has ALREADY failed, without waiting on one that
/// has not (#1302).
///
/// # Why this cannot hang on a real terminal
///
/// This is the property the module header calls load-bearing: a terminal
/// lives for as long as the user keeps it open, so waiting for it would
/// block the command for hours.
///
/// It is preserved by polling rather than waiting. `try_wait` never
/// blocks -- it asks whether the child has exited and returns
/// immediately either way. The loop runs for at most [`SETTLE`] and then
/// RETURNS OK regardless of what the child is doing. A terminal that is
/// still running after 400ms is the expected case, and it is left alone:
/// nothing here ever calls `wait`, and no code path waits on the child
/// for longer than [`SETTLE`], whatever the child does.
///
/// The child is not killed or reaped on the success path either. It is
/// dropped still running, which is exactly what the previous `spawn()`
/// did.
///
/// # Why exit status zero is a success
///
/// A launcher that exits 0 has handed off to a terminal that is now its
/// own process -- `osascript` driving iTerm does precisely this, and
/// returns as soon as the window exists. Treating a quick clean exit as
/// failure would break every correct preset.
fn watch_briefly(child: &mut std::process::Child) -> Result<(), LaunchError> {
    let deadline = std::time::Instant::now() + SETTLE;
    loop {
        match child.try_wait() {
            // Still running: this is a terminal doing its job.
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(());
                }
                // Short enough to stay responsive, long enough not to
                // spin. The exact figure does not matter; that the loop
                // is bounded by `deadline` does.
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    let mut buf = Vec::new();
                    // Best effort: the status alone is still a usable
                    // error, so a failure to read the pipe must not
                    // replace "it exited 1" with "could not read a pipe".
                    let _ = pipe.read_to_end(&mut buf);
                    stderr = String::from_utf8_lossy(&buf).trim().to_string();
                }
                return Err(LaunchError::ExitedImmediately {
                    status: describe(&status),
                    stderr,
                });
            }
            // `try_wait` itself failed. Nothing is known about the
            // child, and inventing a failure would be as wrong as
            // inventing a success -- but the child WAS spawned, so the
            // launch is reported as done rather than as an error the
            // user cannot act on.
            Err(_) => return Ok(()),
        }
    }
}

/// An exit status in words the error message can carry.
fn describe(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(c) => format!("exit status {c}"),
        // Killed by a signal: `code()` is None and the Display impl is
        // the only thing that says which.
        None => status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative template for each terminal the issue names, so
    /// the parser is exercised against real shapes rather than shapes
    /// invented to suit it.
    const REAL: &[(&str, &str)] = &[
        (
            "Terminal via osascript",
            "/usr/bin/osascript -e 'on run argv' -e 'tell application \"Terminal\"' -e 'do script (item 1 of argv)' -e 'activate' -e 'end tell' -e 'end run' {command}",
        ),
        (
            "iTerm via osascript",
            "/usr/bin/osascript -e 'on run argv' -e 'tell application \"iTerm\"' -e 'activate' -e 'tell (create window with default profile) to tell current session to write text (item 1 of argv)' -e 'end tell' -e 'end run' {command}",
        ),
        ("gnome", "gnome-terminal -- bash -c {command}"),
        ("konsole", "konsole -e {command}"),
        ("wezterm", "wezterm start -- bash -lc {command}"),
        (
            "quoted path",
            "\"/Applications/My Terminal.app/x\" -e {command}",
        ),
    ];

    /// The presets the settings panel offers, read from the TypeScript
    /// that offers them.
    ///
    /// Read rather than copied. A preset the panel offers and Rust
    /// refuses is a button that saves a template which then fails at
    /// launch -- and a second copy of the list here would pass this
    /// test while the real one drifted.
    #[test]
    fn every_preset_the_settings_panel_offers_is_one_this_parser_accepts() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("src")
                .join("lib")
                .join("terminalTemplate.ts"),
        )
        .expect("terminalTemplate.ts is where the panel's presets live");

        // `template: \`...\`` -- a backtick literal ending in the
        // placeholder interpolation, which is how each preset is
        // spelled.
        let mut found = 0usize;
        for line in src.lines() {
            let Some((_, rest)) = line.split_once("template: `") else {
                continue;
            };
            let Some((tpl, _)) = rest.split_once('`') else {
                continue;
            };
            // The TS writes the placeholder as `${PLACEHOLDER}`.
            let tpl = tpl.replace("${PLACEHOLDER}", PLACEHOLDER);
            Template::parse(&tpl).unwrap_or_else(|e| panic!("preset {tpl:?}: {e}"));
            found += 1;
        }
        // Without this the test passes vacuously the moment the preset
        // spelling changes and the scan finds nothing.
        assert!(
            found >= 5,
            "expected to find the panel's presets, found {found} -- has the spelling changed?"
        );
    }

    #[test]
    fn every_real_template_parses_and_puts_the_command_in_exactly_one_slot() {
        for (name, raw) in REAL {
            let t = Template::parse(raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            let (_prog, argv) = t.render("cd 'x' && claude");
            let hits = argv.iter().filter(|a| a.contains("claude")).count();
            assert_eq!(
                hits, 1,
                "{name}: command landed in {hits} argv slots: {argv:?}"
            );
        }
    }

    #[test]
    fn a_path_with_a_space_stays_one_word() {
        // The reason `split_words` honours quotes at all.
        let t = Template::parse("\"/Applications/My Terminal.app/x\" -e {command}").unwrap();
        let (prog, argv) = t.render("c");
        assert_eq!(prog, "/Applications/My Terminal.app/x");
        assert_eq!(argv, vec!["-e".to_string(), "c".to_string()]);
    }

    #[test]
    fn the_command_is_never_split_however_it_is_spelled() {
        // THE property of this module. `resume_command` shell-quotes the
        // path and the session id so a directory named `$(whoami)`
        // cannot inject; that only holds if the quoted string stays in
        // one argv slot, byte for byte.
        let nasty = "cd '/tmp/a b; rm -rf ~' && claude --resume '$(whoami) `id`'";
        let t = Template::parse("open -a Terminal {command}").unwrap();
        let (_p, argv) = t.render(nasty);
        assert_eq!(
            argv,
            vec!["-a".to_string(), "Terminal".to_string(), nasty.to_string()]
        );
    }

    #[test]
    fn the_command_keeps_the_literal_text_around_it_in_its_own_word() {
        // `-e{command}` is one argv entry, not two.
        let t = Template::parse("term -e{command};exit").unwrap();
        let (_p, argv) = t.render("C");
        assert_eq!(argv, vec!["-eC;exit".to_string()]);
    }

    #[test]
    fn a_template_without_the_placeholder_is_refused() {
        // Would open a terminal on nothing, which LOOKS like it worked.
        let e = Template::parse("open -a Terminal").unwrap_err();
        assert!(matches!(e, LaunchError::BadTemplate { .. }), "{e:?}");
        assert!(e.to_string().contains("{command}"), "{e}");
    }

    #[test]
    fn the_placeholder_cannot_be_the_program() {
        let e = Template::parse("{command}").unwrap_err();
        assert!(matches!(e, LaunchError::BadTemplate { .. }), "{e:?}");
    }

    #[test]
    fn a_repeated_placeholder_is_refused() {
        // Ambiguous rather than harmless: running the command twice is
        // two Claude sessions, and there is no reading of the template
        // that makes that the user's intent.
        for raw in ["t {command} {command}", "t {command}{command}"] {
            let e = Template::parse(raw).unwrap_err();
            assert!(matches!(e, LaunchError::BadTemplate { .. }), "{raw}: {e:?}");
        }
    }

    #[test]
    fn an_unclosed_quote_is_refused_rather_than_guessed() {
        let e = Template::parse("term -e \"{command}").unwrap_err();
        assert!(e.to_string().contains("unclosed"), "{e}");
    }

    #[test]
    fn an_empty_template_is_not_configured_rather_than_a_parse_error() {
        // Different remedies: "set one up" versus "fix the one you set".
        for raw in ["", "   ", "\t\n"] {
            assert_eq!(
                launch(raw, "c", None).unwrap_err(),
                LaunchError::NotConfigured
            );
        }
    }

    #[test]
    fn a_missing_cwd_refuses_before_spawning() {
        // A terminal that opens and immediately fails its own `cd`
        // leaves the user in an unrelated directory with a Claude
        // session about to start there.
        let e = launch(
            "open -a Terminal {command}",
            "cd x && claude",
            Some("/no/such/dir"),
        )
        .unwrap_err();
        assert!(matches!(e, LaunchError::CwdMissing { .. }), "{e:?}");
    }

    #[test]
    fn a_cwd_that_is_a_file_is_reported_as_missing() {
        // `is_dir`, not `exists`: a path that became a file is not
        // somewhere a `cd` can land either.
        let f = std::env::temp_dir().join("headstate-launch-cwd-test");
        std::fs::write(&f, b"x").unwrap();
        let e = launch(
            "open -a Terminal {command}",
            "c",
            Some(&f.to_string_lossy()),
        )
        .unwrap_err();
        let _ = std::fs::remove_file(&f);
        assert!(matches!(e, LaunchError::CwdMissing { .. }), "{e:?}");
    }

    #[test]
    fn a_program_that_does_not_exist_reports_spawn_and_names_it() {
        // Reached only with a real cwd, so this also proves the cwd
        // check passes a directory that IS there.
        let dir = std::env::temp_dir();
        let e = launch(
            "headstate-no-such-terminal-xyz {command}",
            "c",
            Some(&dir.to_string_lossy()),
        )
        .unwrap_err();
        match &e {
            LaunchError::Spawn { why } => {
                assert!(why.contains("headstate-no-such-terminal-xyz"), "{why}")
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
    }

    #[test]
    fn every_error_says_something_different() {
        // The enum exists because each arm is a different REMEDY. Four
        // arms rendering the same sentence would be a `String`.
        let msgs = [
            LaunchError::NotConfigured.to_string(),
            LaunchError::BadTemplate { why: "w".into() }.to_string(),
            LaunchError::CwdMissing { path: "p".into() }.to_string(),
            LaunchError::Spawn { why: "w".into() }.to_string(),
        ];
        let mut uniq = msgs.to_vec();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), msgs.len(), "two errors read the same: {msgs:?}");
        // And each names its own remedy rather than a generic failure.
        assert!(msgs[0].contains("Settings"), "{}", msgs[0]);
    }

    /// The property the whole Claudify feature rests on (#1292): a
    /// multi-line Markdown brief reaches `claude` as ONE argument,
    /// byte-for-byte.
    ///
    /// This is the question the issue asked and could not answer from
    /// the code: every call site in this repository was `claude
    /// --resume <id>`, a single short line, and nothing here had ever
    /// passed a PROMPT. The answer is that `claude`'s prompt is a
    /// positional argument, so newlines in it are just bytes -- and
    /// this test is what keeps that true.
    ///
    /// The brief below is deliberately hostile in every way a real one
    /// can be: it spans lines, it holds backticks and a `$(...)` that a
    /// shell would substitute, and it holds an apostrophe, which is the
    /// ONE character single quotes cannot pass through unescaped.
    #[test]
    fn a_multi_line_brief_survives_as_exactly_one_argument() {
        let brief =
            "## It names `src-tauri/` paths\n\nEvidence: `a.md:38` — $(whoami)\nDon't guess.\n";
        // A REAL directory, because the `&&` below short-circuits on a
        // failed `cd` -- which would leave the shell check asserting
        // nothing at all while still passing.
        let dir = std::env::temp_dir();
        let command = prompt_command(brief, &dir.to_string_lossy());

        // Through the template, into argv. `bash -lc` is the shape three
        // of the five presets use.
        let t = Template::parse("gnome-terminal -- bash -lc {command}").unwrap();
        let (_p, argv) = t.render(&command);

        // The command string is ONE argv slot, never re-split -- the
        // invariant this module's header states. Counted rather than
        // indexed at a fixed position: the template decides how many
        // literal words precede it, and hardcoding that number tests
        // the template rather than the property.
        assert_eq!(
            argv.iter().filter(|a| a.contains("claude")).count(),
            1,
            "{argv:?}"
        );
        assert_eq!(argv.last().unwrap(), &command);

        // And that slot, parsed back by the shell it is handed to, is a
        // `cd` plus `claude` plus the brief verbatim. Asserted by
        // running the real shell rather than by re-implementing its
        // quoting rules, which would only prove this test agrees with
        // itself.
        let out = Command::new("sh")
            .arg("-c")
            .arg(command.replacen("claude", "printf %s", 1))
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            brief,
            "the brief did not survive quoting"
        );
    }

    /// A brief cannot break out of its quoting and become a command.
    ///
    /// The round-trip test above proves the brief ARRIVES intact; this
    /// one proves the stronger property that nothing in it can ESCAPE.
    /// They are different failures: a mangled prompt is a bug, and a
    /// prompt that runs `touch` is a vulnerability.
    ///
    /// Asserted two ways, because each catches what the other misses.
    /// The structural check would pass for a quoter that escaped nothing
    /// but happened to produce a balanced string; the executed check
    /// would pass for a payload that simply failed to trigger. A
    /// canary file that is never created is the only direct evidence
    /// that the breakout did not run.
    #[test]
    fn nothing_in_a_brief_can_escape_its_quoting() {
        let dir = std::env::temp_dir();
        let canary = dir.join("headstate-1292-canary");
        let _ = std::fs::remove_file(&canary);

        // Every shape a Markdown brief can legitimately contain, plus a
        // deliberate quote-breakout attempt.
        let payloads = [
            "`backtick`",
            "$(touch canary)",
            "a'; touch canary; echo 'b",
            "line one\nline two",
            "$(whoami) && `id` ; rm -rf /",
            "it's got an apostrophe",
        ];

        for p in payloads {
            let brief = p.replace("canary", &canary.to_string_lossy());
            let command = prompt_command(&brief, &dir.to_string_lossy());

            // Structural: the prompt occupies exactly one single-quoted
            // region, so the only `'` characters in it are the ones
            // `shell_quote`'s `'\''` escape put there.
            let prefix = format!("cd '{}' && claude '", dir.to_string_lossy());
            assert!(command.starts_with(&prefix), "{command}");
            assert!(command.ends_with('\''), "{command}");

            // Executed: run the real thing with `claude` stubbed out,
            // and require the payload back verbatim on stdout.
            let out = Command::new("sh")
                .arg("-c")
                .arg(command.replacen("claude", "printf %s", 1))
                .output()
                .expect("sh");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                brief,
                "payload did not arrive verbatim: {p:?}"
            );
            assert!(
                !canary.exists(),
                "a brief escaped its quoting and ran a command: {p:?}"
            );
        }
        let _ = std::fs::remove_file(&canary);
    }

    /// The `cd` is quoted, so a repository path cannot inject.
    ///
    /// `resume_command` has this property and says why; a prompt command
    /// must not be the place it is lost.
    #[test]
    fn the_working_directory_is_quoted() {
        let c = prompt_command("hi", "/tmp/$(touch pwned)");
        assert!(c.starts_with("cd '/tmp/$(touch pwned)' && claude "), "{c}");
    }

    #[test]
    fn an_empty_quoted_word_is_still_a_word() {
        // Dropping it would shift every later argument one slot left,
        // which silently changes what the terminal is told to do.
        let t = Template::parse("t \"\" {command}").unwrap();
        let (_p, argv) = t.render("C");
        assert_eq!(argv, vec![String::new(), "C".to_string()]);
    }

    /// A launcher that fails instantly is an ERROR, not a success
    /// (#1302).
    ///
    /// The bug this is the regression test for: `open -a iTerm 'cd … &&
    /// claude'` exits 1 in milliseconds having opened nothing, and
    /// `spawn().map(|_| ())` called that a successful launch. The user
    /// pressed Run and could not tell whether anything had happened.
    ///
    /// A template that runs the COMMAND in a shell, on either platform.
    ///
    /// The four tests below need a child with a chosen exit status and a
    /// chosen lifetime. They get one by putting the payload where a real
    /// launch puts it -- in the `{command}` slot -- so the template keeps
    /// the shape a preset has (`<shell> <flag> {command}`, which three of
    /// the five presets use) and the payload is one argv element.
    ///
    /// `sh` is spelled WITHOUT a path and is not gated behind
    /// `#[cfg(unix)]`. The first CI run of this PR failed on Windows
    /// because these tests named `/usr/bin/false` and `/bin/sh`, which
    /// `windows-latest` does not have:
    ///
    /// ```text
    /// expected ExitedImmediately, got Spawn { why: "/usr/bin/false:
    ///   The system cannot find the path specified. (os error 3)" }
    /// ```
    ///
    /// A bare `sh` does resolve there -- `windows-latest` ships Git for
    /// Windows on PATH -- which is not an assumption but an observation:
    /// `a_multi_line_brief_survives_as_exactly_one_argument` and
    /// `nothing_in_a_brief_can_escape_its_quoting` already spawn
    /// `Command::new("sh")` and both PASSED on the Windows leg of that
    /// same run.
    ///
    /// Keeping these on Windows rather than gating them matters:
    /// `watch_briefly` is the launcher's only guarantee and is not
    /// platform-specific, so skipping it there would leave a shipped
    /// platform unverified.
    const SHELL: &str = "sh -c {command}";

    /// Exit at once with a chosen non-zero status.
    const FAILS_FAST: &str = "exit 1";

    /// Write a known marker to stderr, then exit 3.
    const TALKS_THEN_FAILS: &str = "echo headstate-1302-marker >&2; exit 3";

    /// Exit 0 at once, the way a launcher that has handed off does.
    const HANDS_OFF: &str = "exit 0";

    /// Stay running, the way a terminal the user keeps open does. 60s
    /// stands in for "as long as the user keeps it open": long enough
    /// that waiting on it is unmistakable in the timing.
    const STAYS_OPEN: &str = "sleep 60";

    #[test]
    fn a_program_that_exits_non_zero_at_once_is_reported_not_called_success() {
        let dir = std::env::temp_dir();
        let e = launch(SHELL, FAILS_FAST, Some(&dir.to_string_lossy())).unwrap_err();
        match &e {
            LaunchError::ExitedImmediately { status, .. } => {
                assert!(status.contains('1'), "expected exit 1, got {status:?}")
            }
            other => panic!("expected ExitedImmediately, got {other:?}"),
        }
        // And it reads as something a user can act on.
        assert!(e.to_string().contains("exited immediately"), "{e}");
    }

    /// Whatever the failing program said is carried to the user.
    ///
    /// The status alone would have told the #1302 reporter only that
    /// something exited 1. `open`'s "The file … does not exist" is the
    /// line that actually explains it, and it was being written to a
    /// stderr nobody read.
    #[test]
    fn the_stderr_of_an_instant_failure_reaches_the_message() {
        let dir = std::env::temp_dir();
        let e = launch(SHELL, TALKS_THEN_FAILS, Some(&dir.to_string_lossy())).unwrap_err();
        match &e {
            LaunchError::ExitedImmediately { stderr, status } => {
                assert!(stderr.contains("headstate-1302-marker"), "{stderr:?}");
                assert!(status.contains('3'), "{status:?}");
            }
            other => panic!("expected ExitedImmediately, got {other:?}"),
        }
        assert!(e.to_string().contains("headstate-1302-marker"), "{e}");
    }

    /// A launcher that exits 0 immediately is a SUCCESS.
    ///
    /// `osascript` driving iTerm does exactly this: it returns as soon
    /// as the window exists, long before the user closes it. Treating a
    /// quick clean exit as failure would break every working preset,
    /// which is why the check is on the status and not on the speed.
    #[test]
    fn a_launcher_that_hands_off_and_exits_zero_is_a_success() {
        let dir = std::env::temp_dir();
        launch(SHELL, HANDS_OFF, Some(&dir.to_string_lossy()))
            .expect("a clean instant exit is a handoff, not a failure");
    }

    /// The property the module header calls load-bearing: a real
    /// terminal, which stays open for hours, must not be waited on.
    ///
    /// Asserted on the CLOCK rather than on the code, because "it
    /// returns" is not the claim -- the claim is that it returns
    /// PROMPTLY while the child is still running. A `wait()` that
    /// slipped back in would hang here for 60 seconds and fail on the
    /// elapsed-time assertion, which is the regression that matters.
    #[test]
    fn a_terminal_that_stays_open_is_not_waited_on() {
        let dir = std::env::temp_dir();
        let start = std::time::Instant::now();
        launch(SHELL, STAYS_OPEN, Some(&dir.to_string_lossy()))
            .expect("a still-running terminal is a successful launch");
        let elapsed = start.elapsed();
        assert!(
            elapsed < SETTLE * 4,
            "launch waited {elapsed:?} on a child that stays open -- it must return after ~{SETTLE:?}"
        );
    }

    /// Read the macOS presets out of the file the settings panel offers
    /// them from.
    ///
    /// Shared by the two tests below so there is exactly one place that
    /// knows how a preset is spelled. A copy in each would let one drift
    /// while the other kept passing, which is the failure mode that put
    /// the broken `open -a iTerm` preset in a shipped release.
    ///
    /// Returns `(app_name, template)` for every preset naming a macOS
    /// terminal, with `${PLACEHOLDER}` already interpolated.
    ///
    /// The app is matched on its name ANYWHERE in the template, not on
    /// the quoted `"iTerm"` that an AppleScript `tell` happens to use.
    /// Keying on the quotes was itself a version of the bug under test:
    /// restoring the broken `open -a iTerm {command}` preset left the
    /// name unquoted, so it matched nothing, was skipped, and the test
    /// went green on exactly the template #1302 was filed about.
    fn macos_presets() -> Vec<(&'static str, String)> {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("src")
                .join("lib")
                .join("terminalTemplate.ts"),
        )
        .expect("terminalTemplate.ts is where the panel's presets live");

        let mut out = Vec::new();
        for line in src.lines() {
            let Some((_, rest)) = line.split_once("template: `") else {
                continue;
            };
            let Some((tpl, _)) = rest.split_once('`') else {
                continue;
            };
            let tpl = tpl.replace("${PLACEHOLDER}", PLACEHOLDER);
            // `iTerm` first: "iTerm" does not contain "Terminal", but a
            // future iTerm template that mentioned both would otherwise
            // be filed under the wrong app.
            let app = if tpl.contains("iTerm") {
                "iTerm"
            } else if tpl.contains("Terminal") {
                "Terminal"
            } else {
                continue;
            };
            out.push((app, tpl));
        }
        out
    }

    /// Both macOS presets are an `osascript` invocation whose AppleScript
    /// body receives the command as `item 1 of argv` -- proven by
    /// running the real thing (#1309).
    ///
    /// # What this is, and what it is not
    ///
    /// This is the automatable half of the canary test below. That one
    /// drives Terminal.app and iTerm for real, which is the strongest
    /// possible evidence and also why it cannot run in CI: it needs a
    /// logged-in windowing session and a TCC Automation grant, and it
    /// was measured failing one run in three as windows accumulate.
    /// Left as the only coverage, the presets were verified by a manual
    /// run recorded in a PR body and by nothing a future edit would
    /// have to satisfy.
    ///
    /// So this asserts one layer down: that `osascript` PARSES AND
    /// DISPATCHES the script the preset generates, with the command
    /// arriving as a positional `argv` item the script can run. Only
    /// the GUI target is swapped out -- the `tell application …` block
    /// becomes `do shell script`, which needs no app, no window and no
    /// automation grant. The program, the `on run argv` / `end run`
    /// wrapper, the `-e` argument structure and the `{command}` slot
    /// are the shipped preset's own, and the whole thing goes through
    /// the real [`launch`].
    ///
    /// It is strictly weaker than driving the app: a preset that opens
    /// a window but writes the command into the wrong session would
    /// pass here. It is also enough to have caught the actual shipped
    /// bug, which is the point -- `open -a iTerm {command}` has no
    /// AppleScript body to retarget at all, so it cannot reach this
    /// test's assertion, and the refusal below is what fails.
    ///
    /// # Why a preset it does not understand is a FAILURE, not a skip
    ///
    /// The first version of the canary test skipped a template it could
    /// not match and passed with nothing checked. Here an unrecognised
    /// preset panics by name. A preset this test stops understanding is
    /// a preset nothing is verifying, and that has to be loud: it is
    /// precisely what a regression to `open -a` looks like from in
    /// here.
    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "osascript is macOS-only; the presets it checks are too"
    )]
    fn the_macos_presets_hand_the_command_to_a_script_osascript_can_run() {
        let presets = macos_presets();
        // Without this the test passes vacuously the moment the preset
        // spelling changes and the scan finds nothing.
        assert_eq!(
            presets.len(),
            2,
            "expected the two macOS presets, found {presets:?}"
        );

        for (app, tpl) in presets {
            // Retarget the AppleScript, and NOTHING else.
            //
            // A preset is `/usr/bin/osascript -e 'on run argv' <body…>
            // -e 'end run' {command}`. The body is the part that names
            // the GUI app; everything around it is the structure under
            // test. Rebuilt by keeping the head and the tail of the
            // real template verbatim and substituting one `-e`.
            let head = "-e 'on run argv' ";
            let tail = " -e 'end run' {command}";
            let (before, _after) = tpl.split_once(head).unwrap_or_else(|| {
                panic!(
                    "{app} preset is not an `osascript … on run argv` invocation, \
                     so this test cannot check that osascript runs it. That is what \
                     the #1302 `open -a {app} {{command}}` preset looks like from \
                     here, and it is a failure, not something to skip: {tpl}"
                )
            });
            assert!(
                tpl.ends_with(tail),
                "{app} preset does not end in `{tail}` -- the command must reach the \
                 script as a positional argv item, not be pasted into it: {tpl}"
            );
            let retargeted = format!("{before}{head}-e 'do shell script (item 1 of argv)'{tail}");

            let canary = std::env::temp_dir().join(format!("headstate-1309-dispatch-{app}"));
            let _ = std::fs::remove_file(&canary);
            let command = format!(
                "touch {}",
                super::super::sessions::shell_quote(&canary.to_string_lossy())
            );

            // Through the real launcher, so `watch_briefly` gets a say:
            // `open -a` exits 1 in milliseconds and this is now an
            // `ExitedImmediately` error rather than a silent success.
            launch(
                &retargeted,
                &command,
                Some(&std::env::temp_dir().to_string_lossy()),
            )
            .unwrap_or_else(|e| panic!("{app} preset shape failed to launch: {e}\n{retargeted}"));

            // `do shell script` is synchronous inside `osascript`, but
            // `launch` deliberately does not wait on its child, so the
            // file can appear just after it returns. Polled to a
            // deadline rather than slept on a fixed guess.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !canary.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                canary.exists(),
                "{app} preset's script shape did not run the command -- osascript \
                 accepted it but the command never reached a shell: {retargeted}"
            );
            let _ = std::fs::remove_file(&canary);
        }
    }

    /// The presets the panel offers actually RUN A COMMAND on this
    /// machine (#1302).
    ///
    /// This is the test the issue asked for by name, and the only kind
    /// that would have caught the bug. `open -a iTerm {command}` was a
    /// plausible string that parsed, rendered into one argv slot, and
    /// passed every assertion in this file -- while running nothing at
    /// all. A string-level test cannot tell the difference; a canary
    /// file can.
    ///
    /// # Why it is `#[ignore]`d rather than run by default
    ///
    /// It drives real GUI applications through AppleScript, which needs
    /// macOS, the app installed, a logged-in windowing session, and a
    /// TCC Automation grant that cannot be given non-interactively.
    /// None of that holds in CI.
    ///
    /// It is also genuinely flaky when run repeatedly: observed passing
    /// twice and failing on a third consecutive `cargo test` run on the
    /// development machine, with `osascript` still exiting 0. Each run
    /// leaves its windows open, and the AppleEvent gets slower as they
    /// accumulate. That is a property of driving a real GUI app, not of
    /// the templates -- run by hand against a fresh Terminal the same
    /// invocation succeeds every time -- but a test that fails one run
    /// in three would be worse than useless in the `Race check` job,
    /// which runs the suite three times precisely to catch flakes.
    ///
    /// So it is opt-in, the way `transcripts::real_corpus` is for the
    /// same reason: run deliberately by a developer on a Mac, with the
    /// result recorded in the PR. That is where the claim "I observed
    /// this preset work" has to be made, and it cannot be made by a
    /// string comparison.
    ///
    ///     cargo test --manifest-path src-tauri/Cargo.toml \
    ///         the_macos_presets -- --ignored --nocapture
    ///
    /// # What covers the presets when this does not run (#1309)
    ///
    /// Being the only check meant the presets were verified by a manual
    /// run recorded in a PR body and by nothing a future edit had to
    /// satisfy. [`the_macos_presets_hand_the_command_to_a_script_osascript_can_run`]
    /// now runs in CI and asserts the layer beneath this one: that
    /// `osascript` parses and dispatches the script each preset builds,
    /// with the command arriving as a positional `argv` item a shell
    /// runs. It retargets the `tell application` block and changes
    /// nothing else, so it needs no app, no window and no TCC grant.
    ///
    /// What it does NOT cover, and what this test is still the only
    /// evidence for: that the AppleScript addressed at Terminal.app and
    /// iTerm opens a window and writes the command into the right
    /// session. A preset that dispatched cleanly into the wrong session
    /// would pass in CI and fail here. That gap is real and is why this
    /// test is still worth running by hand before a release.
    #[test]
    #[ignore = "drives real GUI apps: needs macOS, a windowing session and a TCC grant"]
    fn the_macos_presets_actually_run_a_command_here() {
        // Read from the SAME file the settings panel offers, via the
        // same helper the CI-runnable test uses, so a preset that
        // drifts is a preset BOTH tests stop covering -- rather than a
        // second copy here that passes while the real one is broken.
        // That second-copy failure is exactly how the iTerm preset
        // shipped broken.
        let presets = macos_presets();
        assert_eq!(
            presets.len(),
            2,
            "expected the two macOS presets, found {presets:?}"
        );

        let mut checked = 0usize;
        for (app, tpl) in presets {
            // Only the apps actually present on this machine.
            // `gnome-terminal` and friends are Linux and are already
            // filtered out by `macos_presets`; claiming to have checked
            // them would be the lie this test exists to prevent.
            if !std::path::Path::new(&format!("/Applications/{app}.app")).exists()
                && !std::path::Path::new(&format!("/System/Applications/Utilities/{app}.app"))
                    .exists()
            {
                eprintln!("SKIP {app}: not installed");
                continue;
            }

            let canary = std::env::temp_dir().join(format!("headstate-1302-canary-{app}"));
            let _ = std::fs::remove_file(&canary);
            let command = format!(
                "touch {}",
                super::super::sessions::shell_quote(&canary.to_string_lossy(),)
            );

            launch(
                &tpl,
                &command,
                Some(&std::env::temp_dir().to_string_lossy()),
            )
            .unwrap_or_else(|e| panic!("{app} preset failed to launch: {e}"));

            // The terminal runs the line asynchronously once its window
            // exists, so the canary appears strictly after `launch`
            // returns. Polled to a deadline rather than slept on a
            // fixed guess: a fast machine should not wait, and a slow
            // one should not fail.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !canary.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(
                canary.exists(),
                "{app} preset opened without running the command -- \
                 this is the #1302 bug: {tpl}"
            );
            let _ = std::fs::remove_file(&canary);
            checked += 1;
        }

        // BOTH macOS presets, not merely "at least one". `checked > 0`
        // was the guard here first and it passed while the iTerm preset
        // was being skipped entirely -- the test reported success for a
        // list in which the #1302 template had been restored. A count
        // that cannot tell "all of them worked" from "the one I looked
        // at worked" is the vacuous-pass failure this whole test exists
        // to avoid.
        assert_eq!(
            checked, 2,
            "expected both macOS presets to be exercised, checked {checked} -- \
             a preset that is skipped is a preset nothing is verifying"
        );
    }
}
