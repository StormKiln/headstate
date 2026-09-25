//! The remote allowlist: the single place that decides what a paired
//! phone can do.
//!
//! Every Tauri command registered in `lib.rs`'s `generate_handler!` has
//! exactly one row in [`SURFACE`], and the row's [`Class`] is the whole
//! policy. A command added to `generate_handler!` without a row here
//! fails [`tests::every_registered_command_has_exactly_one_class`], so a
//! new command can be neither exposed nor omitted silently.
//!
//! # Contract for the `POST /v1/call/{command}` handler
//!
//! The HTTP route lives in `remote/listener.rs`, not here. The handler
//! that mounts it must:
//!
//! 1. Read `{command}` from the path and the JSON body as
//!    `serde_json::Value` (an object of camelCase keys, exactly what the
//!    webview passes to `invoke`; an empty or absent body is `{}`).
//! 2. Look the command up with [`class_of`]. When it is
//!    [`Class::Destructive`], verify the `X-Headstate-Signature` step-up
//!    header (nonce, timestamp, and every signature the pairing record
//!    expects) BEFORE calling [`dispatch`]. `dispatch` does not check
//!    signatures; it trusts that the caller already refused a destructive
//!    request without a valid one.
//! 3. Call [`dispatch`] with the paired device's name and map the result:
//!    `Ok(value)` is the response body, and each [`RemoteError`] variant
//!    documents the status it should become.
//!
//! `dispatch` refuses [`Class::Local`] and unknown commands itself, so a
//! handler that skips step 2 for a non-destructive command is still safe;
//! it is the signature check that only the handler can do.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::commands;
use crate::remote::error_kind::CommandError;

/// What a command does, which decides what a phone must present to run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// No side effects on GitHub or disk.
    Read,
    /// Changes GitHub state through the existing write module, or a
    /// desktop setting.
    Write,
    /// Deletes files, branches, images, or volumes. Requires the step-up
    /// signature (see the module docs) before dispatch.
    Destructive,
    /// Not exposed remotely: opens a window, reveals a file, changes
    /// autostart, runs an agent, or decides who may pair.
    ///
    /// The test is whether the phone could act on the answer, not
    /// whether the command touches the desktop -- driving the desktop
    /// is what the companion is for. Revealing a file in a Finder the
    /// phone cannot see fails that test; restarting Docker does not.
    Local,
}

/// Command name to class. The order is the spec's; keep it that way so a
/// diff against the design document is a line-by-line comparison.
pub const SURFACE: &[(&str, Class)] = &[
    // read: no side effects on GitHub or disk.
    // Read: the phone asking whether the DESKTOP's background work died
    // is a question about state, not an action on the machine. The
    // remedy it points at (`reveal_log`) stays Local, which is why the
    // companion says the log opens at that Mac.
    ("background_panicked", Class::Read),
    // Read: whether the desktop's background LOOPS are still working,
    // as distinct from whether one panicked (#1145). Same question and
    // same class as the row above -- a phone asking "why has the chart
    // stopped" needs the answer, and the answer is about that Mac.
    ("background_health", Class::Read),
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
    // The hardened stats layer (#824). A Read: it issues `search` and
    // `repository.pullRequests` queries and returns counts.
    //
    // Exposed because the phone's stats page asks the same questions the
    // desktop's does, and the alternative is a second copy of the
    // slicing, metering and concurrency rules in `src-mobile` -- which
    // is precisely the drift `health_alerts` above refuses for the same
    // reason. The desktop's wall-clock ceiling and read-concurrency cap
    // are inside the command, so a phone's `remote_call` inherits both.
    ("stats_count", Class::Read),
    // The scope hierarchy the stats sidebar renders (#825). A Read: two
    // GraphQL lookups returning org, repository and member NAMES, and no
    // statistics at all.
    //
    // Exposed for the same reason `stats_count` is, even though PR Stats is
    // in `MOBILE_HIDDEN_VIEWS` today: the row classes a command by what it
    // DOES, not by which screens currently call it, and a Local
    // classification here would have to be revisited the moment the
    // companion grows the view. Nothing about enumerating scopes is
    // desktop-specific -- unlike `reveal_in_finder`, a phone could act on
    // this answer perfectly well.
    ("stats_tree", Class::Read),
    // The per-author board behind the Mine and Others views (#826). A
    // Read, and the most expensive one in this table: it probes, slices,
    // and fetches per-PR nodes across a whole scope.
    //
    // Classed by what it does rather than by which screens call it, the
    // same rule `stats_tree` above is classed by. Its wall-clock ceiling,
    // read-concurrency cap and budget refusal all live INSIDE the command,
    // so a phone's `remote_call` inherits every one of them -- which is
    // the property that makes exposing the expensive path safe rather
    // than a second set of limits to keep in sync.
    ("stats_board", Class::Read),
    // The scoped daily activity series (#826). A Read, and the cheap half
    // of a scope page: count-only searches, no nodes.
    ("stats_series", Class::Read),
    // The reviews-GIVEN leaderboard (#826, restored by its reopening). A
    // Read, and the CHEAPEST query on the stats page: one count-only
    // `reviewed-by:<login>` search per member, all aliased into one request
    // at 1 point -- measured 0.84-1.04s for this account's real 4-member
    // org. No nodes, so the ~11s deadline that governs `stats_board`'s
    // document does not bind it.
    //
    // Classed by what it does rather than by which screens call it, the same
    // rule `stats_tree` above is classed by: a phone could act on a ranking
    // of reviewers perfectly well, and its budget refusal and wall-clock
    // ceiling live inside the command.
    ("stats_reviewers", Class::Read),
    ("get_reviewing", Class::Read),
    ("count_reviewing", Class::Read),
    ("get_pr_detail", Class::Read),
    ("get_viewer", Class::Read),
    ("build_target", Class::Read),
    ("latest_release", Class::Read),
    ("list_worktrees", Class::Read),
    // The repository browser's two commands (#1035, epic #1011). Both
    // `Class::Read`: one lists a directory level from the git index and
    // the other reads at most 256 KB of one file, and neither writes
    // anything -- on GitHub or on disk.
    //
    // Exposed rather than `Local`, and this is the sharpest case of the
    // companion's whole purpose yet. Apply `Class::Local`'s stated test --
    // could the phone act on the answer? -- and it is plainly yes; a
    // listing and a file are exactly what a person away from their desk
    // wants. `claude_transcript_tail` makes the stronger form of the
    // argument and it transfers verbatim: "the desktop user can `cat` the
    // file and the companion user cannot reach the machine." A repository
    // browser is that argument repeated for every file in 38
    // repositories.
    //
    // What makes `Read` safe here is that ALL THREE limits live inside
    // the commands, so a phone's `remote_call` inherits every one of them
    // rather than keeping a second copy in sync -- the rule `stats_board`
    // is classed by:
    //
    //   1. `repo_path_in` refuses a path outside the repository root, a
    //      symlink, and an absolute path. A `Class::Read` command's path
    //      argument arrives over the pairing transport, and a paired
    //      device being trusted to read Headstate's data is not a reason
    //      to turn a path parameter into "read any file on this machine".
    //   2. The root is RE-DERIVED against `list_worktrees` at the moment
    //      of the call, never trusted from the UI's selection. The
    //      browser has no extension check to lean on, so this is what
    //      stops a caller-chosen root from meaning "anywhere".
    //   3. A 256 KB window on the file read, with the truncation stated.
    //      So the 275 MB tracked zip measured in that corpus cannot be
    //      pulled over the transport: the command never reads it.
    //
    // Is a 275 MB file a problem? No -- because the command never sends
    // one, and that is an answer only because the bound is server-side.
    // Never widen it for the desktop alone: one limit, inside the
    // command, for both callers.
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
    // Read: what a previous scan already found, so a cold start paints
    // rows instead of a blank page (#1152). Same class as the scan
    // above -- it returns the same information, just earlier -- and it
    // authorises nothing: every destructive path re-verifies live.
    ("read_cached_scan", Class::Read),
    ("size_artifacts", Class::Read),
    ("scan_venvs", Class::Read),
    ("size_venvs", Class::Read),
    ("docker_state", Class::Read),
    ("docker_builds", Class::Read),
    ("docker_images", Class::Read),
    // System Health (#663). Reads, so neither carries a step-up
    // signature and neither can reach the nonce path (#656).
    ("system_health", Class::Read),
    ("system_health_history", Class::Read),
    // The evaluated health conditions (#789). A Read: it runs the rules
    // over the same stored series `system_health_history` returns and
    // reports verdicts rather than data.
    //
    // Exposed because the rules must exist in exactly ONE place. The
    // phone could fetch the history and evaluate it itself, and that
    // would be a second copy of every threshold in `health::alerts` and
    // `health::runaway` -- in a separate crate with a separate lockfile,
    // drifting silently while both suites stayed green. A drifted copy
    // of a rule about when to interrupt someone is worse than no rule.
    ("health_alerts", Class::Read),
    // What is using the DESKTOP's processors and memory (#687, #721) --
    // the machine's top processes, which the CPU and Memory detail pages
    // show. Exposed because that is the whole point on the phone: the
    // companion drives a desktop you left running, and "what is pinning
    // that machine's cores" is a question you ask about a machine you
    // are not sitting at. Reads the process table and nothing else.
    //
    // Named `footprint` for historical reasons only: it answered "is
    // Headstate itself why that machine is busy" until #795 removed that
    // panel. The name is load-bearing HERE specifically -- this row
    // matches on the literal command string, and `src-mobile`'s copy of
    // this list has to agree with it -- so it was not renamed.
    ("system_footprint", Class::Read),
    // Which processes are using the DESKTOP's network (#718). A read,
    // and exposed for the same reason `system_footprint` is: the phone
    // drives a desktop, and "what is saturating that machine's link" is
    // a question you ask about the machine you left running.
    //
    // It costs ~5 SECONDS on the desktop, uniquely among these -- see
    // `health::netproc`. That is why it is its own command rather than
    // part of `system_health`, and the phone must call it on the
    // Network page's slow cadence only, never beside the health poll.
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
    // Local, and here the class IS the safety property rather than a
    // rendering hint (#1219). These two end a Claude Code process on
    // THIS machine, and a phone must not be able to kill a session on a
    // Mac it is not sitting at -- so neither has a dispatch arm in
    // `call()` below, and `every_registered_command_has_exactly_one_class`'s
    // sibling assertion fails on any that grows one.
    //
    // `Destructive` was considered and rejected: a stopped session keeps
    // its transcript and can be resumed, so a step-up signature would be
    // friction disproportionate to a recoverable action -- and would make
    // the genuinely irreversible actions feel routine by association.
    //
    // The PROPOSAL signals nothing and could have been `Read`. It is
    // `Local` anyway, because splitting the classification would put the
    // evidence for an action on the phone beside a button that can only
    // reject, which is the #603/#604/#606 shape.
    ("claude_propose_stop", Class::Local),
    ("claude_stop_session", Class::Local),
    // Local for the same reason as the two above, and it is the
    // reason more than the side effect: these describe the argv of a
    // WINDOW that would open on the desktop. A phone shown that line
    // would be reading the preview of a button it cannot press (#1214).
    ("claude_launch_worktree_preview", Class::Local),
    ("claude_launch_session_preview", Class::Local),
    // Read, unlike the four above: a constant list of the tokens
    // `terms::Terms::parse` accepts, with no side effect and nothing
    // desktop-specific about it. Classing a constant as Local would
    // make the desktop refuse to forward a call that cannot do
    // anything -- Local is about what a command DOES, not about which
    // screen its caller sits on.
    ("claude_launch_terms", Class::Read),
    ("check_packages", Class::Read),
    ("packages_markdown", Class::Read),
    // Read: the effective context a session loads, across scopes
    // (#1131). One more file read than `scan_claude_md`, same class.
    ("claude_md_effective", Class::Read),
    // Read: the same walk as `claude_md_effective`, with every advice
    // producer run over it, or the stored report served when the tracked
    // inputs still match (#1293).
    //
    // Still `Read` with the cache: the only thing it writes is
    // Headstate's own derived report, the same ground on which
    // `claude_import_transcripts` is Read -- it changes no GitHub state,
    // no desktop setting, and nothing under `~/.claude`. `Write` is for
    // a command that changes something a user would not want changed by
    // a phone.
    ("claude_md_advice", Class::Read),
    ("claude_md_advice_launch", Class::Local),
    ("claude_md_advice_launch_preview", Class::Local),
    ("scan_claude_md", Class::Read),
    ("read_claude_md", Class::Read),
    // Rescan `~/.claude/projects` and upsert into our own cache (#914).
    //
    // `Read`, by the same reasoning `refresh_now` is Read: it reads
    // local disk WITHOUT writing to it -- the transcripts are Claude
    // Code's data and `claude --resume` depends on them, so nothing here
    // touches `~/.claude` -- and the only thing it writes is Headstate's
    // own cache. It changes no GitHub state and no desktop setting, which
    // is what `Write` is for.
    //
    // Exposed rather than `Local` even though the PHONE has no
    // transcripts of its own, because that is the point: a remote call
    // asks the DESKTOP to re-read the desktop's history, and "which of my
    // sessions died?" is precisely the away-from-desk question the
    // companion exists for. Apply `Local`'s stated test -- could the phone
    // act on the answer? -- and the answer is yes: the resurrect action is
    // a copy of `claude --resume <id>`, which is why `claudify_command`
    // above is Read on the same grounds.
    //
    // Not cheap, but bounded: a full rescan of the real 1,430-session,
    // 881 MB corpus measures well under a second because each transcript
    // costs a bounded head read plus a 16 KB tail seek, and the ceiling
    // lives inside the command rather than in this table.
    ("claude_import_transcripts", Class::Read),
    // Content search over the transcript corpus (#1203). Read: it walks
    // `~/.claude/projects` without writing to it and queries the FTS5
    // index in Headstate's own cache. The one write it makes is to our
    // OWN denominator row, which records how many transcripts exist --
    // bookkeeping about our index, not about the user's data.
    //
    // Worth exposing remotely for the reason the session list is: "which
    // session was I in when I hit that error" is an away-from-desk
    // question, and the phone has no transcripts of its own to search.
    ("claude_search_transcripts", Class::Read),
    // How much of the corpus is searchable (#1203). Read: two COUNT
    // queries and one directory walk. Separate from the search so a
    // client can state coverage before anything is typed -- a search box
    // that says nothing about its own readiness invites an empty result
    // to be read as settled.
    ("claude_index_coverage", Class::Read),
    // The session list with derived liveness (#917). Read: it queries our
    // own cache, lists `~/.claude/sessions` and probes the process table.
    // Nothing is written anywhere, and `~/.claude` is only ever read.
    //
    // Exposed for the reason `claude_import_transcripts` above is, and
    // this one is the stronger case: "did the thing I left running on my
    // laptop die?" is the away-from-desk question the companion exists
    // for, and it is the same question `system_health` answers remotely.
    // The answer describes the PAIRED DESKTOP's sessions, which is what
    // the view must say on the phone.
    //
    // The row's resume command comes back as text, so the phone can read
    // what would resurrect a session even though its clipboard cannot
    // reach a desktop shell -- the same reasoning that makes
    // `claudify_command` Read.
    ("claude_sessions", Class::Read),
    // The other half of that row since #985: what ONE session knows that
    // the list no longer carries -- its resume command, transcript path
    // and stat, version, start time and run count.
    //
    // `Read` for exactly the reasons `claude_sessions` is, and the
    // classing does not change by being split: it queries the desktop's
    // own cache, stats two paths under the desktop's home, and writes
    // nothing. The resume command still comes back as text the phone can
    // read without being able to run it.
    //
    // It is the phone that gains most from the split -- the list is what
    // crosses the pairing transport every ten seconds, and this is the
    // call that lets it stop carrying the detail for 1,474 rows to render
    // one.
    // Read: one indexed query (#1132).
    ("claude_sessions_for_pr", Class::Read),
    ("claude_session_detail", Class::Read),
    // #1002. Reads each attributed child transcript with #959's bounded
    // summariser and the app's own database; writes nothing. The phone
    // wants the rollup for the same reason the desktop does.
    ("claude_subagent_rollup", Class::Read),
    // #1062, #1063, #1064. Two indexed reads over the app's own
    // database plus a parse of `~/.claude/settings.json`; writes
    // nothing. The phone wants both for the same reason the desktop
    // does -- "which of my sessions was fighting something" is a
    // question you ask while away from the machine.
    ("claude_session_events", Class::Read),
    ("claude_event_profile", Class::Read),
    // One pass over the two LIVE sources: the hook's handoff file and the
    // `~/.claude/sessions` registry (#913).
    //
    // A `Read` even though it writes, and the distinction this table
    // draws is "changes GitHub state or a desktop setting", which this
    // does not: every write lands in Headstate's own cache, plus a
    // truncation of Headstate's OWN handoff file after the records in it
    // are committed. `claude_import_transcripts` above is classed the
    // same way for the same reason -- both populate a cache from disk
    // rather than changing anything a user would call state.
    //
    // NOT `Local`, and the test the class docs give is whether the phone
    // could act on the answer. It can: the answer is which sessions are
    // running on the paired desktop and which crashed, which is the whole
    // of what the companion's session list shows.
    ("claude_poll_live", Class::Read),
    // Aggregates for the Claude Code overview page (#921).
    //
    // `Read`, and for a narrower reason than the rescan above: it runs two
    // SELECTs over our own cache, stats one directory per session, and
    // lists `~/.claude/sessions`. It writes nothing anywhere -- not even
    // to Headstate's cache, unlike `claude_import_transcripts`, which
    // upserts.
    //
    // Exposed rather than `Local` on `Local`'s own stated test: could the
    // phone act on the answer? Yes, and this is the strongest case in the
    // set. The page's headline number is how many sessions are resumable,
    // and "did the thing I left running on my laptop die, and how much is
    // waiting for me?" is exactly the away-from-desk question. The action
    // it leads to is a copy of `claude --resume <id>`, which
    // `claudify_command` is already `Read` for.
    ("claude_overview", Class::Read),
    // What the app has read against what it holds (#1212).
    //
    // `Read`, and the narrowest in this whole table: three COUNTs over
    // Headstate's own cache. It opens no transcript, stats no directory
    // and writes nothing.
    //
    // Exposed on `Local`'s own test -- could the phone act on the
    // answer? The question here is subtler than "act": this is the panel
    // that says what every OTHER figure covers, so withholding it from
    // the companion would leave the phone showing bounded numbers with
    // the bound only visible at the desk.
    ("claude_coverage", Class::Read),
    // Installed plugins and their measured usage (#1075).
    //
    // `Read`: it lists `~/.claude/plugins/installed_plugins.json`, reads
    // transcript bodies, and writes only to Headstate's OWN cache
    // (migration 16's per-file scan table) -- the same thing
    // `claude_import_transcripts` above is `Read` for. It installs
    // nothing, enables nothing and removes nothing; every plugin
    // management action stays in Claude Code's own hands.
    //
    // The classification is about what the command DOES, and this one
    // counts. That its answer might persuade someone to uninstall a
    // plugin is a property of the information, not of the command.
    // Read: a directory walk over ~/.claude with no side effects
    // (#1129).
    ("claude_definitions", Class::Read),
    ("claude_plugins", Class::Read),
    // The restart list: every running session's resume command (#1071).
    //
    // `Read`, on the same grounds as `claude_sessions`, which it is a
    // fold of: one registry listing, one process probe, two SELECTs over
    // Headstate's own cache. It writes nothing anywhere, spawns nothing,
    // and touches no terminal -- the answer is text.
    //
    // Exposed rather than `Local`, and the phone's case is weaker than
    // for anything else in this group but it is not zero. The stated test
    // is whether the phone could act on the answer, and "which sessions
    // are alive on my laptop right now, and what would bring each back"
    // is readable away from the desk even where pasting is not: it is the
    // away-from-desk inventory, and the companion already shows the same
    // fact per row through `claude_session_detail`. The frontend decides
    // whether to OFFER the button on a phone; the class only decides
    // whether the command can be reached at all, and refusing it here
    // would make a command the companion's own session pane already
    // exposes piecemeal unreachable in aggregate.
    ("claude_restart_list", Class::Read),
    // How much work happened inside ONE session, summed from its own
    // transcript (#959).
    //
    // `Read`: it reads one `.jsonl` under `~/.claude/projects` and writes
    // nothing at all -- not even to Headstate's cache, since the figures
    // are derived per read and a stored copy is exactly the kind of
    // refreshed-into-a-lie state migration 11 deliberately lacks.
    //
    // Exposed on `Local`'s own test -- could the phone act on the answer?
    // Yes: "was that session the long one or the typo" is how a user
    // decides which of 1,475 rows is worth resuming, and 994 assistant
    // messages against 4 is the distinction nothing else on the row
    // makes.
    //
    // The response is a handful of integers, so the transport cost is
    // nil. The READ is bounded inside the command at 8 MB
    // (`usage::BUDGET_BYTES`) and says when it stopped, which is what
    // keeps the 76.7 MB transcript on the development machine from
    // hanging a phone's request -- the same "the limits live inside the
    // command" rule `stats_board` above is classed by.
    // Read: one aggregate query over stored rows (#1134).
    ("claude_usage_profile", Class::Read),
    ("claude_session_usage", Class::Read),
    // The tail of one session's transcript, as conversation (#982).
    //
    // `Read`, and the phone's case here is STRONGER than the desktop's.
    // `claude_reveal_path` is `Class::Local`, so until now a companion
    // user who could see that a session died could not see one word of
    // what it was doing -- the desktop user can `cat` the file and the
    // companion user cannot reach the machine at all.
    //
    // It reads one `.jsonl` under `~/.claude/projects` and writes
    // nothing; `~/.claude` stays read-only, per `claude/mod.rs`'s two
    // stated exceptions, neither of which this is.
    //
    // The response is bounded inside the command -- a 256 KB window, at
    // most 200 messages, each block clamped -- so the phone cannot be
    // handed a 76 MB file by asking for one. Both commands resolve their
    // path argument against `~/.claude/projects` before reading, because
    // unlike `claude_reveal_path` a `Read` command's argument arrives
    // from a paired device rather than from this machine's own frontend;
    // `claude_transcript_path` in `commands.rs` argues it.
    ("claude_transcript_tail", Class::Read),
    // One incremental step of following a live transcript (#1208).
    //
    // `Read` on exactly the grounds the row above carries, and the phone
    // benefits more than the desktop again: a companion user watching a
    // running agent gets the transcript as it is written rather than a
    // snapshot frozen at the moment they tapped.
    //
    // It reads LESS than the row above, not more. `tail` pulls a 256 KB
    // window per call; this reads from the cursor the caller returns, so
    // a poll over a transcript that did not change moves no transcript
    // bytes at all -- only the bounded 64 KB fingerprint that detects a
    // compaction having rewritten history behind the cursor.
    ("claude_transcript_follow", Class::Read),
    // Whether the Claude Code hooks are in `~/.claude/settings.json`
    // (#915).
    //
    // `Read`: it reads one file and writes nothing. Exposed even though
    // the three commands that CHANGE that file are `Local` below, because
    // reading status remotely is both harmless and useful -- "is that
    // desktop recording?" is a real away-from-desk question, and the
    // answer does not depend on being able to act on it.
    //
    // Note the return type is a three-state `Status`, not a bool. The
    // phone must be able to render "cannot tell" as its own thing: a
    // settings file Claude Code cannot parse is IGNORED silently, so a
    // desktop in that state has every hook dead, and showing that as
    // "not installed" points at a button the phone does not even have.
    // Read: one file, no side effects, and "what is wired into my
    // sessions" is a reasonable thing to ask from a phone (#1127).
    ("claude_hooks_inventory", Class::Read),
    // Read: three file reads, no side effects (#1130).
    ("claude_effective_settings", Class::Read),
    // Read: the configuration-health sweep (#1217). Many file reads
    // across every scanned repository and no side effects -- it parses
    // settings and resolves CLAUDE.md imports, and writes nothing, offers
    // no repair and takes no path from the caller. `Read` rather than
    // `Local` because the answer is about the desktop's repositories,
    // which is exactly what a phone cannot see for itself.
    ("claude_config_health", Class::Read),
    // Read: `~/.claude.json` plus each installed plugin's `.mcp.json`,
    // bounded and read-only (#1216). Never a write: that file is Claude
    // Code's live state, rewritten by its owner while it runs.
    ("claude_mcp_servers", Class::Read),
    // Read: two file reads plus a sweep of Headstate's OWN ledger, which
    // is not the user's settings file (#1199). Nothing under `~/.claude`
    // is written, and the middle state -- a rule the user has edited
    // since we wrote it -- must render as theirs rather than as ours.
    ("claude_permission_ownership", Class::Read),
    ("claude_hooks_status", Class::Read),
    ("get_poll_interval", Class::Read),
    ("get_worktree_dirs", Class::Read),
    ("get_ui_prefs", Class::Read),
    // How a run is going, or how it ended. The resume path: a
    // suspended phone holds no event stream, so it asks instead.
    ("update_run_state", Class::Read),
    // How the Update All run is going, or how it ended (#1016). The same
    // resume path as `update_run_state` above and `Read` for the same
    // reason: it reads one in-memory registry and writes nothing.
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
    // `open_update_pr` was a `Write` row here and is gone (#964), with
    // its `#[tauri::command]` wrapper and its `lib.rs` registration. #626
    // superseded the two-phase flow, so nothing on the desktop called it
    // and only a paired phone could reach it -- one fewer `Write` on the
    // wire. `commands::open_update_pr_inner` is the live code and stays;
    // `apply_updates_in_background`, still classed below, is how the
    // phone drives an update now.
    // Driving the desktop IS the companion, so these are Write
    // rather than Local: pulling a checkout, starting the desktop's
    // Docker and restarting it are the things a person opens the
    // phone to do. They change the desktop but delete nothing, so
    // they do not carry the step-up signature.
    ("pull_checkout", Class::Write),
    // Fast-forward every repository in the scan roots (#1019, #1012).
    //
    // WRITE, and the argument has three halves.
    //
    // **Not `Read`.** This table's `Read` means "no side effects on
    // GitHub OR DISK" -- `fetch_refs` below refuses `Read` for a far
    // smaller write. This moves branch refs and rewrites working tree
    // files. Not close.
    //
    // **Not `Local`.** `Local`'s own test is "whether the phone could act
    // on the answer, not whether the command touches the desktop --
    // driving the desktop is what the companion is for", and
    // `pull_checkout` directly above settles the single-repository case
    // deliberately in favour of exposure. On "a phone triggering a bulk
    // mutation of a desktop's working trees": the phone is not the risk.
    // The REFUSAL SET is the safety property, and it is identical
    // whatever the caller -- a phone's `remote_call` routes through the
    // same `worktrees::update`, inheriting its fresh per-repository dirty
    // check, its `--ff-only`, and every refusal in it. There is no code
    // path by which a phone can fast-forward a repository the desktop
    // would not. That is the property `stats_board`'s row relies on: the
    // limits live INSIDE the command, so exposure costs no second set to
    // keep in sync. Classing it `Local` would also class by WHICH SCREEN
    // calls it rather than by what it does, which `stats_tree`'s row
    // rejects explicitly.
    //
    // **Not `Destructive`.** This table draws that line at DELETION, and
    // says so four times -- unlock ("nothing is deleted"), `fetch_refs`
    // ("passes no `--prune`, precisely so that it cannot delete"),
    // `apply_updates_in_background` ("deleting nothing"). A `--ff-only`
    // pull deletes nothing: it creates no merge commit, discards no
    // commit, and on refusal leaves the working tree byte-identical.
    // Dirty trees are refused outright, so there is no uncommitted work
    // for it to touch. The closest precedent is
    // `apply_updates_in_background` below -- already a bulk,
    // long-running, cancellable working-tree mutation started from a
    // phone -- and this has the smaller blast radius per unit: it writes
    // no file contents of its own, only moving a branch to a commit that
    // already exists on the remote.
    //
    // The counter-argument, stated because it deserves to be: Destructive
    // is the only class carrying a step-up signature, and a bulk action
    // across 45 repositories is where a mis-tap costs most. But step-up
    // is not a general "are you sure" -- it is this table's marker for
    // deletion, and stretching it to mean "large" would make the class
    // mean two things and leave the next author unable to tell which.
    // Confirmation for a bulk action belongs in the UI, where it can name
    // the count and say what will be skipped; step-up can say neither.
    //
    // Exposed only because it can be STOPPED and its outcome read back
    // after a suspension -- `cancel_update_all` and `update_all_state`
    // ship in the same change, borrowing `apply_updates_in_background`'s
    // own condition verbatim: *starting something you cannot stop or see
    // the end of is not a feature*. That is a hard dependency, not a
    // nicety: MEASURED, the worst case is 45 x `GIT_TIMEOUT` ~= 22
    // minutes, and a phone holds no event stream while suspended.
    //
    // `pull_checkout` is NOT reclassified. Its row is load-bearing for
    // the single-repository case, and this row does not change what one
    // pull does. Nor is this a way to reach a laxer pull: the refusal set
    // is inherited from `pull_checkout`, and if that ever stops being
    // true this class argument collapses with it.
    ("update_all_repositories", Class::Write),
    ("cancel_update_all", Class::Write),
    // Refreshing one repository's remote refs (#788). WRITE, not Read.
    //
    // It mutates nothing on GitHub, which is what makes the Read
    // classification tempting -- `git fetch` is the most read-shaped
    // thing git does. But this table's Read means "no side effects on
    // GitHub OR DISK", and a fetch writes: `origin/*` refs, `FETCH_HEAD`,
    // and new objects in the repository. It is not Destructive either --
    // `fetch_refs` passes no `--prune`, precisely so that it cannot
    // delete the `origin/*` refs that `MergedUpstreamDeleted` (#732)
    // reads, and it moves no branch the user is standing on. Write is
    // exactly the middle the class exists for: changes local state,
    // deletes nothing, so no step-up signature.
    //
    // Exposed rather than Local on the same argument the three rows
    // around it make: driving the desktop IS the companion. A phone
    // reading "up to date with upstream · as of a fetch 2 days ago" can
    // act on that answer perfectly well, and it is the one screen where
    // the alternative -- pull, to find out whether you needed to pull --
    // is worst, because the phone cannot then fix a conflict.
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
    // Clears a worktree's lock (#775). Write, not Destructive, and the
    // line is drawn where this table draws it everywhere else: nothing
    // is deleted. The lock is one file git wrote, `git worktree lock`
    // puts it back, and no work can be lost by clearing it.
    //
    // It does remove a GUARD, which is why it is not Read -- but
    // routing it through step-up would say the phone is about to
    // destroy something, and the honest warning is a different one:
    // another process may be using that directory. That belongs in the
    // confirmation, which names the holder and the age, rather than in
    // a signature prompt about an unrecoverable action this is not.
    //
    // Removal is unaffected. `remove_worktree` stays Destructive and
    // its gate re-classifies the worktree from scratch, so unlocking
    // buys a phone no shortcut to deleting anything.
    ("unlock_worktree", Class::Write),
    // Clears a repository's stale worktree registrations (#793). Write,
    // not Destructive, and the line is the one this table draws
    // everywhere else: nothing recoverable is deleted.
    //
    // `git worktree prune` removes entries under `.git/worktrees/` whose
    // directory is ALREADY gone -- git itself reports them as "gitdir
    // file points to non-existent location". So there is no tree to lose
    // work from, no file leaves the disk, no branch is touched and no
    // commit becomes unreachable. It is strictly less destructive than
    // `unlock_worktree` above, which at least removes a guard; the worst
    // case here is that it does nothing, which is also a legitimate
    // result it reports as 0.
    //
    // Deliberately NOT Destructive, even though the name reads like
    // `docker_prune_cache` which IS. That one deletes build cache a
    // later build would otherwise reuse -- real bytes, real rebuild
    // cost. This deletes a dangling pointer. Routing it through the
    // step-up signature would tell the phone's user they are about to
    // destroy something, and spending that prompt on bookkeeping is how
    // it stops being read on the removals that matter.
    //
    // Removal is unaffected: `remove_worktree` and
    // `remove_worktree_forced` stay Destructive, and pruning buys a
    // phone no shortcut to either -- a registration it clears had no
    // directory for them to act on.
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
    // `apply_package_updates` was a `Destructive` row here and is gone
    // (#964), the largest single reduction in this table's write surface:
    // it ran package managers in write mode, in a worktree, from a phone,
    // down a path #626 left with no desktop caller and no frontend test.
    // The apply helpers stay -- `apply_updates_in_background` below is
    // what actually runs an update, and it is classed and dispatched.
    // local: not exposed remotely.
    ("diag_log", Class::Local),
    // Read: the tail of the DESKTOP's log, redacted before it leaves
    // (#1147). A question about what happened on that machine, which is
    // exactly what a phone diagnosing a failure asks -- and the gap
    // `reveal_log` cannot close, because there is no Finder here to
    // reveal into. That one stays Local; this shows the text.
    ("read_log_tail", Class::Read),
    ("reveal_log", Class::Local),
    // Reveals a session's directory or transcript in the file manager
    // (#917). `Local` for exactly the reason this class's own doc comment
    // gives: "Revealing a file in a Finder the phone cannot see fails
    // that test." The same call as `reveal_log` above, pointed at a path
    // from a session row instead of at our log.
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
    // The Claude Code hook installer (#915). All three, deliberately.
    //
    // Not Read, not Write, not Destructive -- `Local` is the right
    // REFUSAL, and the reasoning is worth stating because none of the
    // other three classes is obviously wrong:
    //
    // - It edits a config file OUTSIDE Headstate's ownership, shared with
    //   other tools. `Write`'s own definition above is "changes GitHub
    //   state through the existing write module, or a desktop setting",
    //   and this is neither.
    // - Apply `Local`'s stated test: could the phone act on the answer?
    //   Installing hooks only matters for sessions started at that
    //   desktop's keyboard, so nobody needs to install one from a phone.
    //   And the REFUSAL cases need a human reading an explanation at the
    //   machine with the broken file -- a settings file Claude Code
    //   cannot parse is ignored silently, which is a conversation, not a
    //   status code.
    // - `Destructive` would technically fit uninstall, and its biometric
    //   step-up is the wrong frame: the risk here is not deletion, it is
    //   corrupting another tool's config remotely with no way to see the
    //   result. Refusing outright beats gating.
    //
    // `claude_hooks_status` above stays `Read`, because reading is the
    // half that remains useful without being able to act.
    ("claude_install_hooks", Class::Local),
    ("claude_reinstall_hooks", Class::Local),
    ("claude_uninstall_hooks", Class::Local),
];

/// The class of a registered command, or `None` when no such command
/// exists.
pub fn class_of(command: &str) -> Option<Class> {
    SURFACE
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, class)| *class)
}

/// Why a remote call was not carried out, or did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// No registered command has this name. Suggested status: 404.
    #[error("`{0}` is not a Headstate command")]
    Unknown(String),
    /// The command exists but is desktop-only. Suggested status: 403.
    #[error("`{0}` is only available on the desktop")]
    Local(String),
    /// The body did not decode into the command's arguments. Suggested
    /// status: 400.
    #[error("bad arguments for `{command}`: {message}")]
    BadArgs { command: String, message: String },
    /// The command ran and rejected. The message is verbatim what the
    /// webview would have seen, and `kind` is that rejection classified
    /// on this side rather than guessed back on the other one (#1202).
    /// Suggested status: 500.
    ///
    /// `Display` is the message ALONE, so `to_string()` is byte-identical
    /// to what this variant produced when it held a bare `String`. Every
    /// existing reader of the plain-text refusal keeps working unchanged.
    #[error("{}", .0.message)]
    Command(CommandError),
}

impl RemoteError {
    /// The statuses suggested on each variant, for the `/v1/call`
    /// handler in `remote/listener.rs`.
    pub fn http_status(&self) -> u16 {
        match self {
            RemoteError::Unknown(_) => 404,
            RemoteError::Local(_) => 403,
            RemoteError::BadArgs { .. } => 400,
            RemoteError::Command(_) => 500,
        }
    }
}

/// The gate `dispatch` applies before touching any argument: known and
/// not local, or a refusal naming why. Separate from `dispatch` so the
/// refusals are testable without an `AppHandle`.
fn admit(command: &str) -> Result<Class, RemoteError> {
    match class_of(command) {
        None => Err(RemoteError::Unknown(command.to_string())),
        Some(Class::Local) => Err(RemoteError::Local(command.to_string())),
        Some(class) => Ok(class),
    }
}

/// The JSON body of a call, read the way Tauri's IPC reads it: one key per
/// argument, camelCase, with a missing key acceptable only for an
/// `Option` argument.
struct Args<'a> {
    command: &'a str,
    body: Value,
}

impl<'a> Args<'a> {
    fn new(command: &'a str, body: Value) -> Result<Self, RemoteError> {
        match body {
            Value::Object(_) => Ok(Self { command, body }),
            Value::Null => Ok(Self {
                command,
                body: Value::Object(Default::default()),
            }),
            other => Err(RemoteError::BadArgs {
                command: command.to_string(),
                message: format!("expected a JSON object of arguments, got {other}"),
            }),
        }
    }

    /// One argument by its camelCase key. A missing key decodes as JSON
    /// `null`, which is `None` for an `Option<T>` and an error naming the
    /// key for anything else, matching what Tauri reports.
    fn get<T: DeserializeOwned>(&self, key: &str) -> Result<T, RemoteError> {
        let value = self.body.get(key).cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|e| RemoteError::BadArgs {
            command: self.command.to_string(),
            message: if self.body.get(key).is_none() {
                format!("missing required argument `{key}`")
            } else {
                format!("argument `{key}`: {e}")
            },
        })
    }
}

/// A command's plain return value, as the webview would receive it.
fn ok<T: Serialize>(value: T) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|e| RemoteError::Command(CommandError::classify(e.to_string())))
}

/// A command's `Result<T, String>`, as the webview would receive it.
///
/// The single place a command's own rejection becomes a `RemoteError`,
/// which is why classifying here covers all 103 dispatch arms without
/// touching one command signature (#1202).
fn res<T: Serialize>(result: Result<T, String>) -> Result<Value, RemoteError> {
    result
        .map_err(|m| RemoteError::Command(CommandError::classify(m)))
        .and_then(ok)
}

/// Run one allowlisted command on behalf of a paired device.
///
/// Refuses [`Class::Local`] and unknown commands, decodes `args` into the
/// same argument shapes the webview sends, and calls the same
/// `commands::*` function the webview would have called, with the same
/// managed state. Each command keeps its own logging; this adds one line
/// per call naming the device, so a log reads "phone asked, desktop did".
///
/// Does NOT verify the step-up signature for destructive commands. The
/// HTTP handler must do that first; see the module docs.
pub async fn dispatch(
    app: &AppHandle,
    command: &str,
    args: Value,
    device_name: &str,
) -> Result<Value, RemoteError> {
    let class = admit(command)?;
    log::info!("remote: {device_name} called {command} ({class:?})");
    let result = call(app, command, Args::new(command, args)?).await;
    if let Err(e) = &result {
        log::warn!("remote: {command} for {device_name} failed: {e}");
    }
    result
}

/// The match over the allowlist. Every non-local row of [`SURFACE`] has an
/// arm here, which [`tests::every_remote_command_has_a_dispatch_arm`]
/// enforces on the source.
///
/// Managed state is reached through `app.state()`, which yields the same
/// `State<'_, T>` Tauri injects into the command; `AppHandle` arguments
/// get a clone of `app`. Argument keys are the camelCase names the
/// webview sends, so `repo_path` on the Rust side is `"repoPath"` here.
async fn call(app: &AppHandle, command: &str, a: Args<'_>) -> Result<Value, RemoteError> {
    match command {
        // ---- read -------------------------------------------------------
        "background_panicked" => ok(commands::background_panicked()),
        "background_health" => ok(commands::background_health()),
        "tool_versions" => res(commands::tool_versions().await),
        // `maxBytes` is optional: a missing key decodes as `None`, and
        // the command applies its own default and its own ceiling, so a
        // phone cannot ask for a larger payload than the desktop would.
        "read_log_tail" => res(commands::read_log_tail(app.clone(), a.get("maxBytes")?).await),
        "get_auth_state" => ok(commands::get_auth_state(app.state())),
        "get_cached" => res(commands::get_cached(app.clone())),
        "get_cached_reviewing" => res(commands::get_cached_reviewing(app.clone())),
        "refresh_now" => res(commands::refresh_now(app.state()).await),
        "get_stats" => res(commands::get_stats(app.state()).await),
        "get_history" => res(commands::get_history(app.state(), a.get("days")?).await),
        "get_periods" => res(commands::get_periods(app.state()).await),
        "get_cycle_trend" => res(commands::get_cycle_trend(app.state()).await),
        "get_merged_detail" => res(commands::get_merged_detail(app.state()).await),
        "stats_count" => res(commands::stats_count(
            app.clone(),
            app.state(),
            a.get("subject")?,
            a.get("scopeKind")?,
            a.get("scopeValue")?,
            a.get("measure")?,
            a.get("days")?,
        )
        .await),
        "stats_tree" => res(commands::stats_tree(app.state()).await),
        // No `subject`, deliberately, and not an omission: a board asks
        // about everyone in the scope, and a subject qualifier would render
        // a leaderboard with one name on it. The viewer's login comes back
        // IN the answer so the caller can split Mine from Others.
        "stats_board" => res(commands::stats_board(
            app.clone(),
            app.state(),
            a.get("scopeKind")?,
            a.get("scopeValue")?,
            a.get("measure")?,
            a.get("days")?,
        )
        .await),
        "stats_series" => res(commands::stats_series(
            app.clone(),
            app.state(),
            a.get("subject")?,
            a.get("scopeKind")?,
            a.get("scopeValue")?,
            a.get("days")?,
        )
        .await),
        "stats_reviewers" => res(commands::stats_reviewers(
            app.state(),
            a.get("scopeKind")?,
            a.get("scopeValue")?,
            // A `Vec<String>`, which `Args::get` decodes like any other
            // shape: the key is the camelCase name the webview sends, and a
            // missing one is an error naming it rather than a silent empty
            // list that would produce an empty leaderboard.
            a.get("logins")?,
            a.get("days")?,
        )
        .await),
        "get_reviewing" => res(commands::get_reviewing(app.clone(), app.state()).await),
        "count_reviewing" => res(commands::count_reviewing(app.state()).await),
        "get_pr_detail" => {
            res(commands::get_pr_detail(app.state(), a.get("repo")?, a.get("number")?).await)
        }
        "get_viewer" => res(commands::get_viewer(app.state()).await),
        "build_target" => ok(commands::build_target()),
        "latest_release" => ok(commands::latest_release(app.clone()).await),
        "list_worktrees" => res(commands::list_worktrees(app.clone()).await),
        // The repository browser (#1031, #1033). `app.clone()` because
        // both commands re-derive the repository root against
        // `list_worktrees` before touching a path -- the phone's caller
        // does not get to name a root the desktop's current scan does not
        // hold.
        "repo_tree" => {
            res(commands::repo_tree(app.clone(), a.get("repoPath")?, a.get("path")?).await)
        }
        "repo_file" => {
            res(commands::repo_file(app.clone(), a.get("repoPath")?, a.get("path")?).await)
        }
        // `app.clone()` since #830, exactly as `size_worktrees` below:
        // the command now emits `worktree-safety` per worktree, and the
        // handle is what carries those events to this phone through the
        // hub. A phone that only awaited the return value would see the
        // whole repository land at once -- which is the wait #830 is
        // about, and the client least able to wait it out.
        "classify_worktrees" => {
            res(commands::classify_worktrees(app.clone(), a.get("repoPath")?).await)
        }
        // No `app` handle, unlike the arm above: this command emits no
        // events. Its unit of work is ONE worktree, so the promise
        // settling is the answer arriving -- there is nothing partial to
        // stream (#1042).
        "classify_repo_upstream" => res(commands::classify_repo_upstream(a.get("repoPath")?).await),
        "size_worktrees" => res(commands::size_worktrees(app.clone(), a.get("repoPath")?).await),
        "list_branches" => res(commands::list_branches(app.clone(), a.get("repoPath")?).await),
        "scan_artifacts" => res(commands::scan_artifacts(app.clone()).await),
        "read_cached_scan" => res(commands::read_cached_scan(app.clone(), a.get("kind")?).await),
        "size_artifacts" => res(commands::size_artifacts(a.get("paths")?).await),
        "scan_venvs" => res(commands::scan_venvs(app.clone()).await),
        "size_venvs" => res(commands::size_venvs(a.get("paths")?).await),
        "docker_state" => ok(commands::docker_state().await),
        "docker_builds" => res(commands::docker_builds().await),
        "docker_images" => res(commands::docker_images(app.clone()).await),
        "system_health" => res(commands::system_health(app.state()).await),
        "system_health_history" => res(commands::system_health_history(app.clone()).await),
        "health_alerts" => res(commands::health_alerts(app.clone()).await),
        "system_footprint" => res(commands::system_footprint(app.state()).await),
        "system_network_processes" => res(commands::system_network_processes().await),
        "docker_disk_usage" => res(commands::docker_disk_usage().await),
        // These two shell out and would stall the listener's worker for
        // every other request (#496 was this bug in the webview). The
        // `blocking()` wrapper that used to live here is gone because
        // #1090 moved the `spawn_blocking` INTO the commands themselves:
        // the desktop called the same two functions synchronously and
        // froze its own UI, so wrapping only the phone's arm fixed half
        // the defect. Both callers now get the same treatment.
        "docker_dangling_volumes" => res(commands::docker_dangling_volumes().await),
        "docker_running_containers" => res(commands::docker_running_containers().await),
        "preview_cleanup" => res(commands::preview_cleanup(app.clone()).await),
        "cleanup_log" => res(commands::cleanup_log(app.clone())),
        "get_cleanup_prefs" => ok(commands::get_cleanup_prefs(app.clone())),
        // `async` since #1090: it ran `git rev-parse HEAD` once per
        // assessed worktree inline on this listener's worker.
        "assessed_worktrees" => ok(commands::assessed_worktrees(app.clone()).await),
        "assess_worktree" => res(commands::assess_worktree(
            a.get("repoPath")?,
            a.get("worktreePath")?,
            a.get("branch")?,
        )
        .await),
        "claudify_command" => ok(commands::claudify_command(
            a.get("repoPath")?,
            a.get("worktreePath")?,
            a.get("branch")?,
        )),
        // The four `claude_launch*` commands have no arm: they are
        // `Class::Local` and `admit` rejects them before this match is
        // reached. `claude_launch_terms` is `Class::Read`, so it does.
        "claude_launch_terms" => ok(commands::claude_launch_terms()),
        "check_packages" => res(commands::check_packages(a.get("repoPath")?).await),
        "packages_markdown" => ok(commands::packages_markdown(
            a.get("repoPath")?,
            a.get("reports")?,
            a.get("filter")?,
        )),
        "claude_md_effective" => res(commands::claude_md_effective(a.get("repoPath")?).await),
        "claude_md_advice" => {
            res(commands::claude_md_advice(app.clone(), a.get("repoPath")?, a.get("mode")?).await)
        }
        "scan_claude_md" => res(commands::scan_claude_md(a.get("repoPath")?).await),
        // `async` since #1090: an unbounded `read_to_string` dispatched
        // inline held this listener for the length of the file.
        "read_claude_md" => res(commands::read_claude_md(a.get("path")?).await),
        "claude_import_transcripts" => res(commands::claude_import_transcripts(app.clone()).await),
        "claude_search_transcripts" => {
            res(
                commands::claude_search_transcripts(app.clone(), a.get("query")?, a.get("limit")?)
                    .await,
            )
        }
        "claude_index_coverage" => res(commands::claude_index_coverage(app.clone()).await),
        "claude_sessions" => res(commands::claude_sessions(app.clone()).await),
        "claude_sessions_for_pr" => {
            res(
                commands::claude_sessions_for_pr(app.clone(), a.get("repo")?, a.get("number")?)
                    .await,
            )
        }
        "claude_session_detail" => {
            res(commands::claude_session_detail(app.clone(), a.get("sessionId")?).await)
        }
        "claude_subagent_rollup" => {
            res(commands::claude_subagent_rollup(app.clone(), a.get("sessionId")?).await)
        }
        "claude_session_events" => {
            res(commands::claude_session_events(app.clone(), a.get("sessionId")?).await)
        }
        "claude_event_profile" => res(commands::claude_event_profile(app.clone()).await),
        "claude_poll_live" => res(commands::claude_poll_live(app.clone()).await),
        "claude_overview" => res(commands::claude_overview(app.clone()).await),
        "claude_coverage" => res(commands::claude_coverage(app.clone()).await),
        "claude_definitions" => res(commands::claude_definitions(app.clone()).await),
        "claude_plugins" => res(commands::claude_plugins(app.clone()).await),
        "claude_restart_list" => res(commands::claude_restart_list(app.clone()).await),
        "claude_usage_profile" => res(commands::claude_usage_profile(app.clone()).await),
        "claude_session_usage" => res(commands::claude_session_usage(a.get("path")?).await),
        "claude_transcript_tail" => res(commands::claude_transcript_tail(a.get("path")?).await),
        "claude_transcript_follow" => {
            res(commands::claude_transcript_follow(a.get("path")?, a.get("cursor")?).await)
        }
        "claude_hooks_inventory" => res(commands::claude_hooks_inventory()),
        "claude_effective_settings" => {
            res(commands::claude_effective_settings(a.get("repoPath")?).await)
        }
        "claude_config_health" => res(commands::claude_config_health(app.clone()).await),
        "claude_mcp_servers" => res(commands::claude_mcp_servers().await),
        "claude_permission_ownership" => {
            res(commands::claude_permission_ownership(app.clone()).await)
        }
        "claude_hooks_status" => res(commands::claude_hooks_status()),
        "get_poll_interval" => ok(commands::get_poll_interval(app.state())),
        "get_worktree_dirs" => ok(commands::get_worktree_dirs(app.clone())),
        "get_ui_prefs" => ok(commands::get_ui_prefs(app.clone())),
        "update_run_state" => ok(commands::update_run_state(app.state(), a.get("repoPath")?)),
        "update_all_state" => ok(commands::update_all_state(app.state())),

        // ---- write ------------------------------------------------------
        "act_on_pr" => res(commands::act_on_pr(
            app.state(),
            app.state(),
            a.get("id")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("action")?,
        )
        .await),
        "act_on_prs" => {
            res(
                commands::act_on_prs(app.state(), app.state(), a.get("prs")?, a.get("action")?)
                    .await,
            )
        }
        "review_pr" => res(commands::review_pr(
            app.state(),
            app.state(),
            a.get("id")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("verdict")?,
            a.get("body")?,
        )
        .await),
        "comment_on_pr" => res(commands::comment_on_pr(
            app.state(),
            a.get("id")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("body")?,
        )
        .await),
        "resolve_thread" => res(commands::resolve_thread(
            app.state(),
            a.get("threadId")?,
            a.get("repo")?,
            a.get("number")?,
        )
        .await),
        "unresolve_thread" => res(commands::unresolve_thread(
            app.state(),
            a.get("threadId")?,
            a.get("repo")?,
            a.get("number")?,
        )
        .await),
        "reply_to_thread" => res(commands::reply_to_thread(
            app.state(),
            a.get("threadId")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("body")?,
        )
        .await),
        "rerun_checks" => res(commands::rerun_checks(
            app.state(),
            app.state(),
            a.get("repo")?,
            a.get("number")?,
            a.get("runId")?,
        )
        .await),
        "update_pr_branch" => res(commands::update_pr_branch(
            app.state(),
            app.state(),
            a.get("id")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("expectedHead")?,
        )
        .await),
        "set_auto_merge" => res(commands::set_auto_merge(
            app.state(),
            app.state(),
            a.get("id")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("expectedHead")?,
            a.get("enable")?,
        )
        .await),
        "mark_assessed" => res(commands::mark_assessed(app.clone(), a.get("worktreePath")?)),
        "clear_assessed" => res(commands::clear_assessed(
            app.clone(),
            a.get("worktreePath")?,
        )),
        "set_cleanup_prefs" => res(commands::set_cleanup_prefs(app.clone(), a.get("prefs")?)),
        "set_ui_prefs" => res(commands::set_ui_prefs(app.clone(), a.get("prefs")?)),
        "apply_updates_in_background" => res(commands::apply_updates_in_background(
            app.clone(),
            app.state(),
            a.get("repoPath")?,
            a.get("requests")?,
            a.get("branch")?,
        )
        .await),
        "cancel_update_run" => res(commands::cancel_update_run(app.state(), a.get("repoPath")?)),
        "pull_checkout" => res(commands::pull_checkout(a.get("path")?).await),
        // No arguments: the set comes from the desktop's own scan roots,
        // re-derived inside the command, never from the caller. See the
        // command's doc -- a caller-supplied path list would make this a
        // way to pull arbitrary directories.
        "update_all_repositories" => res(commands::update_all_repositories(app.clone()).await),
        "cancel_update_all" => res(commands::cancel_update_all(app.state())),
        "fetch_refs" => res(commands::fetch_refs(a.get("path")?).await),
        // Shell out like the other sync Docker commands, so a slow
        // engine start does not stall the listener for everyone else.
        "docker_start" => res(blocking(commands::docker_start).await?),
        "docker_restart" => res(blocking(commands::docker_restart).await?),
        "set_view_needs_github" => {
            commands::set_view_needs_github(a.get("needs")?, app.state(), app.state());
            ok(())
        }
        "set_poll_interval" => ok(commands::set_poll_interval(
            app.clone(),
            a.get("secs")?,
            app.state(),
            app.state(),
        )),
        // `open_update_pr`'s arm went with its row in `SURFACE` (#964).
        // `every_remote_command_has_a_dispatch_arm` reads that table, so
        // the two must move together: a row with no arm fails it, and an
        // arm with no row is dead but harmless -- `dispatch` is only ever
        // reached through `admit`. Removed anyway, because an arm nothing
        // can route to is the next reader's puzzle.

        // ---- destructive (signature already verified by the handler) ----
        "delete_head_branch" => res(commands::delete_head_branch(
            app.state(),
            app.state(),
            a.get("refId")?,
            a.get("repo")?,
            a.get("number")?,
            a.get("branch")?,
            a.get("merged")?,
        )
        .await),
        "delete_branches" => {
            res(commands::delete_branches(app.clone(), a.get("repoPath")?, a.get("names")?).await)
        }
        "delete_remote_branches" => {
            res(
                commands::delete_remote_branches(app.clone(), a.get("repoPath")?, a.get("names")?)
                    .await,
            )
        }
        "remove_worktree" => {
            res(
                commands::remove_worktree(app.state(), a.get("repoPath")?, a.get("worktreePath")?)
                    .await,
            )
        }
        "remove_worktrees" => res(commands::remove_worktrees(
            app.clone(),
            a.get("repoPath")?,
            a.get("worktreePaths")?,
        )
        .await),
        "remove_worktree_forced" => res(commands::remove_worktree_forced(
            app.clone(),
            a.get("repoPath")?,
            a.get("worktreePath")?,
        )
        .await),
        "unlock_worktree" => {
            res(commands::unlock_worktree(a.get("repoPath")?, a.get("worktreePath")?).await)
        }
        // Beside the other worktree arms rather than up in the write
        // block, because the repository-path argument and the
        // re-classify-afterwards contract are what it shares -- it is
        // `Class::Write` and carries no step-up signature. Takes no
        // worktree path: `git worktree prune` is repo-wide.
        "prune_worktrees" => res(commands::prune_worktrees(a.get("repoPath")?).await),
        "remove_artifacts" => res(commands::remove_artifacts(app.clone(), a.get("paths")?).await),
        "remove_venvs" => res(commands::remove_venvs(app.clone(), a.get("paths")?).await),
        // `app.clone()` like the two rows above, since #854: the scan
        // roots it checks containment against come from settings rather
        // than from this request, which is what makes the path a paired
        // peer sends unable to name a directory outside them.
        "remove_orphan" => res(commands::remove_orphan(app.clone(), a.get("path")?).await),
        "docker_remove_images" => {
            let ids: Vec<String> = a.get("ids")?;
            ok(blocking(move || commands::docker_remove_images(ids)).await?)
        }
        "docker_remove_volume" => {
            let name: String = a.get("name")?;
            res(blocking(move || commands::docker_remove_volume(name)).await?)
        }
        "docker_prune_cache" => {
            let until: Option<String> = a.get("until")?;
            res(blocking(move || commands::docker_prune_cache(until)).await?)
        }
        // `apply_package_updates`' arm went with its row too (#964).

        // A classified, non-local command with no arm is a wiring bug
        // that the source test catches; at runtime it must still refuse
        // rather than pretend.
        _ => Err(RemoteError::Unknown(command.to_string())),
    }
}

/// Run a synchronous, shelling-out command on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, RemoteError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| {
        RemoteError::Command(CommandError::classify(format!("command task failed: {e}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `path::name` entries inside `generate_handler![...]` in
    /// `lib.rs`, read from the source at test time so the desktop's real
    /// registration is what is compared, not a copy of it. Any module
    /// path counts -- `commands::x` and `remote::pairing::x` alike --
    /// because a command registered under a path the parser ignored
    /// would be neither classified nor caught.
    fn registered_commands() -> Vec<String> {
        let src = include_str!("../lib.rs");
        let open = "generate_handler![";
        let start = src
            .find(open)
            .expect("lib.rs must register commands with generate_handler!");
        let block = &src[start + open.len()..];
        let end = block.find(']').expect("generate_handler! block must close");
        block[..end]
            .split_whitespace()
            .map(|tok| tok.trim_end_matches(','))
            .filter(|tok| tok.contains("::"))
            .filter_map(|tok| tok.rsplit("::").next())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_parser_sees_commands_under_every_module_path() {
        let registered = registered_commands();
        for name in ["get_cached", "issue_pairing_token", "set_remote_enabled"] {
            assert!(registered.iter().any(|r| r == name), "{name} not parsed");
        }
    }

    #[test]
    fn every_registered_command_has_exactly_one_class() {
        let registered = registered_commands();
        assert!(
            registered.len() > 50,
            "parsed only {} commands from lib.rs; the parser is broken",
            registered.len()
        );
        let mut problems = Vec::new();
        for name in &registered {
            let rows = SURFACE.iter().filter(|(n, _)| n == name).count();
            if rows != 1 {
                problems.push(format!(
                    "{name}: registered in lib.rs but has {rows} rows in SURFACE (need exactly 1)"
                ));
            }
        }
        for (name, _) in SURFACE {
            if !registered.iter().any(|r| r == name) {
                problems.push(format!(
                    "{name}: classified in SURFACE but not registered in lib.rs"
                ));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
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
    fn local_commands_are_refused() {
        for name in [
            "reveal_log",
            "set_autostart",
            "set_worktree_dirs",
            "issue_pairing_token",
            "respond_to_pairing",
            "list_paired_devices",
            "revoke_paired_device",
            "get_remote_enabled",
            "set_remote_enabled",
        ] {
            assert_eq!(
                admit(name),
                Err(RemoteError::Local(name.to_string())),
                "{name} must be refused as local"
            );
        }
    }

    /// The commands v5.5.0 moved OFF `Class::Local`.
    ///
    /// Kept as its own test rather than folded into the admission test
    /// above, because the reason they moved is a product decision and
    /// not an implementation detail: driving the desktop is what the
    /// companion is for, and a phone that could see a stopped Docker
    /// engine but not start it was offering a button that could only
    /// ever fail. Anything that moves one of these back to `Local` is
    /// removing a feature, and should have to say so here.
    #[test]
    fn the_companion_may_drive_the_desktop() {
        for (name, class) in [
            ("assess_worktree", Class::Read),
            ("claudify_command", Class::Read),
            ("get_ui_prefs", Class::Read),
            ("update_run_state", Class::Read),
            ("pull_checkout", Class::Write),
            // Never Local at any point -- it was born exposed (#788) --
            // but it belongs in this list for the same reason the rest
            // do: moving it to Local would remove the companion's only
            // way to refresh a stale comparison, and that is a product
            // decision that should have to be argued here.
            ("fetch_refs", Class::Write),
            ("docker_start", Class::Write),
            ("docker_restart", Class::Write),
            ("set_ui_prefs", Class::Write),
            ("set_view_needs_github", Class::Write),
            ("apply_updates_in_background", Class::Write),
            ("cancel_update_run", Class::Write),
            // Update All (#1019). Listed here with its two companions
            // because the three are inseparable: the `Write` row was
            // argued on the condition that the run can be STOPPED and its
            // outcome read back after a suspension, so moving either of
            // the other two to `Local` would silently withdraw the
            // premise this row rests on.
            ("update_all_repositories", Class::Write),
            ("cancel_update_all", Class::Write),
            ("update_all_state", Class::Read),
        ] {
            assert_eq!(
                admit(name),
                Ok(class),
                "{name} must be drivable from a phone"
            );
        }
    }

    /// Update All is exposed ONLY because it can be stopped and read back
    /// (#1019), borrowing `apply_updates_in_background`'s own condition:
    /// *starting something you cannot stop or see the end of is not a
    /// feature*.
    ///
    /// MEASURED, the worst case is 45 x `GIT_TIMEOUT` ~= 22 minutes, and
    /// a suspended phone holds no event stream at all -- so a phone that
    /// could start the run but neither stop it nor learn how it ended is
    /// exactly what that sentence refuses. This test is the gate: remove
    /// either companion from the surface and the bulk row must go with
    /// it.
    #[test]
    fn update_all_is_exposed_only_alongside_its_stop_and_its_readback() {
        if admit("update_all_repositories").is_ok() {
            assert!(
                admit("cancel_update_all").is_ok(),
                "a run a phone cannot stop must not be exposed to a phone"
            );
            assert!(
                admit("update_all_state").is_ok(),
                "a run whose end a phone cannot read must not be exposed to a phone"
            );
        }
    }

    #[test]
    fn each_error_maps_to_the_status_its_docs_suggest() {
        assert_eq!(RemoteError::Unknown("x".into()).http_status(), 404);
        assert_eq!(RemoteError::Local("x".into()).http_status(), 403);
        let bad = RemoteError::BadArgs {
            command: "x".into(),
            message: "m".into(),
        };
        assert_eq!(bad.http_status(), 400);
        assert_eq!(
            RemoteError::Command(CommandError::classify("m")).http_status(),
            500
        );
    }

    #[test]
    fn unknown_commands_are_refused() {
        for name in ["", "drop_database", "getCached", "commands::get_cached"] {
            assert_eq!(admit(name), Err(RemoteError::Unknown(name.to_string())));
        }
    }

    #[test]
    fn read_write_and_destructive_commands_are_admitted() {
        assert_eq!(admit("get_cached"), Ok(Class::Read));
        assert_eq!(admit("act_on_pr"), Ok(Class::Write));
        assert_eq!(admit("remove_worktree"), Ok(Class::Destructive));
    }

    /// Every non-local row must have a `"name" =>` arm in `call`. Asserted
    /// on the source because `call` needs a live `AppHandle`, which a unit
    /// test cannot construct for the Wry runtime the commands are typed
    /// against.
    #[test]
    fn every_remote_command_has_a_dispatch_arm() {
        let src = include_str!("surface.rs");
        let start = src.find("async fn call(").expect("call must exist");
        let end = src[start..]
            .find("#[cfg(test)]")
            .map(|i| start + i)
            .unwrap_or(src.len());
        let body = &src[start..end];
        let missing: Vec<&str> = SURFACE
            .iter()
            .filter(|(_, class)| *class != Class::Local)
            .map(|(name, _)| *name)
            .filter(|name| !body.contains(&format!("\"{name}\" =>")))
            .collect();
        assert!(
            missing.is_empty(),
            "classified as remote but not dispatched: {}",
            missing.join(", ")
        );
        let local_with_arm: Vec<&str> = SURFACE
            .iter()
            .filter(|(_, class)| *class == Class::Local)
            .map(|(name, _)| *name)
            .filter(|name| body.contains(&format!("\"{name}\" =>")))
            .collect();
        assert!(
            local_with_arm.is_empty(),
            "local commands must not have an arm: {}",
            local_with_arm.join(", ")
        );
    }

    /// The webview sends camelCase keys (`repoPath`), and Tauri's default
    /// argument renaming expects them. A snake_case key in an arm would
    /// compile and then fail every call with "missing required argument".
    #[test]
    fn dispatch_arms_read_camel_case_keys_only() {
        let src = include_str!("surface.rs");
        let start = src.find("async fn call(").expect("call must exist");
        let end = src[start..].find("#[cfg(test)]").unwrap() + start;
        let snake: Vec<&str> = src[start..end]
            .split(".get(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|key| key.contains('_'))
            .collect();
        assert!(snake.is_empty(), "snake_case argument keys: {snake:?}");
    }

    // One command per class, decoded from exactly the body its `tauri.ts`
    // wrapper sends.

    #[test]
    fn read_args_decode_as_the_webview_sends_them() {
        let a = Args::new("get_history", json!({ "days": 14 })).unwrap();
        assert_eq!(a.get::<i64>("days"), Ok(14));
    }

    #[test]
    fn write_args_decode_as_the_webview_sends_them() {
        let a = Args::new(
            "act_on_pr",
            json!({ "id": "PR_1", "repo": "octocat/hello-world", "number": 7, "action": "merge" }),
        )
        .unwrap();
        assert_eq!(a.get::<String>("id"), Ok("PR_1".into()));
        assert_eq!(a.get::<String>("repo"), Ok("octocat/hello-world".into()));
        assert_eq!(a.get::<u64>("number"), Ok(7));
        assert_eq!(a.get::<String>("action"), Ok("merge".into()));
    }

    #[test]
    fn destructive_args_decode_as_the_webview_sends_them() {
        let a = Args::new(
            "remove_worktrees",
            json!({ "repoPath": "/srv/hello-world", "worktreePaths": ["/srv/hello-world-wt"] }),
        )
        .unwrap();
        assert_eq!(a.get::<String>("repoPath"), Ok("/srv/hello-world".into()));
        assert_eq!(
            a.get::<Vec<String>>("worktreePaths"),
            Ok(vec!["/srv/hello-world-wt".to_string()])
        );
    }

    #[test]
    fn a_missing_required_argument_names_the_key() {
        let a = Args::new("remove_worktree", json!({ "repoPath": "/srv/x" })).unwrap();
        assert_eq!(
            a.get::<String>("worktreePath"),
            Err(RemoteError::BadArgs {
                command: "remove_worktree".into(),
                message: "missing required argument `worktreePath`".into(),
            })
        );
    }

    #[test]
    fn a_missing_optional_argument_is_none() {
        let a = Args::new("docker_prune_cache", json!({})).unwrap();
        assert_eq!(a.get::<Option<String>>("until"), Ok(None));
        let a = Args::new("docker_prune_cache", json!({ "until": "24h" })).unwrap();
        assert_eq!(a.get::<Option<String>>("until"), Ok(Some("24h".into())));
    }

    #[test]
    fn a_wrongly_typed_argument_is_refused_with_the_key() {
        let a = Args::new("get_history", json!({ "days": "fourteen" })).unwrap();
        let err = a.get::<i64>("days").unwrap_err();
        match err {
            RemoteError::BadArgs { command, message } => {
                assert_eq!(command, "get_history");
                assert!(message.starts_with("argument `days`:"), "{message}");
            }
            other => panic!("expected BadArgs, got {other:?}"),
        }
    }

    #[test]
    fn a_null_body_is_an_empty_argument_set_and_a_non_object_is_refused() {
        assert!(Args::new("get_cached", Value::Null).is_ok());
        assert!(Args::new("get_cached", json!([1, 2])).is_err());
        assert!(Args::new("get_cached", json!("x")).is_err());
    }

    #[test]
    fn a_command_error_is_passed_through_verbatim() {
        let r: Result<(), String> = Err(commands::AUTH_ERR.to_string());
        assert_eq!(
            res(r),
            Err(RemoteError::Command(CommandError::classify(
                commands::AUTH_ERR
            )))
        );
        assert_eq!(res(Ok(("a".to_string(), 1u64))), Ok(json!(["a", 1])));
    }

    /// The two superseded update commands are off the wire (#964).
    ///
    /// Named explicitly rather than left to
    /// `every_registered_command_has_exactly_one_class`, which asserts
    /// the two lists AGREE and is therefore satisfied by putting both
    /// rows back alongside both registrations. This says which direction
    /// the agreement must be reached in: these two are gone, and a
    /// re-registration is a decision to make deliberately rather than by
    /// restoring a line that looked missing.
    ///
    /// The evidence they are dead is in the issue: `apply_updates_in_
    /// background` supersedes both, and it is asserted present below so
    /// this cannot pass by the whole flow having been deleted.
    #[test]
    fn the_superseded_update_commands_are_not_on_the_remote_surface() {
        for gone in ["apply_package_updates", "open_update_pr"] {
            assert_eq!(
                class_of(gone),
                None,
                "{gone} is classified again; #964 removed it because #626 \
                 left it with no desktop caller while a paired phone could \
                 still dispatch it. Re-adding it needs a desktop route first."
            );
        }
        // The flow that replaced them, so this test fails if the removal
        // is ever "fixed" by deleting the update feature instead.
        assert_eq!(
            class_of("apply_updates_in_background"),
            Some(Class::Write),
            "the flow that supersedes them must still be reachable"
        );
    }

    /// And the wrapper functions themselves are gone from `commands.rs`,
    /// not merely unregistered.
    ///
    /// A `#[tauri::command]` left in place with no entry in
    /// `generate_handler!` is invisible to every guard in this file --
    /// they all read the registration list -- and it is the shape the
    /// next person re-registers by accident, because the function is
    /// sitting there looking like a command. So the source is read
    /// directly, the same technique `registered_commands` above uses.
    ///
    /// `open_update_pr_inner` must SURVIVE: it is the live code
    /// `apply_updates_in_background` calls, and a fix that removed it
    /// would take the pull-request half of the update flow with it.
    #[test]
    fn the_superseded_wrappers_are_gone_from_commands_rs_but_the_helper_is_not() {
        let src = include_str!("../commands.rs");
        for gone in ["apply_package_updates", "open_update_pr"] {
            let sig = format!("pub async fn {gone}(");
            assert!(
                !src.contains(&sig),
                "commands.rs still defines `{sig}`; an unregistered \
                 command is invisible to every guard here and is how it \
                 gets re-registered by accident"
            );
        }
        assert!(
            src.contains("pub(crate) async fn open_update_pr_inner("),
            "open_update_pr_inner is the live code apply_updates_in_background \
             calls; removing it would take the pull-request half of the \
             update flow with it"
        );
    }
}
