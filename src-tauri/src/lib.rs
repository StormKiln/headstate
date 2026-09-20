pub mod artifacts;
pub mod auth;
/// Whether the background loops are still working (#1145).
pub mod background;
pub mod branches;
pub mod caches;
pub mod claude;
pub mod claudemd;
pub mod cleanup;
pub mod commands;
pub mod diag;
pub mod docker;
pub mod github;
pub mod health;
/// Rules stated elsewhere in this codebase, asserted over its own source
/// (#854). Test-only: the module holds no shipped code, and is declared
/// here so `cargo test` compiles it.
#[cfg(test)]
mod invariants;
pub mod packages;
pub mod panic_hook;
pub mod poll;
pub mod redact;
pub mod remote;
pub mod repos;
pub mod store;
pub mod tools;
pub mod tray;
mod worktrees;

use commands::{AuthState, GhClient};
use github::client::GitHubClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Manager;

/// Whether the main window is currently focused. Shared with `poll::spawn`
/// so the background loop can pick FOCUSED vs BACKGROUND cadence; managed as
/// Tauri state so the window-event handler below (the only place focus
/// actually changes) can reach the same `Arc` and flip it.
struct Focused(Arc<AtomicBool>);

/// Record that the window was hidden (close-to-tray or otherwise): the poll
/// loop should drop to the background cadence. A window can be hidden
/// without the platform ever sending a `WindowEvent::Focused(false)`, so
/// close-to-tray has to clear the flag itself rather than relying on a
/// focus event to follow.
fn mark_hidden(focused: &AtomicBool) {
    focused.store(false, Ordering::Relaxed);
}

/// Record a real focus change: focused -> FOCUSED cadence, blurred ->
/// BACKGROUND cadence.
fn mark_focus(focused: &AtomicBool, is_focused: bool) {
    focused.store(is_focused, Ordering::Relaxed);
}

/// Show one battery alert (#720).
///
/// A sibling of `poll::notify_breakage` rather than a call into it:
/// that one takes a `Breakage`, which is a pull request, and widening
/// it to mean "or a battery" would make a type that describes two
/// unrelated things. What IS shared is the part that must not drift --
/// `poll::notification_allowed`, the ask-once permission gate.
///
/// Failure is logged and swallowed, exactly as it is there: a
/// notification is an affordance, and losing one must never take down
/// the sampler that fills the 24-hour series.
fn notify_battery(app: &tauri::AppHandle, alert: &health::alerts::Alert) {
    use tauri_plugin_notification::NotificationExt;

    if !poll::notification_allowed(app) {
        return;
    }
    if let Err(e) = app
        .notification()
        .builder()
        .title(alert.title())
        .body(alert.body())
        .show()
    {
        log::warn!("failed to show a battery notification: {e}");
    }
}

/// Show one CPU runaway alert (#791).
///
/// A sibling of `notify_battery` for the same reason that one is a
/// sibling of `poll::notify_breakage`: the three take three unrelated
/// types, and one function widened to accept "a battery or a pull
/// request or a process" would describe nothing. What IS shared is the
/// part that must not drift -- `poll::notification_allowed`, the
/// ask-once permission gate.
///
/// Only `health::runaway::Alert` reaches here, which is the one tier
/// that ships. `runaway::Shadow` has no path to this function by
/// design; see that module's docs on shadow mode.
fn notify_runaway(app: &tauri::AppHandle, alert: &health::runaway::Alert) {
    use tauri_plugin_notification::NotificationExt;

    if !poll::notification_allowed(app) {
        return;
    }
    if let Err(e) = app
        .notification()
        .builder()
        .title(alert.title())
        .body(alert.body())
        .show()
    {
        log::warn!("failed to show a CPU notification: {e}");
    }
}

/// Show one "a Claude Code session died" alert (#979).
///
/// A sibling of `notify_battery` and `notify_runaway` for the reason
/// those are siblings of `poll::notify_breakage`: the four take four
/// unrelated types, and one function widened to accept "a battery or a
/// pull request or a process or a session" would describe nothing. What
/// IS shared is the part that must not drift --
/// `poll::notification_allowed`, the ask-once permission gate.
///
/// Failure is logged and swallowed, exactly as it is there: a
/// notification is an affordance, and losing one must never take down the
/// sweep that found the crash.
///
/// # The body carries the resume handle's directory, not the handle
///
/// A notification body cannot be copied from, so the session id would be
/// a UUID the user has to retype. The DIRECTORY is what lets them find
/// the row -- it is one of the four fields the session list searches --
/// and the app already generates the `claude --resume` command on that
/// row. The alert's job is to get the user to the app, not to be the app.
fn notify_claude_crash(app: &tauri::AppHandle, crashed: &crate::claude::crash::Crashed) {
    use tauri_plugin_notification::NotificationExt;

    if !poll::notification_allowed(app) {
        return;
    }
    // The registry's `name`, or the id. NOT a fabricated name: a
    // generated title cannot be told from a real one, which is the rule
    // `transcript.rs` states about the two titleless sessions, and a raw
    // UUID at least reads as an identifier rather than as a description.
    let title = crashed.name.clone().unwrap_or_else(|| {
        format!(
            "Claude session {}",
            // The first segment of the UUID, which is what the session
            // list's own rows show and what a user recognises. The whole
            // thing would fill a notification title with hex.
            crashed
                .session_id
                .split('-')
                .next()
                .unwrap_or(&crashed.session_id)
        )
    });
    let body = match &crashed.cwd {
        Some(cwd) => format!("Stopped without ending cleanly, in {cwd}"),
        // "Where" is genuinely unknown rather than suppressed: a registry
        // record without a cwd is a real case, and a body that simply
        // omitted the clause would read as if there were nowhere to go.
        None => "Stopped without ending cleanly. No directory was recorded for it.".to_owned(),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("failed to show a Claude session notification: {e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything builds a TLS config. Two rustls providers are
    // compiled in (see Cargo.toml), and without an installed default the
    // first `ClientConfig::builder()` in any dependency panics.
    remote::gate::install_crypto_provider();

    // Before anything spawns. A panic in a background task otherwise
    // kills that task silently -- the poll loop stops, the badge freezes
    // on its last value, and the window still paints (#1144).
    panic_hook::install();

    tauri::Builder::default()
        // A GUI-launched .app has no stderr, so every eprintln! in this
        // codebase went nowhere a user could reach. "It stopped updating"
        // was uninvestigable: no log file to ask for, and no way to know
        // which of the failure paths fired.
        //
        // Writes to the OS log directory (Console.app on macOS) and a
        // rotating file beside it. Never log the token, and never log a
        // repository owner -- see CONTRIBUTING and check-privacy.sh.
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // No launch args: the app already opens hidden-to-tray on its
        // own terms, and passing --hidden here would be a second, easily
        // divergent source of truth for that behaviour.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // KEEP the previous file. The default discards it on
                // startup, which destroyed a real diagnostic recording
                // mid-analysis: 305 lines became 1 the moment the app
                // relaunched.
                //
                // That is not a corner case for this feature. The
                // workflow is "turn the log on, reproduce the problem,
                // send the file" -- and reproducing a hang or a slow
                // start is exactly what makes someone quit and relaunch,
                // destroying the evidence of the thing they were
                // capturing.
                //
                // Bounded, because this app is a disk-cleanup tool and
                // must not become the thing filling the disk.
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .max_file_size(8 * 1024 * 1024)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("headstate".into()),
                    }),
                ])
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::diag_log,
            commands::background_panicked,
            commands::background_health,
            commands::tool_versions,
            commands::read_log_tail,
            commands::reveal_log,
            commands::pull_checkout,
            commands::fetch_refs,
            commands::remove_orphan,
            commands::get_cached,
            commands::get_cached_reviewing,
            commands::refresh_now,
            commands::get_stats,
            commands::get_history,
            commands::get_periods,
            commands::get_reviewing,
            commands::count_reviewing,
            commands::get_pr_detail,
            commands::act_on_pr,
            commands::build_target,
            commands::get_viewer,
            commands::rerun_checks,
            commands::get_ui_prefs,
            commands::set_ui_prefs,
            commands::get_autostart,
            commands::set_autostart,
            commands::get_notify_prefs,
            commands::set_notify_prefs,
            commands::review_pr,
            commands::comment_on_pr,
            commands::scan_artifacts,
            commands::read_cached_scan,
            commands::remove_artifacts,
            commands::size_artifacts,
            commands::mark_assessed,
            commands::clear_assessed,
            commands::preview_cleanup,
            commands::cleanup_log,
            commands::get_cleanup_prefs,
            commands::set_cleanup_prefs,
            // apply_package_updates and open_update_pr were registered
            // here and are gone (#964). #626 replaced the two-phase flow
            // with apply_updates_in_background below, which does both
            // halves in one background task and reaches the helpers
            // directly, so the two command wrappers had no desktop caller
            // while staying remotely dispatchable as Destructive and
            // Write. The helpers themselves are untouched; see
            // commands.rs where each wrapper stood.
            //
            // NOTE for whoever edits this comment: the parser behind
            // every_registered_command_has_exactly_one_class reads this
            // block by finding the first closing square bracket, so a
            // comment in here must not contain one -- an attribute
            // written out in backticks truncates the command list and
            // fails that test with "the parser is broken".
            commands::apply_updates_in_background,
            commands::cancel_update_run,
            commands::update_run_state,
            commands::claude_md_effective,
            commands::scan_claude_md,
            commands::read_claude_md,
            commands::claude_import_transcripts,
            commands::claude_sessions,
            commands::claude_sessions_for_pr,
            commands::claude_session_detail,
            commands::claude_subagent_rollup,
            commands::claude_session_events,
            commands::claude_event_profile,
            commands::claude_reveal_path,
            commands::claude_usage_profile,
            commands::claude_session_usage,
            commands::claude_transcript_tail,
            commands::claude_poll_live,
            commands::claude_overview,
            commands::claude_definitions,
            commands::claude_plugins,
            commands::claude_restart_list,
            commands::claude_hooks_inventory,
            commands::claude_effective_settings,
            commands::claude_config_health,
            commands::claude_hooks_status,
            commands::claude_install_hooks,
            commands::claude_reinstall_hooks,
            commands::claude_uninstall_hooks,
            commands::check_packages,
            commands::packages_markdown,
            commands::scan_venvs,
            commands::size_venvs,
            commands::remove_venvs,
            commands::resolve_thread,
            commands::unresolve_thread,
            commands::reply_to_thread,
            commands::update_pr_branch,
            commands::set_auto_merge,
            commands::delete_head_branch,
            commands::latest_release,
            commands::act_on_prs,
            commands::get_poll_interval,
            commands::set_poll_interval,
            commands::get_worktree_dirs,
            commands::set_worktree_dirs,
            commands::list_worktrees,
            commands::repo_tree,
            commands::repo_file,
            commands::update_all_repositories,
            commands::cancel_update_all,
            commands::update_all_state,
            commands::classify_worktrees,
            commands::classify_repo_upstream,
            commands::list_branches,
            commands::system_health,
            commands::system_health_history,
            commands::health_alerts,
            commands::system_footprint,
            commands::system_network_processes,
            commands::delete_branches,
            commands::delete_remote_branches,
            commands::remove_worktree,
            commands::remove_worktrees,
            commands::docker_state,
            commands::docker_builds,
            commands::docker_images,
            commands::docker_disk_usage,
            commands::docker_remove_images,
            commands::docker_dangling_volumes,
            commands::docker_remove_volume,
            commands::docker_prune_cache,
            commands::docker_running_containers,
            commands::docker_restart,
            commands::docker_start,
            commands::assess_worktree,
            commands::claudify_command,
            commands::claude_launch_worktree,
            commands::claude_launch_session,
            commands::claude_propose_stop,
            commands::claude_stop_session,
            commands::claude_launch_worktree_preview,
            commands::claude_launch_session_preview,
            commands::claude_launch_terms,
            commands::assessed_worktrees,
            commands::remove_worktree_forced,
            commands::unlock_worktree,
            commands::prune_worktrees,
            commands::size_worktrees,
            commands::set_view_needs_github,
            commands::get_cycle_trend,
            commands::get_merged_detail,
            commands::stats_count,
            commands::stats_tree,
            commands::stats_board,
            commands::stats_series,
            commands::stats_reviewers,
            commands::get_auth_state,
            remote::pairing::issue_pairing_token,
            remote::pairing::respond_to_pairing,
            remote::pairing::list_paired_devices,
            remote::pairing::revoke_paired_device,
            remote::gate::get_remote_enabled,
            remote::gate::set_remote_enabled,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Before anything that logs. The stored preference decides
            // whether the verbose `[diag]` lines are written, and
            // reading it first means a user who left diagnostics on
            // captures the startup sequence too -- which is where the
            // v3.5.3 log proved most useful.
            crate::diag::set_enabled(crate::commands::read_ui_prefs(&handle).diagnostic_logging);

            // `read_token` shells out via `std::process::Command`, which
            // blocks -- fine here, because `setup` is a plain synchronous
            // closure and is not occupying a tokio worker.
            //
            // `build_client` is different and must run inside the runtime.
            // Octocrab builds a hyper/tower stack whose Buffer layer calls
            // `tokio::spawn` during construction, so building it outside a
            // reactor panics with "there is no reactor running" -- crashing
            // the app on launch for every user who IS authenticated, which
            // no unit test catches because tests always construct it from an
            // async context. `block_on` enters Tauri's own runtime for the
            // duration of the call.
            let (auth_state, gh_client) = match auth::read_token() {
                Ok(token) => {
                    match tauri::async_runtime::block_on(async { auth::build_client(&token) }) {
                        Ok(octocrab) => {
                            let client = Arc::new(GitHubClient::new(octocrab));
                            (
                                AuthState {
                                    ok: true,
                                    message: String::new(),
                                },
                                Some(client),
                            )
                        }
                        Err(e) => (
                            AuthState {
                                ok: false,
                                message: e.to_string(),
                            },
                            None,
                        ),
                    }
                }
                // `AuthError`'s Display messages (including
                // `GhNotLoggedIn`'s, which comes verbatim from `gh`'s own
                // stderr) are already display-ready prose for a first-run
                // screen -- never re-wrapped, never containing a token.
                Err(e) => (
                    AuthState {
                        ok: false,
                        message: e.to_string(),
                    },
                    None,
                ),
            };

            app.manage(auth_state);
            app.manage(GhClient(gh_client.clone()));

            // Only poll GitHub if we actually have a client; there is
            // nothing to fetch without one, and this task is the only
            // caller of GitHub in the whole app.
            // Managed unconditionally: the tray menu is built whether or
            // not auth succeeded, and its handler must always find a Waker
            // to signal even when nothing is listening for it.
            let waker = Arc::new(tokio::sync::Notify::new());
            app.manage(poll::Waker(waker.clone()));

            // Managed unconditionally, like the Waker: the settings command
            // must find it whether or not auth succeeded.
            // Restore the saved interval, falling back to the default.
            // Read here rather than lazily so the FIRST tick already uses
            // the user's choice instead of polling fast once and then
            // settling down.
            let saved = store::open_db(&commands::db_path(&handle))
                .ok()
                .and_then(|c| {
                    store::settings::get::<u64>(&c, store::settings::keys::POLL_INTERVAL_SECS).ok()
                })
                .flatten()
                .map(poll::clamp_interval)
                .unwrap_or(poll::DEFAULT_FOCUSED_SECS);
            let interval = Arc::new(std::sync::atomic::AtomicU64::new(saved));
            app.manage(poll::PollInterval(interval.clone()));

            // Starts true: the app opens on a PR view.
            let needs_gh = Arc::new(AtomicBool::new(true));
            app.manage(poll::ViewNeedsGithub(needs_gh.clone()));
            // Which repositories have a background update run going,
            // and how the last one ended. Default-constructed: it is
            // empty until someone starts a run.
            app.manage(packages::runs::UpdateRuns::default());
            // Whether an Update All run is going, how far it has got, and
            // how the last one ended (#1016). ONE slot, not a map: the
            // run covers the whole scan root, so a per-repository claim
            // would let two runs interleave across 45 repositories and
            // make both reports wrong.
            app.manage(repos::runs::UpdateAllRuns::default());

            // The system-health reader, and the sampler that fills the
            // 24-hour series behind the System Health view (#663).
            //
            // ONE collector for the process: sysinfo reports CPU use
            // since the last refresh, so a fresh instance per call would
            // report an idle machine forever.
            let collector = Arc::new(health::collect::Collector::default());
            app.manage(collector.clone());

            // The process-table reader behind the System Health CPU and
            // Memory detail pages (#687, #721): what is using this
            // machine, as a bounded top-N. Its own reader rather than a
            // field on `Collector`, for the same reason it is a separate
            // command -- the machine sample and the process sample
            // refresh different kernel state on different schedules, and
            // one mutex across both would make each wait on the other's
            // read for nothing.
            //
            // Still spelled `footprint` because the command and the wire
            // type are: it began as #665's "what Headstate is costing"
            // panel, which #795 removed. See `commands::system_footprint`
            // for why renaming it would be a remote-surface break.
            //
            // Not on the once-a-minute sampler and not written to
            // SQLite: this is a live reading the view asks for, and
            // there is no 24-hour series of it to keep.
            app.manage(Arc::new(health::footprint::Footprints::default()));
            // #865's watch notices, written by the poll loop below and
            // read by `health_alerts`. One accumulator, not two.
            app.manage(Arc::new(health::runaway::Watched::default()));
            {
                let app_handle = app.handle().clone();
                // Blocking, not async: it reads the kernel and writes
                // SQLite, and both belong off the async workers.
                std::thread::spawn(move || {
                    // The first sample is discarded: sysinfo needs two
                    // refreshes before CPU use means anything, so
                    // recording the first would store a zero that reads
                    // as an idle machine.
                    let _ = collector.sample(&chrono::Utc::now().to_rfc3339());
                    // What the battery alerts have already announced
                    // (#720). In memory rather than in SQLite, matching
                    // `poll`'s `previous`: a relaunch re-arming every
                    // condition is correct, because the user has just
                    // opened the app and a standing alert is worth one
                    // restatement, not a permanent silence.
                    let mut fired = health::alerts::Fired::default();
                    // The CPU runaway rules (#791). Three pieces, all
                    // in memory and none of them a schema change:
                    //
                    // - `table` reads the process table for the
                    //   aggregate rule's "no single process explains
                    //   it" clause and for the shadow log. Its own
                    //   `sysinfo::System`, held across ticks for the
                    //   same reason `collector` is: CPU use is a delta
                    //   since the previous refresh of the SAME
                    //   instance, so a fresh one would report an idle
                    //   machine forever.
                    // - `watcher` approximates per-process duration for
                    //   the shadow log only. #791's persisted
                    //   `(pid, start_time)` tracking is deferred with
                    //   the tiers that would notify from it -- a
                    //   forgotten accumulation is one missing log line,
                    //   and that does not justify a migration.
                    // - `last_pass` is what makes the duration honest:
                    //   the watcher is told how long it has been since
                    //   the previous pass and refuses to credit a span
                    //   wider than `GAP_MS`. Across a closed lid that
                    //   span is hours, and crediting it would hand
                    //   every hot process half a day of "sustained"
                    //   burn the instant the app reopens.
                    //
                    // The first pass is discarded along with the first
                    // `collector.sample` above, and for the same
                    // reason: sysinfo needs two refreshes before CPU
                    // use means anything.
                    let table = health::runaway::Table::new();
                    let _ = table.read();
                    let mut watcher = health::runaway::Watcher::default();
                    let mut last_pass = std::time::Instant::now();
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                        let sample = collector.sample(&chrono::Utc::now().to_rfc3339());

                        // Read and logged BEFORE the database work, and
                        // unconditionally. Both halves matter: the
                        // process table's CPU figures are a delta since
                        // the previous refresh, so skipping a tick when
                        // SQLite is unhappy would make the next
                        // reading an average over two minutes rather
                        // than one -- and the shadow log is the point
                        // of this release, so a failed metric write
                        // must not be what silences it.
                        let (observations, aggregate) = table.read();
                        let elapsed = last_pass.elapsed().as_millis();
                        last_pass = std::time::Instant::now();

                        // SHADOW ONLY. These lines are the distribution
                        // #791 asks for before tiers 1 and 2 are
                        // allowed to interrupt anyone; nothing here
                        // notifies, and `Shadow` has no path to a
                        // notification even by accident.
                        let minutes = watcher
                            .observe(&observations, i64::try_from(elapsed).unwrap_or(i64::MAX));
                        for would in health::runaway::shadow(&observations, minutes) {
                            log::info!("{}", would.line());
                        }

                        // #865's watch tier. NOT a notification: these
                        // reach `health_alerts` and the System Health
                        // page only, which is what the user asked for --
                        // "at least have an indicator in the UI". The
                        // `Alert` path above is what interrupts someone,
                        // and `nothing_converts_a_shadow_into_an_alert`
                        // asserts that staying deliberate.
                        //
                        // Written here because `minutes` only exists
                        // here: duration is accumulated across passes by
                        // the `watcher` above, and a command with a
                        // fresh `Table` would see every process at 0.0.
                        app_handle
                            .state::<Arc<health::runaway::Watched>>()
                            .set(health::runaway::watch(&observations, minutes));
                        // A failed sample is logged and skipped, never
                        // fatal: a gap in the chart is a far better
                        // outcome than an app that stops because it
                        // could not write a metric.
                        match store::open_db(&commands::db_path(&app_handle))
                            .map_err(|e| e.to_string())
                            .and_then(|c| {
                                store::health::record(&c, &sample).map_err(|e| e.to_string())?;
                                // Read BACK rather than kept in memory:
                                // `history` is already gap-preserving
                                // and downsampled, so the alerts see
                                // exactly the series the charts draw.
                                // That is what keeps #720's rule --
                                // never claim a rate the picture
                                // refuses to draw -- true by
                                // construction rather than by two
                                // implementations agreeing.
                                store::health::history(&c).map_err(|e| e.to_string())
                            }) {
                            Ok(history) => {
                                // The write landed, so the consecutive
                                // count resets. `last_error` is kept:
                                // "it failed 40 times and then
                                // recovered" is worth reading, and a
                                // chart with a gap and no explanation
                                // is what clearing it produces.
                                background::HEALTH_SAMPLER.ok(background::now_ms());
                                // Read per tick rather than cached, like
                                // the poll loop's: a setting change
                                // takes effect on the next sample
                                // instead of at the next relaunch.
                                //
                                // Until #789 these alerts notified
                                // UNCONDITIONALLY, with only
                                // `battery_low_percent` to adjust when.
                                // A user who wanted pull-request
                                // notifications and not machine ones had
                                // no way to say so; the master switch
                                // turned off both or neither.
                                let notify_prefs = commands::get_notify_prefs(app_handle.clone());
                                let threshold = health::alerts::low_percent(
                                    commands::read_ui_prefs(&app_handle).battery_low_percent,
                                );
                                let alerts = health::alerts::evaluate(&history, threshold);
                                // `take_new` filters to transitions AND
                                // re-arms cleared conditions, so a
                                // battery sitting at 24% is announced
                                // once rather than every sixty seconds.
                                let charge = history
                                    .last()
                                    .and_then(|s| s.battery.as_ref())
                                    .map(|b| b.percent);
                                // The dedup runs whether or not the
                                // category is wanted, and only the
                                // POSTING is gated. Filtering before
                                // `take_new` would make the fired-set
                                // disagree with reality: a condition
                                // that stood while its category was off
                                // would read as cleared, so switching
                                // the category back on would re-announce
                                // a condition that had never stopped.
                                for alert in fired.take_new(&alerts, charge) {
                                    if notify_prefs.wants_health(alert.key()) {
                                        notify_battery(&app_handle, &alert);
                                    }
                                }

                                // Tier 3, the one rule that notifies,
                                // on the same history the battery
                                // rules just used -- so both see
                                // exactly the series the charts draw,
                                // and neither can claim a duration the
                                // picture refuses to show.
                                let cpu_alerts =
                                    health::runaway::evaluate(&history, Some(&aggregate));
                                // The SAME `Fired` the battery alerts
                                // use, so there is one record of what
                                // has already been said rather than
                                // two that can disagree. `take_new_keys`
                                // is the key-only half of the same
                                // transition filter: a machine grinding
                                // for an hour is announced once, not
                                // sixty times.
                                let present: Vec<&'static str> =
                                    cpu_alerts.iter().map(|a| a.key()).collect();
                                let new = fired.take_new_keys(&["diffuse_cpu"], &present);
                                for alert in &cpu_alerts {
                                    // Gated on the POSTING only, for the
                                    // reason given above the battery
                                    // loop: filtering before the dedup
                                    // would let a standing condition
                                    // read as cleared.
                                    if new.contains(&alert.key())
                                        && notify_prefs.wants_health(alert.key())
                                    {
                                        notify_runaway(&app_handle, alert);
                                    }
                                }
                            }
                            Err(e) => {
                                // Logged as before, AND counted (#1145).
                                // The chart cannot otherwise tell a gap
                                // the user caused by closing the app
                                // from a gap the app caused by failing
                                // to write, and it currently reassures
                                // the user it is the former.
                                log::warn!("system health: could not record a sample: {e}");
                                if background::HEALTH_SAMPLER.failed(e.to_string()) {
                                    use tauri::Emitter;
                                    // Only on the THRESHOLD, not on
                                    // every failure: one blip that
                                    // fixes itself is not worth a
                                    // banner, which is the reasoning
                                    // `poll.rs` already applies.
                                    let _ = app_handle.emit(
                                        "background-degraded",
                                        background::HEALTH_SAMPLER.snapshot(),
                                    );
                                }
                            }
                        }

                        // ---- The Claude Code live pass (#947) ----
                        //
                        // Here rather than in a frontend `useQuery`,
                        // which is what #947 suggested. The difference
                        // matters: a hook only runs while its page is
                        // MOUNTED, so the handoff file would be consumed
                        // only while the user happens to be looking at
                        // the Claude Code view. The file is written by
                        // every session start and end regardless of
                        // which view is open -- and `handoff.rs` makes
                        // truncation the consumer's job, deliberately,
                        // so a consumer that runs only sometimes leaves
                        // a file that grows the rest of the time. That
                        // is the defect #947 reports, and a
                        // page-scoped poll would only narrow it.
                        //
                        // Gated per tick, not once at startup, so
                        // turning the capability off stops the
                        // consumption on the next pass -- which is what
                        // `claude_integrations_enabled`'s own doc
                        // promises ("hide the view AND stop
                        // consuming"), and the same read-per-tick rule
                        // the notify prefs above follow so a setting
                        // change lands on the next sample rather than
                        // the next relaunch.
                        //
                        // A 60-second cadence, which is this loop's, not
                        // the 10 seconds the view's own queries use.
                        // Liveness for the VIEW is derived per read from
                        // the registry and does not depend on this pass
                        // at all; what this pass adds is the recorded
                        // run history and crash rows, where a minute of
                        // latency costs nothing. Sharing the existing
                        // thread also avoids a second timer to reason
                        // about.
                        if commands::read_ui_prefs(&app_handle).claude_integrations_enabled {
                            match commands::claude_live_pass(&commands::db_path(&app_handle)) {
                                Ok(state) => {
                                    background::CLAUDE_LIVE.ok(background::now_ms());
                                    // ---- "your session died" (#979) ----
                                    //
                                    // `crashed_sessions` and NOT a query
                                    // of the table: the list is built on
                                    // the same arm that increments
                                    // `crashed`, which is the
                                    // FIRST-OBSERVATION count.
                                    // `crashed_already_known` is its
                                    // opposite and is deliberately not
                                    // read here -- an orphan left on disk
                                    // is re-swept every minute, and a
                                    // notifier reading the wrong count
                                    // would announce the same dead
                                    // session forever. `crash.rs`'s
                                    // COALESCE is what makes the split
                                    // true and `record_crashed`'s doc
                                    // records the four-sweeps-four-rows
                                    // bug that found it.
                                    //
                                    // Read per tick, not once at startup,
                                    // the same rule the health prefs
                                    // above follow: a setting change
                                    // lands on the next sample rather
                                    // than the next relaunch.
                                    //
                                    // Already inside the
                                    // `claude_integrations_enabled` gate,
                                    // which is the second condition
                                    // #979 requires -- a user who turned
                                    // the feature off must not be
                                    // interrupted by it.
                                    if !state.sweep.crashed_sessions.is_empty() {
                                        let prefs = commands::get_notify_prefs(app_handle.clone());
                                        if prefs.enabled && prefs.claude_crashed {
                                            for crashed in &state.sweep.crashed_sessions {
                                                notify_claude_crash(&app_handle, crashed);
                                            }
                                        }
                                    }

                                    // Logged only when it did something.
                                    // A quiet machine ticking every
                                    // minute would otherwise bury every
                                    // other line in the log.
                                    if state.handoff.runs > 0
                                        || state.sweep.crashed > 0
                                        || !state.handoff.unparseable.is_empty()
                                    {
                                        log::info!(
                                            "claude: consumed {} run(s), {} crash(es), \
                                             {} unparseable, {} running",
                                            state.handoff.runs,
                                            state.sweep.crashed,
                                            state.handoff.unparseable.len(),
                                            state.running.len()
                                        );
                                    }
                                }
                                // Warned and skipped, never fatal, for
                                // the reason the health sample above
                                // gives: a pass that could not read
                                // `~/.claude` must not stop the loop
                                // that also records system health.
                                Err(e) => {
                                    log::warn!("claude: live pass failed: {e}");
                                    if background::CLAUDE_LIVE.failed(e.to_string()) {
                                        use tauri::Emitter;
                                        let _ = app_handle.emit(
                                            "background-degraded",
                                            background::CLAUDE_LIVE.snapshot(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            }

            // Phone pairing. Managed whether or not the listener is on,
            // because Settings lists and revokes paired devices either
            // way. `remote::gate::setup` below loads its device list
            // and manages the desktop identity it reads.
            app.manage(remote::pairing::new_state(handle.clone()));

            log::info!(
                "headstate v{} starting (authenticated: {})",
                env!("CARGO_PKG_VERSION"),
                gh_client.is_some()
            );

            if let Some(client) = gh_client {
                // WHICH account, not just that there is one. A reported
                // failure took four rounds partly because the log said
                // "authenticated: true" and nothing else -- so whether
                // two machines were even using the same account could
                // not be established from it.
                //
                // A login is a public GitHub handle, not a credential;
                // the token is never logged. Fire-and-forget so a slow
                // or failed lookup cannot delay startup.
                {
                    let c = client.clone();
                    tauri::async_runtime::spawn(async move {
                        match c.fetch_viewer().await {
                            Ok(login) => log::info!("signed in as {login}"),
                            Err(e) => log::warn!("could not read the signed-in account: {e}"),
                        }
                    });
                }
                // The PR Stats backfill (#1092, #1093), BESIDE the poll
                // loop rather than inside its tick.
                //
                // `poll::TICK_TIMEOUT`'s margin under `MIN_FOCUSED_SECS`
                // is load-bearing, and `budget::RESERVE` exists to protect
                // the poll loop -- so a consumer placed inside that loop
                // would both spend the margin and make the protection
                // self-referential. `spawn_backfill`'s own docs carry the
                // argument in full.
                poll::spawn_backfill(handle.clone(), client.clone());
                let focused = Arc::new(AtomicBool::new(true));
                app.manage(Focused(focused.clone()));
                poll::spawn(handle, client, focused, waker, interval, needs_gh);
            }

            tray::setup_tray(&app.handle().clone())?;

            // After the GitHub client is managed: `/v1/hello` reports
            // the signed-in login through it. Off by default; this only
            // binds a port when the setting says so.
            remote::gate::setup(&app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing hides to the tray rather than quitting, so polling
            // keeps running and the badge stays live. Quit is explicit,
            // from the tray menu or Cmd-Q. A window can be hidden without a
            // `Focused(false)` event, so this also has to clear the
            // focused flag itself -- otherwise polling would stay on the
            // 60s cadence forever after close-to-tray.
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // Read per close rather than cached at startup, so the
                // setting takes effect immediately rather than at the
                // next launch -- and an unreadable database falls back
                // to hiding, the app's pre-existing behaviour, because
                // quitting unexpectedly loses more than hiding does.
                if !crate::commands::read_ui_prefs(&window.app_handle().clone()).close_hides_to_tray
                {
                    // Let the close proceed: Tauri exits when the last
                    // window closes, which is what "quit" means here.
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
                if let Some(focused) = window.try_state::<Focused>() {
                    mark_hidden(&focused.0);
                }
            }
            tauri::WindowEvent::Focused(is_focused) => {
                if let Some(focused) = window.try_state::<Focused>() {
                    mark_focus(&focused.0, *is_focused);
                }
                // Regaining focus is exactly when fresh data is wanted, and
                // it is the reliable signal that a machine woke from sleep:
                // `tokio::time::sleep` does not fire while suspended and
                // does not compensate on wake, so without this the first
                // tick after a closed lid was up to a full interval late.
                if *is_focused {
                    if let Some(waker) = window.try_state::<poll::Waker>() {
                        waker.0.notify_one();
                    }
                }
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This is the bug Task 10 left behind: the `Arc<AtomicBool>` given to
    /// `poll::spawn` was never retained anywhere else, so nothing could
    /// ever flip it and the background 300s cadence was unreachable at
    /// runtime. Managing `Focused` as Tauri state and mutating it through
    /// `mark_hidden`/`mark_focus` (the same functions the window-event
    /// handler calls) is the fix; these tests exercise the exact same
    /// `Arc<AtomicBool>` that `poll::interval_for` reads, through the exact
    /// same functions the closure calls, proving the flag is both reachable
    /// and mutable at runtime -- not merely that the module compiles.
    #[test]
    fn hiding_the_window_clears_the_focused_flag() {
        let focused = Arc::new(AtomicBool::new(true));
        mark_hidden(&focused);
        assert!(!focused.load(Ordering::Relaxed));
        assert_eq!(
            poll::interval_for(focused.load(Ordering::Relaxed)),
            poll::BACKGROUND
        );
    }

    #[test]
    fn losing_focus_clears_the_flag_and_gaining_it_sets_it() {
        let focused = Arc::new(AtomicBool::new(true));

        mark_focus(&focused, false);
        assert!(!focused.load(Ordering::Relaxed));

        mark_focus(&focused, true);
        assert!(focused.load(Ordering::Relaxed));
    }

    /// Drives a real `tauri::WindowEvent::Focused` value (not a stand-in)
    /// through the same match arm used in `run`'s `on_window_event`
    /// closure, confirming the event type itself -- constructed exactly as
    /// the platform would deliver it -- reaches `mark_focus` and flips the
    /// shared flag that `poll::spawn` reads.
    #[test]
    /// Note the limit of this test: it duplicates the match arm rather than
    /// invoking the real `on_window_event` closure, because Tauri gives no way
    /// to construct a `CloseRequested`'s `api` field or dispatch a synthetic
    /// event from a unit test. So it proves `mark_focus` maps a real
    /// `WindowEvent::Focused` payload onto the cadence correctly -- it does NOT
    /// prove the production closure is wired up. That wiring is guaranteed by
    /// the type checker and by reading lib.rs's handler, not by this test.
    fn a_real_focused_event_reaches_the_shared_flag() {
        let focused = Arc::new(AtomicBool::new(true));

        let event = tauri::WindowEvent::Focused(false);
        match &event {
            tauri::WindowEvent::Focused(is_focused) => mark_focus(&focused, *is_focused),
            _ => unreachable!(),
        }

        assert!(!focused.load(Ordering::Relaxed));
        assert_eq!(
            poll::interval_for(focused.load(Ordering::Relaxed)),
            poll::BACKGROUND
        );
    }

    /// `Focused` is managed as Tauri state and read back through
    /// `try_state::<Focused>()` inside the real window-event closure. That
    /// retrieval is unit-testable on its own: the newtype wraps the same
    /// `Arc<AtomicBool>` `poll::spawn` holds, so storing through one handle
    /// is observable through the other -- which is exactly what
    /// `app.manage(Focused(focused.clone()))` plus `window.try_state` give
    /// us at runtime.
    #[test]
    fn the_managed_focused_newtype_shares_the_same_arc_poll_reads() {
        let focused = Arc::new(AtomicBool::new(true));
        let managed = Focused(focused.clone());

        mark_hidden(&managed.0);

        // The clone `poll::spawn` was given observes the same store.
        assert!(!focused.load(Ordering::Relaxed));
    }
}
