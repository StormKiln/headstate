//! The remote allowlist, as the phone knows it.
//!
//! A copy of [`SURFACE`] from the desktop's `src-tauri/src/remote/surface.rs`,
//! so that `remote_call` refuses a command the desktop would refuse
//! BEFORE putting it on the wire, and so the phone knows which commands
//! need the step-up signature. The spec asks for this client-side check
//! so a mistake in the frontend fails locally with a clear message rather
//! than with a 404 from the desktop.
//!
//! Two copies of one table is a drift risk. It is held together by
//! [`tests::table_is_identical_to_the_desktop_table`], which reads the
//! desktop's source file at test time and compares row by row, in
//! order: a class change or an added command on either side fails the
//! mobile tests until the other side is updated.

/// What a command does. Same four classes as the desktop, same meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// No side effects on GitHub or disk.
    Read,
    /// Changes GitHub state or a desktop setting.
    Write,
    /// Deletes files, branches, images, or volumes. Carries the step-up
    /// signature (`stepup.rs`).
    Destructive,
    /// Desktop-only; refused here and there.
    Local,
}

/// Command name to class, in the desktop's order.
pub const SURFACE: &[(&str, Class)] = &[
    // read: no side effects on GitHub or disk.
    // Whether the DESKTOP's background work has died (#1144). Read: a
    // question about state, not an action on that machine. The remedy it
    // points at (`reveal_log`) stays Local, which is why the companion
    // says the log opens at that Mac.
    ("background_panicked", Class::Read),
    // Read: spawns four `--version` probes and returns what they said.
    // A question about the DESKTOP's toolchain, which a phone
    // diagnosing "why are there no worktrees" reasonably asks (#1154).
    ("tool_versions", Class::Read),
    ("get_auth_state", Class::Read),
    ("get_cached", Class::Read),
    ("get_cached_reviewing", Class::Read),
    ("refresh_now", Class::Read),
    ("get_stats", Class::Read),
    ("get_history", Class::Read),
    ("get_periods", Class::Read),
    ("get_cycle_trend", Class::Read),
    ("get_merged_detail", Class::Read),
    // The hardened stats layer (#824). A Read; the desktop's timeout and
    // read-concurrency cap live inside the command, so this inherits both.
    ("stats_count", Class::Read),
    // The stats scope hierarchy (#825). A Read; names only, no statistics.
    ("stats_tree", Class::Read),
    // The Mine/Others per-author board (#826). A Read; the desktop's
    // ceiling, concurrency cap and budget refusal are inside the command.
    ("stats_board", Class::Read),
    // The scoped daily activity series (#826). A Read; count-only.
    ("stats_series", Class::Read),
    // The reviews-GIVEN leaderboard (#826). A Read; one count-only
    // `reviewed-by:<login>` search per member, aliased into one request.
    ("stats_reviewers", Class::Read),
    ("get_reviewing", Class::Read),
    ("count_reviewing", Class::Read),
    ("get_pr_detail", Class::Read),
    ("get_viewer", Class::Read),
    ("build_target", Class::Read),
    ("latest_release", Class::Read),
    ("list_worktrees", Class::Read),
    // The repository browser (#1035, epic #1011). Both Read: one lists a
    // directory level from the DESKTOP's git index, the other reads at
    // most 256 KB of one of its files, and neither writes anything.
    //
    // The sharpest case of the companion's purpose yet, and the same
    // argument `claude_transcript_tail` below makes about itself: the
    // desktop user can `cat` the file and the phone cannot reach the
    // machine at all.
    //
    // Every limit is on the DESKTOP side, inside the command, so this
    // build inherits all three rather than holding a second copy -- the
    // containment guard on the path, the repository root re-derived
    // against the live scan, and the 256 KB window with its truncation
    // stated. See the desktop table for the full argument.
    ("repo_tree", Class::Read),
    ("repo_file", Class::Read),
    ("classify_worktrees", Class::Read),
    // The All Repositories table's Status column, one repository at a
    // time (#1042). A Read: it lists one repository's worktrees and
    // inspects the refs already on disk for the main checkout. It does
    // NOT fetch -- that property is measured (#1026) and is the reason
    // the column qualifies every verdict by ref age instead.
    //
    // Exposed for `classify_worktrees`' reason, one row down: the phone
    // asks the same question the desktop's overview does, and the bound
    // that matters (`CLASSIFY_TIMEOUT`) lives inside the command, so a
    // `remote_call` inherits it rather than needing a second copy.
    ("classify_repo_upstream", Class::Read),
    ("size_worktrees", Class::Read),
    ("list_branches", Class::Read),
    ("scan_artifacts", Class::Read),
    ("size_artifacts", Class::Read),
    ("scan_venvs", Class::Read),
    ("size_venvs", Class::Read),
    ("docker_state", Class::Read),
    ("docker_builds", Class::Read),
    ("docker_images", Class::Read),
    // System Health (#663) and what is using the desktop's processors
    // and memory (#687, #721). All reads, so none carries a step-up
    // signature.
    //
    // `system_footprint` is named for the "what Headstate is costing"
    // panel it once fed; #795 removed that panel and the command kept
    // the name, because this list and the desktop's copy of it match on
    // the literal string and a phone build pinned to an older desktop
    // could not follow a rename.
    ("system_health", Class::Read),
    ("system_health_history", Class::Read),
    // The evaluated health conditions (#789): the rules run on the
    // desktop and this returns verdicts, so the phone holds no copy of
    // any threshold. See the desktop's surface.rs for why that matters.
    ("health_alerts", Class::Read),
    ("system_footprint", Class::Read),
    // Which processes are using the DESKTOP's network (#718). Costs
    // ~5s on the desktop, so the Network page calls it on its own slow
    // cadence and never beside the health poll -- see
    // `health::netproc` on the desktop side.
    ("system_network_processes", Class::Read),
    ("docker_disk_usage", Class::Read),
    ("docker_dangling_volumes", Class::Read),
    ("docker_running_containers", Class::Read),
    ("preview_cleanup", Class::Read),
    ("cleanup_log", Class::Read),
    ("get_cleanup_prefs", Class::Read),
    ("assessed_worktrees", Class::Read),
    // Reads the desktop's disk to summarise a worktree; no side
    // effects, and the phone needs it to decide what to clean up.
    ("assess_worktree", Class::Read),
    // Builds a command STRING and returns it; its own comment in
    // `commands.rs` records that copying deliberately marks
    // nothing. `mark_assessed` is the write, and it is already
    // Write -- so the phone could record an assessment it had no
    // way to obtain.
    ("claudify_command", Class::Read),
    // Local: opens a terminal WINDOW on the desktop, which is the
    // stated test for this class -- the phone cannot use a window that
    // appeared on a Mac it cannot see. `claudify_command` above stays
    // Read because it returns a string the phone CAN act on: it can
    // show it, and the user can copy it.
    ("claude_launch_worktree", Class::Local),
    ("claude_launch_session", Class::Local),
    ("check_packages", Class::Read),
    ("packages_markdown", Class::Read),
    // Read: the effective context a session loads, across scopes
    // (#1131). One more file read than `scan_claude_md`, same class.
    ("claude_md_effective", Class::Read),
    ("scan_claude_md", Class::Read),
    ("read_claude_md", Class::Read),
    // Rescan the DESKTOP's `~/.claude/projects` (#914). Read: it reads
    // local disk without writing to it and populates only Headstate's own
    // cache. The phone has no transcripts of its own, which is exactly
    // why it asks the desktop -- see the desktop table for the full
    // reasoning.
    ("claude_import_transcripts", Class::Read),
    // The DESKTOP's session list with derived liveness (#917). Read: it
    // queries the desktop's cache, lists its `~/.claude/sessions` and
    // probes its process table, writing nothing. "Did the thing I left
    // running on my laptop die?" is the away-from-desk question this
    // companion exists for -- see the desktop table for the full
    // reasoning. The view must say these are the paired desktop's
    // sessions, not this phone's.
    ("claude_sessions", Class::Read),
    // The DESKTOP's detail for ONE of those sessions (#985): its resume
    // command, transcript path and stat, version, start time and run
    // count. Read for the same reasons the list is -- the desktop's own
    // cache plus two stats under the desktop's home, writing nothing.
    //
    // The split exists FOR this transport. The list crosses the pairing
    // link every ten seconds and was carrying every session's detail to
    // render one; it now carries what the list draws, and the phone asks
    // for the rest only when the user opens a session.
    // Read: one indexed query (#1132).
    ("claude_sessions_for_pr", Class::Read),
    ("claude_session_detail", Class::Read),
    ("claude_subagent_rollup", Class::Read),
    // The hook's failure and denial profile for one session, and
    // across all of them (#1062, #1063, #1064). Reads only.
    ("claude_session_events", Class::Read),
    ("claude_event_profile", Class::Read),
    // The two live sources on the DESKTOP: its hook handoff file and its
    // `~/.claude/sessions` registry (#913). The phone has neither of its
    // own -- it runs no Claude sessions -- which is exactly why it asks
    // the desktop. See the desktop table for why this is a Read.
    ("claude_poll_live", Class::Read),
    // The DESKTOP's Claude Code overview aggregates (#921). Read: two
    // SELECTs over Headstate's own cache plus a directory listing, and it
    // writes nothing at all -- not even the cache, unlike the rescan above.
    //
    // The strongest away-from-desk case in this whole group: the page's
    // headline figure is how many sessions are resumable, which is
    // "how much work is waiting for me back at the laptop". See the
    // desktop copy for the full argument.
    ("claude_overview", Class::Read),
    // The DESKTOP's installed plugins and their measured usage (#1075).
    //
    // `Read`, for the desktop copy's reasons. Present here because the
    // companion renders the same `src/` frontend, so the page exists on
    // the phone and would otherwise be a panel that could never load.
    //
    // Worth stating what the phone is reading: this is an inventory of
    // the DESKTOP's plugins, counted from the DESKTOP's transcripts.
    // "Which of my plugins am I actually getting value from?" is a
    // question worth answering from a sofa, and the answer leads to an
    // action taken later, at the machine.
    // Read: a directory walk over ~/.claude with no side effects
    // (#1129).
    ("claude_definitions", Class::Read),
    ("claude_plugins", Class::Read),
    // The DESKTOP's restart list (#1071): every session running there,
    // with the line that would bring each one back. Read -- a fold of
    // `claude_sessions`, writing nothing and spawning nothing.
    //
    // Present because the class decides REACHABILITY, not whether a
    // button is drawn. The companion renders the same `src/` frontend,
    // and that frontend hides the export control on a phone (there is no
    // terminal to paste into), so this is the inventory a companion user
    // can read rather than an action they can take. Refusing it here
    // would make the aggregate unreachable while `claude_session_detail`
    // already hands over the same command one row at a time.
    ("claude_restart_list", Class::Read),
    // How much work happened inside one of the DESKTOP's sessions (#959).
    // Read: one bounded read of one `.jsonl` under the desktop's
    // `~/.claude/projects`, writing nothing.
    //
    // The phone gets this for the same reason the desktop does -- "was
    // that the long session or the typo" is how the row worth resuming is
    // picked -- and the response is a handful of integers, so it costs
    // the transport nothing. See the desktop copy for the 8 MB read
    // budget and why it is reported rather than silent.
    // Read: one aggregate query over stored rows (#1134).
    ("claude_usage_profile", Class::Read),
    ("claude_session_usage", Class::Read),
    // The tail of one of the DESKTOP's transcripts, as conversation
    // (#982). Read: one bounded tail read, writing nothing.
    //
    // The one Claude action where the phone's case is STRONGER than the
    // desktop's. `claude_reveal_path` is `Local` and absent from this
    // table, so until now a companion user could see that a session died
    // and could not see a word of what it was doing -- the desktop user
    // can `cat` the file, and the phone cannot reach the machine.
    //
    // Bounded inside the command (256 KB, 200 messages, clamped blocks)
    // so asking for the desktop's 76 MB transcript cannot hand the phone
    // 76 MB. See the desktop copy for the path guard both commands share.
    ("claude_transcript_tail", Class::Read),
    // Whether the DESKTOP's hooks are installed (#915). Read: one file
    // read, no side effects, and "is that desktop recording?" is a real
    // away-from-desk question.
    //
    // The three commands that CHANGE that file are `Local` and therefore
    // absent from this table entirely -- see the desktop copy for why
    // editing another tool's config is a thing the phone must not do.
    // Reading is the half that stays useful without being able to act.
    //
    // The answer is three-state, and the phone must render "cannot tell"
    // as its own thing rather than as "not installed": a settings file
    // Claude Code cannot parse is ignored silently, so a desktop in that
    // state has every hook dead, and the remedy is at that keyboard.
    // Read: one file, no side effects, and "what is wired into my
    // sessions" is a reasonable thing to ask from a phone (#1127).
    ("claude_hooks_inventory", Class::Read),
    // Read: three file reads, no side effects (#1130).
    ("claude_effective_settings", Class::Read),
    ("claude_hooks_status", Class::Read),
    ("get_poll_interval", Class::Read),
    ("get_worktree_dirs", Class::Read),
    ("get_ui_prefs", Class::Read),
    // How a run is going, or how it ended. The resume path: a
    // suspended phone holds no event stream, so it asks instead.
    ("update_run_state", Class::Read),
    // How the Update All run is going, or how it ended (#1016). The same
    // resume path, and Read for the same reason: one in-memory registry,
    // read, nothing written.
    ("update_all_state", Class::Read),
    // write: changes GitHub state through the existing write module, or
    // a desktop setting.
    ("act_on_pr", Class::Write),
    ("act_on_prs", Class::Write),
    ("review_pr", Class::Write),
    ("comment_on_pr", Class::Write),
    ("resolve_thread", Class::Write),
    ("unresolve_thread", Class::Write),
    ("reply_to_thread", Class::Write),
    ("rerun_checks", Class::Write),
    ("update_pr_branch", Class::Write),
    ("set_auto_merge", Class::Write),
    ("mark_assessed", Class::Write),
    ("clear_assessed", Class::Write),
    ("set_cleanup_prefs", Class::Write),
    ("set_poll_interval", Class::Write),
    // `open_update_pr` was a `Write` row here, mirroring the desktop's,
    // and both are gone (#964). Removed in the SAME change, which is
    // what `table_is_identical_to_the_desktop_table` enforces: it parses
    // the desktop's source at test time, so a row left here after the
    // desktop dropped one fails the mobile tests until this copy catches
    // up.
    //
    // What the phone loses is a command it could dispatch and the desktop
    // could not: #626 superseded the two-phase flow. Updates are driven
    // through `apply_updates_in_background`, which is still classed and
    // still reachable from here.
    // Driving the desktop IS the companion, so these are Write
    // rather than Local: pulling a checkout, starting the desktop's
    // Docker and restarting it are the things a person opens the
    // phone to do. They change the desktop but delete nothing, so
    // they do not carry the step-up signature.
    ("pull_checkout", Class::Write),
    // Fast-forward every repository in the scan roots (#1019). Write, and
    // the line is the one this table draws everywhere: a `--ff-only` pull
    // DELETES nothing -- no merge commit created, no commit discarded,
    // and on refusal the working tree is byte-identical -- so it carries
    // no step-up signature. Not Local either: the refusal set is
    // identical whatever the caller, because a phone's `remote_call`
    // routes through the same code and inherits every per-repository
    // refusal. See the desktop table for the full argument.
    //
    // Exposed only BECAUSE the two rows around it exist: the run can be
    // stopped and its outcome read back after a suspension, which is
    // `apply_updates_in_background`'s own stated condition. MEASURED
    // worst case 22 minutes, and a suspended phone holds no event stream.
    ("update_all_repositories", Class::Write),
    ("cancel_update_all", Class::Write),
    // Refreshing one repository's remote refs (#788). Write, not Read:
    // `git fetch` mutates nothing on GitHub but it writes `origin/*`,
    // `FETCH_HEAD` and new objects to disk, and Read here means neither.
    // Not Destructive -- no `--prune`, so it deletes no ref -- so it
    // carries no step-up signature. See the desktop table for the full
    // argument.
    ("fetch_refs", Class::Write),
    ("docker_start", Class::Write),
    ("docker_restart", Class::Write),
    // Preferences, not machine capabilities: they live in the
    // desktop's SQLite beside `cleanup_prefs` (already Read/Write),
    // and a phone that could not read them fell back to the
    // hardcoded defaults for every `?? value` in the frontend --
    // silently ignoring hidden_views and forcing announce_updates on.
    ("set_ui_prefs", Class::Write),
    // A hint to the desktop's poll loop about what this client
    // needs, not an action on the desktop's machine. Left Local, the
    // loop never learned a phone had stopped needing GitHub data and
    // the cadence optimisation was dead for every remote client.
    ("set_view_needs_github", Class::Write),
    // Starts a long-running task on the desktop. Write rather than
    // Destructive: it creates a worktree and edits manifests in it,
    // deleting nothing, and the pull request it opens is reviewable
    // before anything lands. Only drivable from a phone now that it
    // can be STOPPED and its outcome read back after a suspension
    // (#626) -- starting something you cannot stop or see the end of
    // is not a feature.
    ("apply_updates_in_background", Class::Write),
    ("cancel_update_run", Class::Write),
    // Clears a worktree's lock (#775). Write, not Destructive: nothing
    // is deleted and `git worktree lock` puts it back. It removes a
    // guard, so the warning belongs in the confirmation that names the
    // holder and the age -- not in a step-up prompt about an
    // unrecoverable action this is not. Removal is unaffected: its own
    // gate re-classifies the worktree from scratch afterwards.
    ("unlock_worktree", Class::Write),
    // Clears a repository's stale worktree registrations (#793). Write,
    // not Destructive, and strictly less destructive than the unlock
    // above: `git worktree prune` removes entries under `.git/worktrees/`
    // whose directory git has ALREADY reported gone, so no file leaves
    // the disk, no branch is touched, and no commit becomes unreachable.
    // Not Destructive despite reading like `docker_prune_cache`, which
    // is: that deletes build cache a later build would reuse, this
    // deletes a dangling pointer. Spending the step-up prompt on
    // bookkeeping is how it stops being read on the removals that matter.
    ("prune_worktrees", Class::Write),
    // destructive: deletes files, branches, images, or volumes.
    ("delete_head_branch", Class::Destructive),
    ("delete_branches", Class::Destructive),
    ("delete_remote_branches", Class::Destructive),
    ("remove_worktree", Class::Destructive),
    ("remove_worktrees", Class::Destructive),
    ("remove_worktree_forced", Class::Destructive),
    ("remove_artifacts", Class::Destructive),
    ("remove_venvs", Class::Destructive),
    ("remove_orphan", Class::Destructive),
    ("docker_remove_images", Class::Destructive),
    ("docker_remove_volume", Class::Destructive),
    ("docker_prune_cache", Class::Destructive),
    // `apply_package_updates` was the matching `Destructive` row and is
    // gone with the desktop's (#964). It was the one entry in this table
    // that let a phone run package managers in WRITE mode down a path no
    // desktop control could reach, which is why it is worth naming here
    // rather than merely deleting: the phone is now strictly narrower
    // than the desktop on this flow, not wider.
    // local: not exposed remotely.
    ("diag_log", Class::Local),
    ("reveal_log", Class::Local),
    // Reveals a session's directory or transcript in the DESKTOP's file
    // manager (#917). Local: this phone cannot see that Finder, which is
    // the stated test for the class.
    ("claude_reveal_path", Class::Local),
    ("get_autostart", Class::Local),
    ("set_autostart", Class::Local),
    ("get_notify_prefs", Class::Local),
    ("set_notify_prefs", Class::Local),
    ("set_worktree_dirs", Class::Local),
    // The remote feature's own commands. Pairing and the on/off switch
    // are decisions the desktop's user makes at the desktop: a phone
    // that could approve its own pairing request, revoke a rival, or
    // turn the listener off would defeat the point of each.
    ("issue_pairing_token", Class::Local),
    ("respond_to_pairing", Class::Local),
    ("list_paired_devices", Class::Local),
    ("revoke_paired_device", Class::Local),
    ("get_remote_enabled", Class::Local),
    ("set_remote_enabled", Class::Local),
    // The Claude Code hook installer (#915). All three `Local`: they edit
    // `~/.claude/settings.json`, a config file shared with other tools, and
    // the refusal cases need a human reading an explanation at the machine
    // with the broken file. See the desktop table for the full argument.
    //
    // Present in this copy even though the phone can never run them --
    // `table_is_identical_to_the_desktop_table` requires the table to be
    // VERBATIM, and that is the point: the phone knows a command exists and
    // is refused, which is a different answer from a command it has never
    // heard of. `claude_hooks_status` above is the `Read` half and is
    // genuinely callable.
    ("claude_install_hooks", Class::Local),
    ("claude_reinstall_hooks", Class::Local),
    ("claude_uninstall_hooks", Class::Local),
];

/// The class of a command, or `None` when the desktop has no such
/// command.
pub fn class_of(command: &str) -> Option<Class> {
    SURFACE
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, class)| *class)
}

/// Why `remote_call` did not put a command on the wire. The messages are
/// what the frontend sees as the rejection reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("`{0}` is not a Headstate command")]
    Unknown(String),
    #[error("`{0}` is only available on the desktop")]
    Local(String),
}

/// Known and not local, or the refusal naming why.
pub fn admit(command: &str) -> Result<Class, Refusal> {
    match class_of(command) {
        None => Err(Refusal::Unknown(command.to_string())),
        Some(Class::Local) => Err(Refusal::Local(command.to_string())),
        Some(class) => Ok(class),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop's table, parsed from its source. `include_str!` ties
    /// this test to the desktop file at compile time, so the two are
    /// compared as they are checked in, not as someone remembers them.
    fn desktop_table() -> Vec<(String, String)> {
        let src = include_str!("../../src-tauri/src/remote/surface.rs");
        let start = src
            .find("pub const SURFACE")
            .expect("desktop surface.rs must define SURFACE");
        let body = &src[start..];
        let end = body.find("];").expect("SURFACE must close");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line.strip_prefix("(\"")?;
                let (name, rest) = rest.split_once("\", Class::")?;
                let class = rest.trim_end_matches("),");
                Some((name.to_string(), class.to_string()))
            })
            .collect()
    }

    fn class_name(class: Class) -> &'static str {
        match class {
            Class::Read => "Read",
            Class::Write => "Write",
            Class::Destructive => "Destructive",
            Class::Local => "Local",
        }
    }

    #[test]
    fn table_is_identical_to_the_desktop_table() {
        let desktop = desktop_table();
        assert!(
            desktop.len() > 50,
            "parsed only {} rows from the desktop's surface.rs; the parser is broken",
            desktop.len()
        );
        let mobile: Vec<(String, String)> = SURFACE
            .iter()
            .map(|(name, class)| (name.to_string(), class_name(*class).to_string()))
            .collect();
        assert_eq!(
            mobile, desktop,
            "src-mobile/src/surface.rs SURFACE differs from src-tauri/src/remote/surface.rs; \
             copy the desktop's table verbatim"
        );
    }

    #[test]
    fn class_of_reports_each_class_and_nothing_for_unknown_names() {
        assert_eq!(class_of("get_cached"), Some(Class::Read));
        assert_eq!(class_of("act_on_pr"), Some(Class::Write));
        assert_eq!(class_of("remove_worktree"), Some(Class::Destructive));
        assert_eq!(class_of("reveal_log"), Some(Class::Local));
        assert_eq!(class_of("drop_database"), None);
    }

    #[test]
    fn local_and_unknown_commands_are_refused_with_the_desktop_wording() {
        assert_eq!(
            admit("reveal_log"),
            Err(Refusal::Local("reveal_log".into()))
        );
        assert_eq!(
            admit("reveal_log").unwrap_err().to_string(),
            "`reveal_log` is only available on the desktop"
        );
        assert_eq!(
            admit("getCached"),
            Err(Refusal::Unknown("getCached".into()))
        );
        assert_eq!(admit("get_cached"), Ok(Class::Read));
        assert_eq!(admit("remove_worktree"), Ok(Class::Destructive));
    }
}
