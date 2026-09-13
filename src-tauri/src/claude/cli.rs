//! The command-line front door for [`super::hook`] (#912).
//!
//! One subcommand, `claude-hook`, dispatched from `main` **before**
//! anything else in the process runs.
//!
//! # Why a subcommand of the app binary rather than a second binary
//!
//! The installer (#915) has to write a hook command line into a settings
//! file and that line has to keep working after the app upgrades and after
//! the user moves the app. That requirement decides this, and it decides it
//! in favour of the existing binary:
//!
//! - **The app binary's path is already the one stable path we have.**
//!   `Headstate.app/Contents/MacOS/headstate` is where macOS itself
//!   launches the app from, so it is a path the bundle format guarantees
//!   rather than one we invent. The Tauri updater replaces the bundle
//!   in place at the same location, so an upgrade does not move it.
//! - **A second binary would need to be bundled, and bundling it is the
//!   part that breaks.** Tauri puts one binary in `Contents/MacOS`; a
//!   sidecar lands under `Contents/Resources` with a target-triple suffix
//!   in its name (`headstate-claude-hook-aarch64-apple-darwin`), which
//!   means the installed hook command line differs per architecture and
//!   an Intel-to-ARM migration silently points at a path that no longer
//!   exists. It also needs its own signing and notarization entry, and an
//!   unsigned helper inside a notarized bundle is a Gatekeeper failure
//!   that shows up only on someone else's machine.
//! - **Neither option survives the app being moved**, so that requirement
//!   does not discriminate between them -- both are absolute paths into a
//!   bundle. What it means is that the installer owns a reinstall action
//!   (#910 deliverable 2 already specifies install/reinstall/uninstall),
//!   and the honest thing is that reinstall is how a moved app is fixed.
//!   Since neither design avoids that, the tie breaks on the points above.
//!
//! The cost of this choice is real and worth writing down: the hook pays
//! the app binary's load cost, because a subcommand of a large binary still
//! has to be mapped and linked before `main` is reached. That cost was
//! MEASURED rather than assumed: a 30MB release binary (the installed,
//! stripped bundle on this machine is 70MB) runs the whole hook in **5.5ms
//! median, 6.3ms worst over 20 runs** -- 274x and 238x inside the 1.5s
//! budget, and comfortably faster than the 27.7ms shell probe #912 cites.
//!
//! The reason a 30MB binary is nearly free here is that dyld maps lazily:
//! the pages backing the webview, TLS and SQLite stacks are never touched
//! on this path, because the dispatch below returns before any of it is
//! constructed. That is a property of the ORDERING, not of the binary --
//! which is why the ordering has a guard of its own below.
//!
//! # What must not happen above this dispatch
//!
//! `lib::run` installs a crypto provider, builds a Tauri app, opens the
//! database, starts the poll loop and binds the phone listener. Measured:
//! running the shipped binary with an unrecognised argument today does all
//! of that -- it logged a poll tick, a GitHub query, and a listener bind
//! failure, and it never exited.
//!
//! So this dispatch sits in `main` **before** `lib::run` is called, and
//! that ordering is the load-bearing part. `hook_runs_before_any_app_setup`
//! in this module asserts `main.rs` keeps the dispatch above the `run()`
//! call, because the failure mode of getting it wrong is not a compile
//! error -- it is a hook that boots an entire GUI application twice per
//! session, blows the 1.5s budget, and fights the real app for the
//! listener port.

use std::io::Read;
use std::path::Path;

/// The subcommand name, as it appears in an installed hook command line.
///
/// `claude-hook` rather than `hook`: this binary is a GUI app first, and a
/// bare `hook` says nothing about whose hook it is. #915 writes this string
/// into a settings file that a human reads and edits, and the name has to
/// be self-explaining there.
pub const SUBCOMMAND: &str = "claude-hook";

/// What the process should do, decided from its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    /// Be the hook: read stdin, append one line, exit.
    Hook,
    /// Be the app. Every other argument vector, including none.
    App,
}

/// Classify an argument vector.
///
/// `args` is the full vector including `argv[0]`, exactly as
/// `std::env::args()` yields it, so the caller does not have to remember to
/// skip the program name.
///
/// # Why only an exact match in position 1
///
/// macOS passes `-psn_0_...` to GUI-launched apps, and a future flag could
/// appear anywhere; searching the whole vector for the subcommand name
/// would let an unrelated argument that happens to contain it turn the app
/// launch into a hook run, which would look like the app failing to start.
/// A subcommand is positional by definition, so position 1 is the only
/// place it can legitimately be.
pub fn classify<I, S>(args: I) -> Invocation
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    let _argv0 = it.next();
    match it.next() {
        Some(a) if a.as_ref() == SUBCOMMAND => Invocation::Hook,
        _ => Invocation::App,
    }
}

/// The pid of the process that ran us, which is the Claude Code process.
///
/// See [`super::hook`]'s module docs for why this is the parent taken
/// directly and not a search up the tree for something named `claude`.
///
/// # Why this is `cfg`-gated rather than one call
///
/// There is no portable `getppid` in `std`. The stable function is
/// `std::os::unix::process::parent_id`, which is Unix-only -- MEASURED, by
/// writing `std::process::parent_id()` first and having the compiler reject
/// it. Windows has no ppid in `std` at all: a parent there is found via
/// `CreateToolhelp32Snapshot`, and a Windows parent is not even guaranteed
/// to still exist, since the OS does not keep the relationship alive.
///
/// So the Windows arm records `0` rather than pretending. `0` is not a
/// valid pid on any platform, which makes it a value #913 can recognise as
/// "this writer could not tell us" instead of a pid it would then try to
/// check for liveness -- absent is not zero, spelled as a sentinel because
/// the field is not optional (see [`super::hook::Record`] on why `ppid` is
/// one of the three fields that always comes from us).
///
/// This is not a gap being papered over: Claude Code hooks on Windows are
/// outside what #910 scopes, and the app's own `libc` dependency is already
/// `cfg(unix)`-gated in `Cargo.toml` for the same reason. What matters is
/// that the Windows build compiles and writes a record a reader cannot
/// misread.
#[cfg(unix)]
pub fn parent_pid() -> u32 {
    std::os::unix::process::parent_id()
}

/// Windows has no parent pid in `std`; see the Unix arm's docs.
#[cfg(not(unix))]
pub fn parent_pid() -> u32 {
    0
}

/// The current time as RFC 3339 in UTC, which is what a record's `ts` is.
///
/// `chrono` is already a direct dependency, and this is the same spelling
/// the rest of the tree uses for a stored timestamp.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Run the hook and exit the process.
///
/// Never returns: a hook has nothing to do afterwards, and returning into
/// `main` would fall through to the app launch.
///
/// # Why failures exit 0 with a message on stderr
///
/// A non-zero exit from a hook is a signal Claude Code acts on, and on
/// `SessionEnd` there is nothing useful for it to do with our failure --
/// the session is already over. Blocking or warning the user because
/// Headstate could not write its own bookkeeping file would make an
/// optional integration degrade the tool it is observing, which is the one
/// thing a hook must never do.
///
/// The message still goes to stderr rather than nowhere, so that a user
/// debugging a missing session has something to find when they run the
/// command by hand -- which the issue asks to remain possible.
///
/// Proven by sabotage: returning `1` from the `Err` arm below fails
/// `tests::a_failed_append_still_exits_zero`, and the test's own output
/// shows the message reaching stderr ("could not append to ...: Not a
/// directory (os error 20)"), so the failure is reported rather than
/// swallowed even though the exit code forgives it.
///
/// Nothing is written to **stdout**. Claude Code reads a hook's stdout as
/// hook output, so printing the record there would be feeding it data it
/// did not ask for.
pub fn run_hook_and_exit() -> ! {
    let mut stdin = String::new();
    // A read failure is not fatal to writing a record: the pid and the
    // timestamp come from us, not from stdin, so an empty payload still
    // yields a line that tells #913 a session event happened. That is the
    // same argument `hook::run` makes for an unparseable payload.
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("headstate {SUBCOMMAND}: could not read the payload on stdin: {e}");
    }

    let home = crate::auth::home_dir();
    let code = match home {
        Some(home) => write_one(&stdin, &home),
        // No home means no `~/.claude`, so there is no file to append to
        // and nothing this process can do about it. Said out loud rather
        // than swallowed, because a silently absent integration is the
        // failure this codebase keeps refusing to ship.
        None => {
            eprintln!(
                "headstate {SUBCOMMAND}: no home directory, so there is no \
                 ~/.claude/headstate/sessions.jsonl to write to"
            );
            0
        }
    };
    std::process::exit(code);
}

/// The one append, with its error reported rather than swallowed.
///
/// Split out from [`run_hook_and_exit`] so the success path is testable
/// without exiting the test process.
fn write_one(stdin: &str, home: &Path) -> i32 {
    match super::hook::run(stdin, home, parent_pid(), &now_rfc3339()) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!(
                "headstate {SUBCOMMAND}: could not append to {}: {e}",
                super::hook::handoff_path_in(home).display()
            );
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hook whose append FAILS still exits 0.
    ///
    /// The rule this pins is the one in [`run_hook_and_exit`]'s docs: an
    /// optional integration must never degrade the tool it observes. A
    /// non-zero exit is a signal Claude Code acts on, and on `SessionEnd`
    /// there is nothing useful it could do with our bookkeeping failure --
    /// the session is already over.
    ///
    /// The failure is real rather than mocked: `home` is a path whose
    /// parent is a FILE, so `create_dir_all` cannot succeed. That exercises
    /// the same `Err` arm an unwritable `~` or a full disk would.
    #[test]
    fn a_failed_append_still_exits_zero() {
        let t = tempfile::TempDir::new().unwrap();
        let blocker = t.path().join("not-a-directory");
        std::fs::write(
            &blocker,
            b"this is a file, so nothing can be created under it",
        )
        .unwrap();

        // `<blocker>/.claude/headstate/` cannot be created.
        let code = write_one("{}", &blocker);

        assert_eq!(
            code, 0,
            "a hook that could not write its own file must not report failure \
             to Claude Code -- an optional integration never degrades the tool \
             it observes"
        );
        assert!(
            !super::super::hook::handoff_path_in(&blocker).exists(),
            "the fixture must really have prevented the write, or this test \
             proves nothing about the failure path"
        );
    }

    /// The success path returns 0 too, so the assertion above is about the
    /// FAILURE arm rather than about a function that always returns 0 for
    /// uninteresting reasons. Without this, `fn write_one(..) -> i32 { 0 }`
    /// would satisfy the test above.
    #[test]
    fn a_successful_append_exits_zero_and_writes_the_file() {
        let home = tempfile::TempDir::new().unwrap();

        let code = write_one(
            r#"{"hook_event_name":"SessionStart","session_id":"abc","source":"startup"}"#,
            home.path(),
        );

        assert_eq!(code, 0);
        let body =
            std::fs::read_to_string(super::super::hook::handoff_path_in(home.path())).unwrap();
        assert!(body.contains("\"session_id\":\"abc\""), "got {body}");
        // The pid is the real one here -- this test runs as a child of the
        // test harness, so the only thing worth asserting is that a real
        // pid was recorded rather than the `0` sentinel.
        assert!(
            !body.contains("\"ppid\":0,"),
            "a Unix build must record a real parent pid, not the \
             cannot-say sentinel: {body}"
        );
    }

    /// `parent_pid` on Unix returns the real parent, which for a test is
    /// whatever ran the harness. Only its shape can be asserted -- but the
    /// shape is what matters, because `0` is the documented "cannot say"
    /// sentinel and a Unix build must never emit it.
    #[test]
    fn the_parent_pid_is_a_real_pid_on_this_platform() {
        let ppid = parent_pid();

        if cfg!(unix) {
            assert_ne!(
                ppid, 0,
                "a Unix build must read a real ppid; 0 is the sentinel \
                 reserved for platforms that cannot tell us"
            );
        }
    }

    #[test]
    fn the_subcommand_in_position_one_is_the_hook() {
        assert_eq!(
            classify(["/path/to/headstate", SUBCOMMAND]),
            Invocation::Hook
        );
    }

    #[test]
    fn no_arguments_is_the_app() {
        assert_eq!(classify(["/path/to/headstate"]), Invocation::App);
    }

    /// macOS hands a GUI-launched app a process-serial-number argument, and
    /// Tauri's own flags could appear too. None of them may turn the app
    /// launch into a hook run -- that failure looks like the app refusing
    /// to start, with a hook record appearing for no reason.
    #[test]
    fn a_gui_launch_argument_is_not_the_hook() {
        assert_eq!(
            classify(["/path/to/headstate", "-psn_0_123456"]),
            Invocation::App
        );
    }

    /// The subcommand is positional. An argument that merely CONTAINS the
    /// name, or one that appears after another argument, is not it -- see
    /// [`classify`] on why searching the vector would be wrong.
    #[test]
    fn the_name_elsewhere_in_the_vector_is_not_the_hook() {
        assert_eq!(
            classify(["/path/to/headstate", "--flag", SUBCOMMAND]),
            Invocation::App
        );
        assert_eq!(
            classify(["/path/to/headstate", "not-claude-hook-either"]),
            Invocation::App
        );
    }

    /// The body of `fn main` in `main.rs`, with comment lines dropped.
    ///
    /// Both halves of this are load-bearing, and both were found by
    /// SABOTAGING the guard below rather than by foresight:
    ///
    /// - **The body, not the file.** A first version searched the whole
    ///   file for `run_hook_and_exit`, which matches the `use` statement on
    ///   line 3 -- before `run()` on any ordering. Reversing the two calls
    ///   in `main` left that guard GREEN. A guard that passes over the
    ///   defect it exists to catch is worse than no guard, because it is
    ///   cited as evidence.
    /// - **Comments dropped.** This codebase documents its rules directly
    ///   above the code, so `run()` and the subcommand name appear in prose
    ///   far more often than in a call. Accepting a doc comment in place of
    ///   code is the trap `invariants.rs` records three v5.14.0 guards
    ///   falling into.
    fn main_body() -> String {
        let main_rs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("main.rs");
        let src = std::fs::read_to_string(&main_rs)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", main_rs.display()));

        let open = src
            .find("fn main()")
            .unwrap_or_else(|| panic!("{} has no `fn main()`", main_rs.display()));

        src[open..]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The guard for the ordering this module's docs call load-bearing.
    ///
    /// Asserted over `main.rs`'s own source because there is no way to
    /// observe it otherwise: both the dispatch and `run()` are calls with
    /// no return value to inspect, and getting the order wrong compiles
    /// cleanly. The failure it prevents is a hook that boots the whole GUI
    /// app -- measured on the shipped binary, which logged a poll tick, a
    /// GitHub query and a listener bind failure before never exiting.
    ///
    /// Proven by sabotage: with the two calls in `main` reversed, this
    /// fails with the message below. See [`main_body`] for the two ways an
    /// earlier version of this guard did NOT fail on that same sabotage.
    #[test]
    fn hook_runs_before_any_app_setup() {
        let body = main_body();

        let dispatch = body
            .find("run_hook_and_exit()")
            .unwrap_or_else(|| panic!("`fn main` no longer calls run_hook_and_exit()"));
        let app = body
            .find("::run()")
            .unwrap_or_else(|| panic!("`fn main` no longer calls the app's run()"));

        assert!(
            dispatch < app,
            "main.rs must dispatch {SUBCOMMAND} BEFORE calling run(): a hook that \
             reaches run() installs a crypto provider, opens the database, starts \
             the poll loop and binds the phone listener, which blows the 1.5s \
             SessionEnd budget and fights the real app for the port"
        );
    }

    /// The guard above can only compare two positions if both calls are
    /// actually there, and `find` returning `None` is a different failure
    /// from the wrong order. This pins that the body it reads is the real
    /// one -- if `main_body` ever returned an empty string (a renamed `fn
    /// main`, a moved file), the ordering guard would panic in its
    /// `unwrap_or_else` rather than pass, but only this test says the
    /// extraction works at all.
    #[test]
    fn the_extracted_main_body_excludes_the_use_statement() {
        let body = main_body();

        assert!(body.contains("run_hook_and_exit()"), "got {body}");
        assert!(body.contains("::run()"), "got {body}");
        assert!(
            !body.contains("use headstate_lib"),
            "the `use` statement must be outside the body the ordering \
             guard reads, or an import satisfies it regardless of order:\n{body}"
        );
    }
}
