//! The terms a session starts on: which model, how much autonomy (#1214).
//!
//! #1126 gave Headstate a spawn path, and the command it spawns is
//! FIXED. The decision a user actually makes when handing work to an
//! agent is which model and how much autonomy -- so this module is the
//! vocabulary of that decision, and nothing else.
//!
//! # Why this is a closed enum and not a string
//!
//! [`super::launch`] states the property this feature is the natural
//! pressure to break: the launch commands take the id and the paths,
//! **never a command string**, because the built line goes into exactly
//! one argv slot and a shell never sees it. The obvious way to add
//! flags is to let the caller pass `--model opus` as text and append it.
//! That would make the launch commands "run whatever flags you are
//! given", which is a different and much larger capability, and it would
//! put caller-controlled text into the line that runs.
//!
//! So the wire carries a TOKEN and Rust carries the flag. [`Model`] and
//! [`PermissionMode`] are closed enums; [`Terms::parse`] maps a token to
//! a variant or REFUSES; and [`Terms::flags`] renders `&'static str`s
//! that were compiled into this binary. There is no path from caller
//! text to argv: the only thing a caller can do is name a variant that
//! already exists, or be told it does not.
//!
//! That is why `flags` returns `&'static str` rather than `String`. A
//! `String` would compile just as well and would leave the door open for
//! a later `format!` that interpolates an argument; the static type
//! closes it, and a test asserts the rendered words are a subset of a
//! constant list.
//!
//! # The vocabulary is not ours and cannot be verified
//!
//! There is no way to ask the installed `claude` binary which values it
//! accepts. Offering `--permission-mode bypassPermissions` to a binary
//! that spells it differently produces a session that fails to start,
//! from a button that promised it would -- worse than not offering the
//! choice at all.
//!
//! So every value here is one this repository already has evidence for,
//! and the evidence is named on the variant. Nothing is added because a
//! doc page mentions it. When the set is wrong it is wrong in the
//! direction of offering too little, which costs the user a flag they
//! can still type themselves.
//!
//! The other half of that caution is [`Terms::preview`]: the clipboard
//! path let the user read the line before running it and a spawn path
//! takes that away. Same reasoning `launch::LaunchError::CwdMissing`
//! already applies -- spawning on someone's behalf has to be stricter,
//! because they are not reading the line before it runs.

use serde::{Deserialize, Serialize};

/// Which model the session starts on.
///
/// # Evidence
///
/// `claude/settings.rs`'s fixtures resolve Claude Code's own `model`
/// setting across scopes with the values `"opus"` and `"sonnet"`
/// (`a_local_file_overrides_the_user_file`, and four tests after it).
/// Those are real settings files as Claude Code reads them, so the
/// alias spelling is evidenced rather than assumed.
///
/// # What is deliberately absent
///
/// No third alias, and no full model id such as `claude-opus-5`.
///
/// The full ids DO appear in this tree -- `usage.rs` and `store.rs` read
/// them out of transcripts -- but they are OBSERVED OUTPUT, the name a
/// finished message reported, not evidence that `--model` accepts that
/// spelling as input. They also pin a version that ages out: a button
/// offering `claude-opus-4-7` keeps offering it after the alias has
/// moved on. The aliases are what the settings files use, and an alias
/// stays correct as models change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Model {
    Opus,
    Sonnet,
}

impl Model {
    /// The token this travels as, and the value the flag carries.
    ///
    /// One function for both because they ARE the same string -- the
    /// wire token is the flag value. Two functions would let them drift,
    /// and a drifted pair renders a flag nobody chose.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
        }
    }

    /// Every model this offers, in the order the UI lists them.
    pub const ALL: [Model; 2] = [Model::Opus, Model::Sonnet];
}

/// How much the session may do without asking.
///
/// # Evidence
///
/// `claude/hook.rs`'s fixtures are payloads Claude Code itself sends to
/// a hook, carrying `permission_mode`: `"acceptEdits"` at :704 and :715,
/// `"bypassPermissions"` at :810. That is Claude Code's own spelling of
/// its own modes, which is better evidence than one machine's
/// transcripts -- but it is still not a validated `--permission-mode`
/// vocabulary, because nothing proves the flag accepts the same tokens
/// the hook payload reports.
///
/// Which is why [`Self::Default`] exists and renders NO FLAG. "Start it
/// the way it normally starts" is a real choice, and expressing it by
/// guessing a spelling like `default` or `ask` -- neither of which
/// appears anywhere in this tree -- would be inventing vocabulary to
/// describe the one case that needs none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Whatever the binary does with no flag. Renders nothing.
    Default,
    /// `acceptEdits` -- evidenced by `hook.rs:704`, `:715`.
    AcceptEdits,
    /// `bypassPermissions` -- evidenced by `hook.rs:810`.
    BypassPermissions,
}

impl PermissionMode {
    /// The token this travels as.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    /// The `--permission-mode` value, or `None` to pass no flag.
    ///
    /// `Default`'s `None` is the whole reason this is separate from
    /// [`Self::token`]: `default` is a name for the absence of a flag in
    /// OUR vocabulary, and passing it to the binary would be asserting a
    /// spelling nothing here has seen.
    pub const fn flag_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::AcceptEdits => Some("acceptEdits"),
            Self::BypassPermissions => Some("bypassPermissions"),
        }
    }

    /// Whether this mode lets the agent act without being asked.
    ///
    /// Carried as data rather than re-derived from the token in the UI:
    /// the warning the panel shows is about the consequence, and a
    /// second copy of "which of these is the dangerous one" would drift
    /// from this one.
    pub const fn is_unattended(self) -> bool {
        matches!(self, Self::BypassPermissions)
    }

    /// Every mode this offers, in increasing order of autonomy.
    pub const ALL: [PermissionMode; 3] = [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
    ];
}

/// A token the caller sent that names no variant.
///
/// A REFUSAL and never a passthrough, which is the point. Passing an
/// unrecognised token through to the binary would be the string-valued
/// design wearing an enum's clothes: the caller would choose the text
/// that lands in the line that runs, which is exactly what
/// [`super::launch`] is written to prevent.
///
/// It names what was rejected and what is accepted, because the only
/// caller that can hit this is a build of the frontend that has drifted
/// from this list, and "invalid" alone would not say which end moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownTerm {
    /// `model` or `permissionMode` -- which choice was unrecognised.
    pub field: String,
    /// The token as it arrived. Echoed for diagnosis only; it never
    /// reaches a command line.
    pub got: String,
    /// The tokens that would have been accepted.
    pub accepted: Vec<String>,
}

impl std::fmt::Display for UnknownTerm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is not a {} Headstate can start a session on. It offers: {}.",
            self.got,
            self.field,
            self.accepted.join(", ")
        )
    }
}

/// The terms a launch runs on.
///
/// Both fields optional and both defaulting to "say nothing": an
/// unspecified term is the binary's own behaviour, which is what every
/// caller before #1214 got and what the launch buttons still do when the
/// user has not chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terms {
    pub model: Option<Model>,
    pub permission_mode: Option<PermissionMode>,
}

impl Terms {
    /// Map wire tokens to variants, or refuse.
    ///
    /// Takes `Option<&str>` rather than deriving `Deserialize` on the
    /// tokens directly so the refusal is OURS and says what is accepted.
    /// Serde's own error for an unknown variant is a parse failure at
    /// the IPC boundary with no room for a remedy, and #1214 asks for a
    /// refusal the user can act on.
    pub fn parse(model: Option<&str>, permission_mode: Option<&str>) -> Result<Self, UnknownTerm> {
        Ok(Self {
            model: lookup("model", model, &Model::ALL, Model::token)?,
            permission_mode: lookup(
                "permissionMode",
                permission_mode,
                &PermissionMode::ALL,
                PermissionMode::token,
            )?,
        })
    }

    /// The flag words these terms add, in argv order.
    ///
    /// `&'static str`, and that is load-bearing: every element is a
    /// literal in this file. Nothing here can interpolate, so there is
    /// no spelling of this function that lets caller text through --
    /// `no_flag_word_is_anything_but_a_compiled_literal` asserts it
    /// against a constant list, and a `format!` added later would fail
    /// to compile against the return type first.
    pub fn flags(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if let Some(m) = self.model {
            out.push("--model");
            out.push(m.token());
        }
        if let Some(v) = self.permission_mode.and_then(PermissionMode::flag_value) {
            out.push("--permission-mode");
            out.push(v);
        }
        out
    }

    /// Splice these flags into an already-built `claude` command line.
    ///
    /// The line arrives shell-quoted from `claudify_command` or
    /// `sessions::resume_command` and is NOT rebuilt here -- reusing
    /// them verbatim is what keeps the launched line the same line the
    /// clipboard would have carried, quoting and caveats included.
    ///
    /// # Why directly after the binary and not at the end
    ///
    /// Both builders end in a positional argument -- the prompt for a
    /// worktree, the session id for a resume -- and a flag appended
    /// after one is read by most CLIs as belonging to it or as another
    /// positional. Inserted after the program word, the flags precede
    /// every argument the line already had.
    ///
    /// The program word is found by scanning for the FIRST unquoted
    /// space after an optional `cd '...' && ` prefix, which is the only
    /// two shapes either builder emits. When the line does not have one
    /// of those shapes this returns it UNCHANGED rather than guessing:
    /// a launch on the terms the user did not choose is a bug, and a
    /// launch on a mangled command line is worse. `preview` shows the
    /// result either way, so an unchanged line is visible before it
    /// runs.
    pub fn splice(self, command: &str) -> String {
        let flags = self.flags();
        if flags.is_empty() {
            return command.to_string();
        }
        let Some(at) = program_word_end(command) else {
            return command.to_string();
        };
        let (head, tail) = command.split_at(at);
        format!("{head} {}{tail}", flags.join(" "))
    }

    /// The exact argv a launch would spawn, for showing the user.
    ///
    /// Returns the template's own `(program, args)` -- the SAME pair
    /// `launch` hands to `Command::new`, produced by the same
    /// `Template::render` call, so a display that drifts from what runs
    /// is not expressible. There is no separately-constructed string
    /// here on purpose: the whole reason this function exists is that a
    /// spawn path removed the user's chance to read the line, and a
    /// preview built by different code would restore the reading without
    /// restoring the guarantee.
    pub fn preview(
        self,
        template: &str,
        command: &str,
    ) -> Result<(String, Vec<String>), super::launch::LaunchError> {
        if template.trim().is_empty() {
            return Err(super::launch::LaunchError::NotConfigured);
        }
        Ok(super::launch::Template::parse(template)?.render(&self.splice(command)))
    }
}

/// Find the variant whose token is `want`, or refuse naming the field.
///
/// Generic over the enum so the two lookups cannot diverge in how they
/// refuse: one of them growing a "close enough" match while the other
/// stayed exact is the kind of drift that turns a closed vocabulary
/// back into an open one.
fn lookup<T: Copy>(
    field: &str,
    want: Option<&str>,
    all: &[T],
    token: fn(T) -> &'static str,
) -> Result<Option<T>, UnknownTerm> {
    let Some(want) = want else { return Ok(None) };
    all.iter()
        .copied()
        .find(|v| token(*v) == want)
        .map(Some)
        .ok_or_else(|| UnknownTerm {
            field: field.to_string(),
            got: want.to_string(),
            accepted: all.iter().map(|v| token(*v).to_string()).collect(),
        })
}

/// Where the `claude` program word ends in a built command line.
///
/// `claudify_command` emits `cd '<path>' && <claude> '<prompt>'` and
/// `resume_command` emits either `cd '<path>' && claude --resume '<id>'`
/// or that line without the prefix. So: skip a `cd '...' && ` prefix if
/// one is there, then take everything up to the next space.
///
/// Quote-aware rather than a plain `find(' ')` because the path inside
/// the `cd` can contain spaces, and so can the claude binary's own path
/// -- `find_claude` returns an absolute path, and a home directory with
/// a space in it is ordinary. A naive split would insert flags into the
/// middle of a quoted path.
fn program_word_end(command: &str) -> Option<usize> {
    let mut chars = command.char_indices();
    let mut quote: Option<char> = None;
    let mut seen_amp = false;
    // Whether we are past the `&&` -- before it, a space ends nothing.
    let mut start = 0usize;
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if quote != Some('\'') => {
                // Skip the escaped character. Never inside single
                // quotes, where a backslash is literal -- and
                // `shell_quote` escapes an embedded quote as `'\''`,
                // which closes the quote first.
                chars.next();
            }
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if Some(c) == quote => quote = None,
            '&' if quote.is_none() => {
                if seen_amp {
                    // `&&` at top level: the program word starts after
                    // the following space.
                    start = i + 1;
                    // Everything before this was the `cd`, so restart
                    // the scan for a space from here.
                    let rest = &command[start..];
                    let offset = rest.len() - rest.trim_start().len();
                    let body = &rest[offset..];
                    return Some(start + offset + first_unquoted_space(body)?);
                }
                seen_amp = true;
            }
            _ => seen_amp = false,
        }
    }
    // No `&&`: the whole line is the command, so the program word is
    // its first word.
    let _ = start;
    first_unquoted_space(command)
}

/// The byte index of the first space outside quotes, or `None`.
fn first_unquoted_space(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if quote != Some('\'') => {
                chars.next();
            }
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if Some(c) == quote => quote = None,
            ' ' if quote.is_none() => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::launch::Template;

    /// Every word `flags` is allowed to emit. Nothing else may appear.
    ///
    /// The list is spelled out here rather than derived from `flags`,
    /// which would make the test tautological: this is a second,
    /// independent statement of the vocabulary, and a variant added
    /// without a deliberate edit here fails.
    const ALLOWED: &[&str] = &[
        "--model",
        "opus",
        "sonnet",
        "--permission-mode",
        "acceptEdits",
        "bypassPermissions",
    ];

    /// THE property of #1214. Nothing a caller can say becomes a word.
    ///
    /// Asserted over every reachable combination rather than a chosen
    /// one: the claim is about the function, not about a sample.
    #[test]
    fn no_flag_word_is_anything_but_a_compiled_literal() {
        let mut seen = 0usize;
        for model in [None].into_iter().chain(Model::ALL.map(Some)) {
            for permission_mode in [None].into_iter().chain(PermissionMode::ALL.map(Some)) {
                for w in (Terms {
                    model,
                    permission_mode,
                })
                .flags()
                {
                    assert!(ALLOWED.contains(&w), "{w:?} is not a vocabulary word");
                    seen += 1;
                }
            }
        }
        // Guards the guard: a `flags` that returned nothing would pass
        // the loop above vacuously.
        assert!(seen > 0, "no flags were produced at all");
    }

    /// #1214's mandatory test. A metacharacter cannot become a word.
    ///
    /// Asserted on the argv VECTOR, not on a rendered string: a string
    /// assertion would pass while the same bytes were split by the
    /// spawner. Every hostile source is represented -- the path and
    /// prompt inside the built command, the session id, and an attempt
    /// at the flag value itself.
    #[test]
    fn a_metacharacter_never_becomes_a_second_argv_word() {
        // A worktree path, a branch name and a session id, each
        // carrying every shape the ticket names.
        let nasty_path = "/tmp/a b; rm -rf ~ && echo $(whoami) `id` \"q\" 'z'";
        let nasty_id = "s; rm -rf ~ && $(whoami) `id` 'q' \"z\" one two";
        let command = format!(
            "cd '{}' && claude --resume '{}'",
            nasty_path.replace('\'', r"'\''"),
            nasty_id.replace('\'', r"'\''")
        );

        let terms = Terms {
            model: Some(Model::Opus),
            permission_mode: Some(PermissionMode::BypassPermissions),
        };
        let (prog, argv) = terms
            .preview("open -a Terminal {command}", &command)
            .unwrap();

        assert_eq!(prog, "open");
        // Exactly three words, and the whole command -- flags and all --
        // is the third. This is the assertion the module exists for.
        assert_eq!(argv.len(), 3, "{argv:?}");
        assert_eq!(argv[0], "-a");
        assert_eq!(argv[1], "Terminal");
        assert!(argv[2].contains("--model opus"), "{}", argv[2]);
        assert!(
            argv[2].contains("--permission-mode bypassPermissions"),
            "{}",
            argv[2]
        );
        // The nasty text is all inside that one slot, byte for byte.
        assert!(argv[2].contains("rm -rf ~"), "{}", argv[2]);
        assert!(argv[2].contains("$(whoami)"), "{}", argv[2]);
        // And no argv word is a bare metacharacter, which is what a
        // split would have produced.
        for w in &argv {
            assert_ne!(w, ";");
            assert_ne!(w, "&&");
            assert_ne!(w, "rm");
            assert_ne!(w, "$(whoami)");
        }
    }

    /// The same, for a template whose placeholder shares a word.
    #[test]
    fn a_metacharacter_stays_in_one_word_with_a_glued_placeholder() {
        let terms = Terms {
            model: Some(Model::Sonnet),
            permission_mode: Some(PermissionMode::AcceptEdits),
        };
        let command = "cd '/tmp/x y; z' && claude --resume 'a b && c'";
        let (_p, argv) = terms.preview("term -e{command};exit", command).unwrap();
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(argv[0].starts_with("-ecd '/tmp/x y; z' && claude --model sonnet"));
        assert!(argv[0].ends_with(";exit"));
    }

    /// #1214's second mandatory test. An unknown token is REFUSED.
    #[test]
    fn an_unrecognised_term_is_refused_rather_than_passed_through() {
        for bad in [
            "haiku",
            "claude-opus-5",
            "opus ",
            "OPUS",
            "",
            "opus --dangerously-skip-permissions",
        ] {
            let e = Terms::parse(Some(bad), None).unwrap_err();
            assert_eq!(e.field, "model", "{bad:?}");
            assert_eq!(e.got, bad);
            // And it says what WOULD have worked, so a drifted frontend
            // is diagnosable from the message alone.
            assert!(e.to_string().contains("opus"), "{e}");
        }
        for bad in ["ask", "plan", "acceptedits", "bypass", "yolo", "default2"] {
            let e = Terms::parse(None, Some(bad)).unwrap_err();
            assert_eq!(e.field, "permissionMode", "{bad:?}");
            assert!(e.to_string().contains("bypassPermissions"), "{e}");
        }
    }

    /// The refusal is not "accept it and render nothing", which would
    /// be a silent downgrade to the default terms -- the user pressed a
    /// button that said `bypassPermissions` and would get a session that
    /// asks about everything, with nothing said.
    #[test]
    fn an_unrecognised_term_never_silently_becomes_the_default() {
        assert!(Terms::parse(Some("nope"), None).is_err());
        // The accepted tokens, by contrast, all round-trip.
        for m in Model::ALL {
            assert_eq!(
                Terms::parse(Some(m.token()), None).unwrap().model,
                Some(m),
                "{}",
                m.token()
            );
        }
        for p in PermissionMode::ALL {
            assert_eq!(
                Terms::parse(None, Some(p.token())).unwrap().permission_mode,
                Some(p),
                "{}",
                p.token()
            );
        }
    }

    /// #1214's third mandatory test. The preview IS the argv.
    ///
    /// Not "matches the argv" -- the same `Template::render` call
    /// produces both, so this asserts the two routes cannot differ by
    /// construction. Were `preview` to build its own display string,
    /// this is the test that would fail.
    #[test]
    fn the_previewed_line_is_the_argv_that_would_be_spawned() {
        let template = "wezterm start -- bash -lc {command}";
        let command = "cd '/tmp/w' && claude '/tmp/w prompt'";
        let terms = Terms {
            model: Some(Model::Opus),
            permission_mode: Some(PermissionMode::AcceptEdits),
        };

        let shown = terms.preview(template, command).unwrap();
        // The spawn path's own route to argv: `launch::launch` parses
        // the template and renders the spliced command, which is what
        // it hands to `Command::new`.
        let spawned = Template::parse(template)
            .unwrap()
            .render(&terms.splice(command));

        assert_eq!(shown, spawned);
        // And it really did carry the terms, so the equality above is
        // not between two empty things.
        assert!(shown.1.last().unwrap().contains("--model opus"));
        assert!(shown
            .1
            .last()
            .unwrap()
            .contains("--permission-mode acceptEdits"));
    }

    /// The preview refuses for the same reasons the launch does, so the
    /// user is not shown a line that could never have run.
    #[test]
    fn the_preview_reports_the_same_refusals_the_launch_would() {
        use super::super::launch::LaunchError;
        let t = Terms::default();
        assert_eq!(
            t.preview("   ", "claude").unwrap_err(),
            LaunchError::NotConfigured
        );
        assert!(matches!(
            t.preview("open -a Terminal", "claude").unwrap_err(),
            LaunchError::BadTemplate { .. }
        ));
    }

    /// Default terms change nothing at all, which is what every caller
    /// before #1214 got.
    #[test]
    fn no_terms_leaves_the_built_command_byte_identical() {
        let command = "cd '/tmp/x' && claude --resume 'abc-123'";
        assert_eq!(Terms::default().splice(command), command);
        assert!(Terms::default().flags().is_empty());
        // And `Default` permission mode is the same as saying nothing.
        let only_default = Terms {
            model: None,
            permission_mode: Some(PermissionMode::Default),
        };
        assert_eq!(only_default.splice(command), command);
        assert!(only_default.flags().is_empty());
    }

    /// The flags go where the binary reads them: after the program
    /// word, before the positional the line already ended with.
    #[test]
    fn the_flags_land_after_the_program_and_before_its_arguments() {
        let terms = Terms {
            model: Some(Model::Opus),
            permission_mode: None,
        };
        // Anchored resume.
        assert_eq!(
            terms.splice("cd '/tmp/x' && claude --resume 'abc'"),
            "cd '/tmp/x' && claude --model opus --resume 'abc'"
        );
        // Unanchored resume -- no `cd`, so the program word is first.
        assert_eq!(
            terms.splice("claude --resume 'abc'"),
            "claude --model opus --resume 'abc'"
        );
        // Claudify, whose binary is an absolute path and whose argument
        // is a long single-quoted prompt.
        assert_eq!(
            terms.splice("cd '/tmp/w' && /Users/me/.local/bin/claude 'do the thing'"),
            "cd '/tmp/w' && /Users/me/.local/bin/claude --model opus 'do the thing'"
        );
    }

    /// A space inside either quoted path does not end the program word.
    ///
    /// The reason the scan is quote-aware. A naive `find(' ')` would
    /// splice the flags into the middle of the `cd`'s path, producing a
    /// line that cds somewhere else entirely.
    #[test]
    fn a_space_inside_a_quoted_path_does_not_end_the_program_word() {
        let terms = Terms {
            model: Some(Model::Sonnet),
            permission_mode: None,
        };
        assert_eq!(
            terms.splice("cd '/tmp/my worktree' && claude --resume 'a b'"),
            "cd '/tmp/my worktree' && claude --model sonnet --resume 'a b'"
        );
        // And a binary path with a space in it.
        assert_eq!(
            terms.splice("cd '/tmp/w' && '/Users/my name/bin/claude' 'p q'"),
            "cd '/tmp/w' && '/Users/my name/bin/claude' --model sonnet 'p q'"
        );
    }

    /// An `&&` inside the quoted path is not the separator.
    ///
    /// A worktree branch named `x && y` reaches the `cd` quoted, and
    /// treating it as the separator would put the flags inside the path.
    #[test]
    fn an_ampersand_inside_a_quoted_path_is_not_the_separator() {
        let terms = Terms {
            model: Some(Model::Opus),
            permission_mode: None,
        };
        assert_eq!(
            terms.splice("cd '/tmp/a && b' && claude --resume 'x'"),
            "cd '/tmp/a && b' && claude --model opus --resume 'x'"
        );
    }

    /// A shape neither builder produces is left ALONE, not guessed at.
    ///
    /// The terms are then not applied, which `preview` shows -- a
    /// visible omission beats a mangled command line.
    #[test]
    fn an_unrecognised_command_shape_is_returned_unchanged() {
        let terms = Terms {
            model: Some(Model::Opus),
            permission_mode: None,
        };
        // One word, no arguments: there is no "after the program word".
        assert_eq!(terms.splice("claude"), "claude");
        assert_eq!(terms.splice(""), "");
    }

    /// The two spellings a `--permission-mode` value can have are the
    /// two the hook fixtures carry, and `Default` has none.
    #[test]
    fn only_the_evidenced_permission_values_ever_reach_a_flag() {
        assert_eq!(PermissionMode::Default.flag_value(), None);
        assert_eq!(
            PermissionMode::AcceptEdits.flag_value(),
            Some("acceptEdits")
        );
        assert_eq!(
            PermissionMode::BypassPermissions.flag_value(),
            Some("bypassPermissions")
        );
        // Spelled exactly as `hook.rs`'s fixtures spell them. Read from
        // that file rather than copied, so a fixture that changed would
        // fail here rather than leaving two copies to drift.
        let hook = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("claude")
                .join("hook.rs"),
        )
        .expect("hook.rs is where the evidenced spellings live");
        for m in PermissionMode::ALL {
            let Some(v) = m.flag_value() else { continue };
            assert!(
                hook.contains(&format!("\"permission_mode\": \"{v}\"")),
                "{v} is offered but `hook.rs` has no fixture carrying it -- \
                 #1214 forbids inventing vocabulary"
            );
        }
    }

    /// The model aliases are the ones the settings fixtures carry, read
    /// from that file for the same reason.
    #[test]
    fn only_the_evidenced_model_aliases_are_offered() {
        let settings = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("claude")
                .join("settings.rs"),
        )
        .expect("settings.rs is where the evidenced aliases live");
        for m in Model::ALL {
            assert!(
                settings.contains(&format!("\"model\": \"{}\"", m.token())),
                "{} is offered but `settings.rs` has no fixture carrying it -- \
                 #1214 forbids inventing vocabulary",
                m.token()
            );
        }
    }

    /// Only one mode is unattended, and it is the one named for it.
    #[test]
    fn exactly_one_offered_mode_acts_without_asking() {
        let unattended: Vec<_> = PermissionMode::ALL
            .into_iter()
            .filter(|m| m.is_unattended())
            .collect();
        assert_eq!(unattended, vec![PermissionMode::BypassPermissions]);
    }

    /// The wire form is the token, so the frontend's list and this enum
    /// cannot disagree about spelling.
    #[test]
    fn the_serialised_form_is_the_token() {
        for m in Model::ALL {
            assert_eq!(
                serde_json::to_value(m).unwrap(),
                serde_json::Value::String(m.token().to_string())
            );
        }
        for p in PermissionMode::ALL {
            assert_eq!(
                serde_json::to_value(p).unwrap(),
                serde_json::Value::String(p.token().to_string())
            );
        }
    }
}
