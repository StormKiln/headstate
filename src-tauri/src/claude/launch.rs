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
    Command::new(&program)
        .args(&args)
        .spawn()
        .map(|_child| ())
        .map_err(|e| LaunchError::Spawn {
            why: format!("{program}: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative template for each terminal the issue names, so
    /// the parser is exercised against real shapes rather than shapes
    /// invented to suit it.
    const REAL: &[(&str, &str)] = &[
        ("open -a Terminal", "open -a Terminal {command}"),
        ("iterm", "/usr/bin/open -a iTerm {command}"),
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

    #[test]
    fn an_empty_quoted_word_is_still_a_word() {
        // Dropping it would shift every later argument one slot left,
        // which silently changes what the terminal is told to do.
        let t = Template::parse("t \"\" {command}").unwrap();
        let (_p, argv) = t.render("C");
        assert_eq!(argv, vec![String::new(), "C".to_string()]);
    }
}
