//! The restart list: every running session's resume command, as text
//! the user saves before a reboot (#1071).
//!
//! # The question this answers, and why it is not the overview's
//!
//! "I am about to restart this machine. Give me the lines I need to
//! bring everything back." That is a different question from the
//! overview page's, and the difference is the whole design:
//!
//! | page | selects | error that matters |
//! |---|---|---|
//! | overview's resumable list | NOT running, cwd exists | offering to resume something alive |
//! | this export | running (and might-be-running) | omitting work the user then loses |
//!
//! The overview is about what is already stopped. This is about what is
//! about to be stopped, by the user, deliberately -- and the two want
//! opposite treatment of an uncertain row.
//!
//! # Inclusive on uncertainty, and it says so
//!
//! [`super::liveness::Liveness`] has three states and `Unknown` is
//! explicitly NOT a shade of `Dead`. #984 is what happens when that is
//! forgotten: one registry read failed, `derive` returned `Unknown` for
//! everything, and the overview's `resumable` predicate -- "absent from
//! the running set" -- misclassified **183 of 1,491** sessions.
//!
//! In export form that defect is not a wrong tile, it is lost work. So
//! the two error directions are weighed rather than treated as equal:
//!
//! - A session **wrongly omitted** is work the user rebooted away. They
//!   saved the list and trusted it; there is no second chance to notice.
//! - A session **wrongly included** is one pasted line that resumes
//!   something already finished. Visible the moment it runs, and
//!   undone by closing the window.
//!
//! So `Unknown` is INCLUDED -- in its own clearly-labelled section, never
//! silently mixed in with the running ones, because "we could not tell"
//! is a different claim from "this is running" and a reader deciding what
//! to paste needs to see which is which.
//!
//! `Dead` is excluded. That is the one verdict the app actually reached,
//! and a restart list padded with 1,213 archived sessions is not a
//! restart list.
//!
//! # The export QUALIFIES itself, per the house rule
//!
//! Only-low -> qualify; possibly-wrong -> suppress. Both shortfalls here
//! are only-low, so both are qualified rather than suppressed:
//!
//! - `registry_failure` means the live registry could not be listed at
//!   all, so nothing could be positively established as running. Every
//!   row becomes `Unknown` and the header says the list may be short.
//! - `registry_unreadable` means individual records were present and
//!   unusable. Each one hides a session that may be running, so the
//!   count is a floor.
//!
//! Neither suppresses the export. A short list the user is TOLD is short
//! is worth more than no list at all before a reboot -- and the rows it
//! does carry were each positively established.
//!
//! # The commands are not built here
//!
//! [`super::sessions::resume_command`] builds them, and reusing it is not
//! a convenience -- it is the only way this export inherits the hardening
//! that function carries. Both halves of the line are shell-quoted (the
//! id is `path.file_stem()` of an arbitrary `*.jsonl`, not a validated
//! UUID, so a file named `` `id`.jsonl `` would otherwise put live shell
//! syntax in a list the user pastes blind), and there are FOUR cwd cases
//! rather than two, each with its own wording. A second implementation
//! here would reproduce the bug a review already caught once.
//!
//! # No terminal is spawned, and no file is written
//!
//! The user asked for text to paste, and that is what this returns.
//! Spawning terminals is a different feature with a much worse failure
//! mode -- macOS has no default-terminal concept, and `x-terminal-emulator`
//! is Debian-only, which is the same argument `sessions.rs` makes for
//! returning strings rather than launching anything. Writing to a path
//! the app picked would leave the restart list somewhere the user did not
//! choose and might never find; the frontend offers the text to copy or
//! to save wherever they want.

use serde::{Deserialize, Serialize};

use super::sessions::{check_cwd, resume_command, ResumeCommand};

/// One line of the restart list.
///
/// Carries the built [`ResumeCommand`] rather than the parts, because the
/// caveat and the command have to be right about each other -- a caveat
/// re-derived on the frontend from a `cwd` string would drift from the
/// command it sits above, which is exactly the failure #918 built
/// `ResumeCommand` to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartEntry {
    pub session_id: String,
    /// Claude's own `aiTitle`, or `None`. Never the UUID dressed up as a
    /// name: a fabricated title cannot be told from a real one.
    pub name: Option<String>,
    /// The directory the command will `cd` into, when it has one. The
    /// live registry's copy wins over the stored one for a running
    /// session, because a live session republishes its own.
    pub cwd: Option<String>,
    /// The line to paste, with its caveat and whether it carries a `cd`.
    pub resume: ResumeCommand,
}

/// Why a session is in the "might be running" half.
///
/// The sentence `liveness::derive` produced, carried verbatim. A section
/// heading saying "we could not tell" with no grounds is the shrug epic
/// #941 exists to remove -- the reader deciding whether to paste these
/// lines needs to know whether the registry was unreadable or the
/// process table was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncertainEntry {
    #[serde(flatten)]
    pub entry: RestartEntry,
    /// `Liveness::Unknown`'s own `why`, unmodified.
    pub why: String,
}

/// The restart list, with everything that qualifies it.
///
/// Two vectors rather than one with a flag, because they are two
/// different claims and the export renders them under two different
/// headings. A single list sorted by confidence would let a reader paste
/// past the boundary without noticing it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartList {
    /// Sessions positively established as running: the process is there
    /// and its start time matches what was recorded.
    pub running: Vec<RestartEntry>,
    /// Sessions whose liveness could not be decided. Included on purpose
    /// -- see the module docs for the 183-of-1,491 case this is written
    /// against.
    pub uncertain: Vec<UncertainEntry>,
    /// Why the live session registry could not be listed. `Some` means
    /// nothing could be positively established as running, so `running`
    /// is empty for a reason that is NOT "nothing is running".
    pub registry_failure: Option<String>,
    /// Registry records present but unusable, with why. Each one hides a
    /// session that may be running, so the list is a floor.
    pub registry_unreadable: Vec<String>,
}

impl RestartList {
    /// How many lines the export carries in total.
    ///
    /// Both halves, because both are lines the user will paste. The
    /// header states the split separately; this is the figure that
    /// answers "how much is in this file".
    pub fn total(&self) -> usize {
        self.running.len() + self.uncertain.len()
    }

    /// Whether anything makes this list a floor rather than a count.
    ///
    /// Either shortfall alone is enough. A caller that checked only
    /// `registry_failure` would print an unqualified count on a machine
    /// where a record could not be parsed, which is the confident number
    /// that might be wrong.
    pub fn may_be_short(&self) -> bool {
        self.registry_failure.is_some() || !self.registry_unreadable.is_empty()
    }
}

/// Build the restart list from an already-derived session list.
///
/// Takes [`super::sessions::SessionList`] rather than reading the
/// registry again, and that is deliberate: the list has already derived
/// every row's liveness from one registry read and one process probe, and
/// a second derivation here would be a second answer to one question --
/// the defect `claude/mod.rs` records as #984, where two surfaces
/// disagreed about the same rows off the same read.
///
/// # Why the cwd is re-checked here and not read from the row
///
/// The row carries a `cwd_state` and this calls [`check_cwd`] again. Not
/// redundant: the export is the thing the user pastes AFTER a reboot, and
/// the honest moment to ask whether the directory exists is the moment
/// the line is built. The list's copy was taken for a chip count on
/// whatever poll happened to land, which can be ten seconds stale -- and
/// a `cd` into a directory deleted in between is the one failure this
/// export must not produce.
pub fn restart_list(list: &super::sessions::SessionList) -> RestartList {
    let mut out = RestartList {
        registry_failure: list.registry_failure.clone(),
        registry_unreadable: list.registry_unreadable.clone(),
        ..Default::default()
    };

    for row in &list.sessions {
        // Matched rather than tested with a helper that returns
        // `!running`. `Liveness::is_running` deliberately has no
        // `is_dead` twin for this reason: a boolean test forgets the
        // third arm, and forgetting the third arm here is #984.
        match &row.liveness {
            super::sessions::ListLiveness::Running { .. } => {
                out.running.push(entry(row));
            }
            super::sessions::ListLiveness::Unknown { why } => {
                out.uncertain.push(UncertainEntry {
                    entry: entry(row),
                    // The reason is INTERNED in the list, so an index out
                    // of range would be a verdict with no grounds. Said
                    // in words rather than defaulted to an empty string,
                    // which would render as a heading with nothing under
                    // it.
                    why: list.reasons.get(*why).cloned().unwrap_or_else(|| {
                        "the reason for this verdict could not be resolved".into()
                    }),
                });
            }
            // Excluded, and this is the only exclusion. `Dead` is a
            // verdict the app actually reached.
            super::sessions::ListLiveness::Dead { .. } => {}
        }
    }

    out
}

/// One row, with its command built against a fresh directory check.
fn entry(row: &super::sessions::ListRow) -> RestartEntry {
    let state = check_cwd(row.cwd.as_deref());
    RestartEntry {
        resume: resume_command(&row.session_id, row.cwd.as_deref(), &state),
        session_id: row.session_id.clone(),
        name: row.name.clone(),
        cwd: row.cwd.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::sessions::{ListLiveness, ListRow, SessionList};
    use crate::claude::signals::{NotWaiting, Waiting};
    use crate::claude::subagent::Kind;

    /// A directory that really exists on every platform.
    ///
    /// `"/tmp"` does not exist on Windows, where `check_cwd` correctly
    /// returns `Gone` -- which has failed tests on the `windows-latest`
    /// job before. `temp_dir()` is `TEMP` there and `/tmp` here.
    fn real_dir() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    fn row(session_id: &str, cwd: Option<&str>, liveness: ListLiveness) -> ListRow {
        ListRow {
            // #1133: not part of what the restart list exports.
            opening_prompt: None,
            session_id: session_id.into(),
            name: None,
            cwd: cwd.map(str::to_string),
            git_branch: None,
            last_activity_at: None,
            liveness,
            cwd_state: crate::claude::sessions::check_cwd(cwd),
            kind: Kind::Own,
            subagents: 0,
            waiting: Waiting::No {
                why: NotWaiting::NeverObserved,
            },
            context_pressure: None,
        }
    }

    fn list(sessions: Vec<ListRow>, reasons: Vec<String>) -> SessionList {
        SessionList {
            sessions,
            reasons,
            registry_failure: None,
            registry_unreadable: Vec::new(),
        }
    }

    /// A `Gone` directory exports the BARE command, with its caveat.
    ///
    /// The failure this rules out is the one that looks like it worked:
    /// `cd /deleted/worktree && claude --resume x` fails in the shell,
    /// the `&&` short-circuits, and the user sees an error they have to
    /// read carefully to distinguish from "claude is broken". The bare
    /// command RUNS -- which is why the caveat above it is not optional.
    #[test]
    fn a_gone_directory_exports_the_bare_command_with_its_caveat() {
        let gone = "/definitely/not/a/directory/anywhere";
        let out = restart_list(&list(
            vec![row(
                "sess-gone",
                Some(gone),
                ListLiveness::Running {
                    pid: 1,
                    status: None,
                },
            )],
            Vec::new(),
        ));

        assert_eq!(out.running.len(), 1);
        let e = &out.running[0];
        assert!(
            !e.resume.anchored,
            "a directory that is gone must not be anchored"
        );
        assert!(
            !e.resume.command.contains("cd "),
            "the exported line must not cd into a directory that no longer \
             exists: {}",
            e.resume.command
        );
        assert_eq!(e.resume.command, "claude --resume 'sess-gone'");
        let caveat = e
            .resume
            .caveat
            .as_deref()
            .expect("an unanchored line must carry its caveat, or the user pastes it blind");
        assert!(
            caveat.contains(gone),
            "the caveat must name the directory that is gone: {caveat}"
        );
    }

    /// A session id carrying shell metacharacters round-trips quoted.
    ///
    /// The id is `path.file_stem()` of any `*.jsonl` under
    /// `~/.claude/projects/<slug>/` with no format check, so this is a
    /// real input rather than a hypothetical one -- and this export is
    /// the surface where it does the most damage, because the user
    /// pastes a whole file of lines without reading each one.
    #[test]
    fn a_session_id_with_shell_metacharacters_round_trips_quoted() {
        let nasty = "$(whoami)`id`;rm -rf ~";
        let dir = real_dir();
        let out = restart_list(&list(
            vec![row(
                nasty,
                Some(&dir),
                ListLiveness::Running {
                    pid: 1,
                    status: None,
                },
            )],
            Vec::new(),
        ));

        assert_eq!(out.running.len(), 1);
        let command = &out.running[0].resume.command;
        assert!(
            command.contains(&format!("claude --resume '{nasty}'")),
            "the id must be single-quoted verbatim: {command}"
        );
        // The id appears ONLY inside quotes. Asserted as the absence of
        // an unquoted occurrence rather than as the presence of a quoted
        // one, because a line carrying both would satisfy the check
        // above and still execute the substitution.
        let outside = command.replace(&format!("'{nasty}'"), "");
        for metachar in ['$', '`', ';'] {
            assert!(
                !outside.contains(metachar),
                "`{metachar}` escaped the quoting in {command}"
            );
        }
    }

    /// A registry that could not be read makes the list a floor.
    ///
    /// The failure is carried through rather than swallowed: with no
    /// registry, nothing can be positively established as running, and an
    /// export that printed "0 running sessions" under those conditions is
    /// the fail-open #841 names -- "nothing is running" is exactly what
    /// makes a user confident it is safe to reboot.
    #[test]
    fn a_registry_failure_makes_the_export_say_the_list_may_be_short() {
        let mut l = list(Vec::new(), Vec::new());
        l.registry_failure = Some("could not read ~/.claude/sessions: permission denied".into());
        let out = restart_list(&l);

        assert!(
            out.may_be_short(),
            "a registry we could not read leaves the list a floor, not a count"
        );
        assert_eq!(
            out.registry_failure.as_deref(),
            Some("could not read ~/.claude/sessions: permission denied"),
            "the reason travels with the shortfall -- a bare flag is a shrug"
        );
    }

    /// An unreadable record alone is enough to qualify the list.
    ///
    /// Separate from the test above because the two shortfalls arrive
    /// independently: a caller that checked only `registry_failure` would
    /// print an unqualified count on a machine where one record was
    /// unparseable, and each such record hides a session that may be
    /// running.
    #[test]
    fn an_unreadable_record_alone_makes_the_list_a_floor() {
        let mut l = list(Vec::new(), Vec::new());
        l.registry_unreadable = vec!["4821.json: invalid JSON at line 1".into()];
        let out = restart_list(&l);

        assert!(out.registry_failure.is_none(), "the directory WAS listed");
        assert!(
            out.may_be_short(),
            "a record that could not be parsed hides a session that may be running"
        );
    }

    /// An `Unknown` liveness is INCLUDED, in its own half.
    ///
    /// #984 in export form. `Unknown` is not a shade of `Dead`, and a
    /// restart list that dropped it would have silently omitted 183 of
    /// 1,491 sessions on the measured corpus -- work the user would have
    /// rebooted away having trusted the file.
    #[test]
    fn an_unknown_liveness_session_is_included_rather_than_dropped() {
        let dir = real_dir();
        let out = restart_list(&list(
            vec![
                row(
                    "sess-running",
                    Some(&dir),
                    ListLiveness::Running {
                        pid: 1,
                        status: None,
                    },
                ),
                row("sess-unknown", Some(&dir), ListLiveness::Unknown { why: 0 }),
                row("sess-dead", Some(&dir), ListLiveness::Dead { why: 1 }),
            ],
            vec![
                "the live session registry could not be read".into(),
                "no process with that pid".into(),
            ],
        ));

        assert_eq!(
            out.uncertain.len(),
            1,
            "a session we could not decide about must appear, not vanish"
        );
        assert_eq!(out.uncertain[0].entry.session_id, "sess-unknown");
        assert_eq!(
            out.uncertain[0].why, "the live session registry could not be read",
            "the reason is resolved from the list's own table, not invented"
        );
        // And NOT mixed into the confident half: the two are different
        // claims, and a reader pasting the file has to be able to see
        // the boundary.
        assert_eq!(out.running.len(), 1);
        assert_eq!(out.running[0].session_id, "sess-running");
        assert_eq!(out.total(), 2, "both halves are lines the user will paste");
        // The one verdict the app actually reached is the one exclusion.
        assert!(
            !out.running
                .iter()
                .chain(out.uncertain.iter().map(|u| &u.entry))
                .any(|e| e.session_id == "sess-dead"),
            "a session established as dead is not part of a restart list"
        );
    }

    /// An unresolvable reason index says so rather than rendering empty.
    ///
    /// The index comes from the list's own interning table, so this is
    /// not reachable today -- but an empty `why` would render as a
    /// heading promising grounds with nothing under it, which is worse
    /// than a sentence admitting the resolution failed.
    #[test]
    fn an_unresolvable_reason_index_still_carries_words() {
        let dir = real_dir();
        let out = restart_list(&list(
            vec![row(
                "sess-unknown",
                Some(&dir),
                ListLiveness::Unknown { why: 7 },
            )],
            Vec::new(),
        ));

        assert_eq!(out.uncertain.len(), 1);
        assert!(
            !out.uncertain[0].why.is_empty(),
            "a verdict with no grounds is the shrug the reason field exists to prevent"
        );
    }

    /// An existing directory is anchored and carries no caveat.
    ///
    /// The normal case, pinned so the `cd` cannot quietly disappear from
    /// every line -- which would make the whole export resume every
    /// session in whatever directory the user happened to be in, with
    /// nothing on screen looking wrong.
    #[test]
    fn an_existing_directory_is_anchored_and_needs_no_caveat() {
        let dir = real_dir();
        let out = restart_list(&list(
            vec![row(
                "sess-live",
                Some(&dir),
                ListLiveness::Running {
                    pid: 9,
                    status: Some("busy".into()),
                },
            )],
            Vec::new(),
        ));

        let e = &out.running[0];
        assert!(e.resume.anchored);
        assert_eq!(e.resume.caveat, None);
        assert!(e.resume.command.starts_with("cd '"));
        assert!(e.resume.command.contains("&& claude --resume 'sess-live'"));
    }

    /// A session with no recorded cwd gets the bare command and a caveat
    /// worded for THAT case.
    ///
    /// `NotRecorded` and `Gone` are different facts -- "we never knew
    /// where it ran" against "it ran somewhere that is now deleted" --
    /// and #918 keeps the wording distinct on purpose. The export must
    /// not collapse them, since the caveat is the only thing the user
    /// reads before pasting.
    #[test]
    fn a_session_with_no_recorded_directory_says_that_and_not_gone() {
        let out = restart_list(&list(
            vec![row(
                "sess-nowhere",
                None,
                ListLiveness::Running {
                    pid: 3,
                    status: None,
                },
            )],
            Vec::new(),
        ));

        let e = &out.running[0];
        assert!(!e.resume.anchored);
        assert_eq!(e.resume.command, "claude --resume 'sess-nowhere'");
        let caveat = e.resume.caveat.as_deref().expect("still needs a caveat");
        assert!(
            caveat.contains("No working directory was recorded"),
            "not-recorded must not be worded as gone: {caveat}"
        );
    }

    /// A list with nothing running and no shortfall is a settled zero.
    ///
    /// The distinction the whole module turns on: this is "we looked and
    /// nothing is running", which the export is allowed to state plainly.
    /// Only a shortfall turns it into "we could not tell".
    #[test]
    fn nothing_running_with_a_readable_registry_is_a_settled_empty_answer() {
        let dir = real_dir();
        let out = restart_list(&list(
            vec![row("sess-dead", Some(&dir), ListLiveness::Dead { why: 0 })],
            vec!["no process with that pid".into()],
        ));

        assert_eq!(out.total(), 0);
        assert!(
            !out.may_be_short(),
            "a readable registry that found nothing running is an ANSWER, not a floor"
        );
    }
}
