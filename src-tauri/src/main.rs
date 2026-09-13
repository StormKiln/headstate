#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use headstate_lib::claude::cli::{classify, run_hook_and_exit, Invocation};

fn main() {
    // FIRST, before anything else in this process. The `claude-hook`
    // subcommand (#912) is what Claude Code runs on `SessionStart` and
    // `SessionEnd`, inside a 1.5 second budget shared by every hook
    // (anthropics/claude-code#41577). `run()` below installs a crypto
    // provider, builds a Tauri app, opens the database, starts the poll
    // loop and binds the phone listener -- measured, by running the
    // shipped binary with an unrecognised argument: it logged a poll tick,
    // a GitHub query and a listener bind failure, and never exited.
    //
    // So this dispatch has to come before `run()`, and the ordering is
    // asserted by `claude::cli`'s `hook_runs_before_any_app_setup` --
    // reversing these two lines compiles cleanly and would boot a GUI app
    // twice per Claude session.
    if classify(std::env::args().collect::<Vec<_>>()) == Invocation::Hook {
        run_hook_and_exit();
    }

    headstate_lib::run()
}
