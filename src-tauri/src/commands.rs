//! The Tauri command surface. React never talks to GitHub directly -- it
//! calls these commands and listens for the `prs-updated` event that
//! [`crate::poll`] emits in the background.

use crate::github::client::{ClientError, GitHubClient};
use crate::github::model::{
    CycleTrend, History, MergedDetail, Periods, PrDetail, PullRequest, Stats,
};
use crate::github::mutate::{PrAction, ReviewVerdict};
use crate::store::{
    load_snapshot, load_snapshot_marked, open_db, save_snapshot, settings, CachedList,
    CachedSnapshot,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Enough for the UI to render a real first-run screen (e.g. "install gh
/// and run `gh auth login`") rather than a generic error. `message` is
/// already display-ready prose from `gh`'s own stderr when auth failed; it
/// is never re-wrapped or parsed, and it never contains the token itself.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthState {
    pub ok: bool,
    pub message: String,
}

/// Managed state wrapping the client. `None` when startup auth failed (no
/// `gh` token available), so commands that need GitHub can fail with a
/// clear message instead of the generic "state not managed" error Tauri
/// would otherwise return if the client type were unmanaged entirely.
pub struct GhClient(pub Option<Arc<GitHubClient>>);

/// Shown verbatim when no client exists. Duplicated across five commands
/// before this; a const means the five cannot drift apart.
pub const AUTH_ERR: &str = "not authenticated: run `gh auth login`";

/// Bound the history window.
///
/// The UI only offers 7/14/30, but a Tauri command is a public surface: an
/// unbounded value builds an arbitrarily large query and, since the fetch
/// chunks by day, spawns roughly `days / HISTORY_CHUNK_DAYS` concurrent
/// requests. Extracted from `get_history` so it can be tested -- deleting
/// the clamp there left all frontend and Rust tests passing while
/// `get_history(10000)` spawned ~2000 chunks.
pub fn clamp_days(days: i64) -> i64 {
    days.clamp(1, 90)
}

pub fn db_path(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("headstate.db")
}

/// DIAGNOSTIC COMMAND (Settings > diagnostic log).
///
/// Lets the frontend write into the same log file as the Rust side, so
/// one file shows the whole path in order: React deciding to fetch, the
/// command running, the HTTP request, and React settling. Without a
/// shared timeline the two halves cannot be lined up, and the open
/// question is precisely WHICH half the missing minute is in.
///
/// Takes an already-formatted line rather than structured fields: every
/// caller is in this repo and passes counts and timings only.
///
/// The line is dropped when diagnostics are off, so the frontend does
/// not need its own copy of the flag -- one source of truth for one
/// setting.
#[tauri::command]
pub fn diag_log(line: String) {
    // Truncated: a log line is not a channel for page content, and a
    // bounded length means a runaway caller cannot fill the disk.
    let line: String = line.chars().take(300).collect();
    crate::diag!("[diag][ui] {line}");
}

/// The cached snapshot, so the window paints real content at launch rather
/// than a spinner. Never talks to GitHub.
#[tauri::command]
pub fn get_cached(app: AppHandle) -> Result<Vec<PullRequest>, String> {
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). Distinguishes a cold
    // cache (n=0, so the UI must wait on a live fetch) from a warm one,
    // which is the difference between "slow query" and "slow paint".
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    let out = load_snapshot(&conn, CachedList::Authored).map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd get_cached {}",
        match &out {
            Ok(v) => format!("ok n={}", v.len()),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

/// A user-initiated, out-of-band fetch (e.g. a manual refresh button).
/// Does not touch the poll loop's cadence or its cached snapshot on disk.
#[tauri::command]
pub async fn refresh_now(client: State<'_, GhClient>) -> Result<Vec<PullRequest>, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    // Bounded like the poll loop's fetch. This is the COLD-START path --
    // `usePullRequests` calls it whenever the cache is empty, which is
    // exactly a fresh install -- and it had no overall timeout at all.
    //
    // The transport timeouts on the client are not enough on their own,
    // for the reason its own comment gives: a server that trickles bytes
    // keeps a read alive indefinitely without ever tripping one. With
    // `retry` enabled each attempt restarts them, so a machine that
    // cannot complete a handshake sat on "Loading pull requests" for
    // minutes rather than failing with something to act on.
    // DIAGNOSTIC LOGGING (Settings > diagnostic log).
    crate::diag!("[diag] cmd refresh_now start");
    let started = std::time::Instant::now();
    let out = match tokio::time::timeout(crate::poll::FETCH_TIMEOUT, client.fetch_prs()).await {
        Ok(res) => res.map_err(|e| e.to_string()),
        Err(_) => Err(ClientError::Timeout(crate::poll::FETCH_TIMEOUT.as_secs()).to_string()),
    };
    crate::diag!(
        "[diag] cmd refresh_now end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            Ok(v) => format!("ok n={}", v.len()),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

#[tauri::command]
pub async fn get_stats(client: State<'_, GhClient>) -> Result<Stats, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    client
        .fetch_stats(chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}

/// PRs awaiting the user's review.
///
/// A separate command from `get_cached`/`refresh_now` so the snapshot
/// cache keeps its shape; the underlying query returns both lists in one
/// request, so this costs no extra rate limit.
#[tauri::command]
pub async fn get_cycle_trend(client: State<'_, GhClient>) -> Result<CycleTrend, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    client
        .fetch_cycle_trend(chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}

/// Apply an action to a pull request.
///
/// **The only command that writes to GitHub.** The read-only invariant
/// asserted elsewhere in this codebase is now "reads by default, writes
/// only on explicit user action" -- see `github::mutate`.
///
/// Confirmation is the UI's job, not this layer's: a command cannot show
/// a dialog, and putting the policy here would mean a caller that forgot
/// to confirm silently gets the destructive path anyway. What this DOES
/// guarantee is that every write is logged with repo, number and action,
/// so "did I merge that?" has an answer.
#[tauri::command]
pub async fn act_on_pr(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    id: String,
    repo: String,
    number: u64,
    action: String,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let act = parse_action(&action)?;

    match client.mutate_pr(&id, act).await {
        Ok(()) => {
            log::info!("{repo}#{number} {}", act.describe());
            // Refresh promptly rather than waiting out the poll interval:
            // the list would otherwise keep showing a PR as open for up
            // to two minutes after merging it.
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not be {}: {e}", act.describe());
            Err(e.to_string())
        }
    }
}

/// Re-run the failed jobs of a pull request's CI.
///
/// Takes the workflow RUN id, which the detail query now fetches per
/// check. One call re-runs every failed job in that run.
#[tauri::command]
pub async fn rerun_checks(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    repo: String,
    number: u64,
    run_id: u64,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    match client.rerun_failed_jobs(&repo, run_id).await {
        Ok(()) => {
            log::info!("{repo}#{number} failed checks re-run requested");
            // CI state changes as a result, so the list should catch up
            // rather than keep showing the old red until the next tick.
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not re-run checks: {e}");
            Err(e.to_string())
        }
    }
}

/// The platform this build is running on.
///
/// Compile-time constants, so this cannot disagree with the binary. Used
/// by the error report, where both diagnoses so far needed to know the
/// platform and neither could get it from the error text.
#[tauri::command]
pub fn build_target() -> (String, String) {
    (
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
    )
}

/// Who the token belongs to.
///
/// Cached forever by the caller: a login does not change during a
/// session. Used to tell the user's own pull requests from everyone
/// else's, which decides whether approving is even offered -- GitHub
/// refuses self-approval.
#[tauri::command]
pub async fn get_viewer(client: State<'_, GhClient>) -> Result<String, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    client.fetch_viewer().await.map_err(|e| e.to_string())
}

/// Submit a review on a pull request.
///
/// The first write path for a PR the user does not own. Body text is
/// validated HERE as well as in the UI: a command is a public surface,
/// and GitHub refusing an empty REQUEST_CHANGES after a round-trip is a
/// worse error than refusing it before one.
#[tauri::command]
pub async fn review_pr(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    id: String,
    repo: String,
    number: u64,
    verdict: String,
    body: String,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let v = parse_verdict(&verdict)?;
    if v.requires_body() && body.trim().is_empty() {
        return Err(format!(
            "GitHub requires a comment to {}.",
            match v {
                ReviewVerdict::RequestChanges => "request changes",
                _ => "leave a review comment",
            }
        ));
    }

    match client.add_review(&id, v, &body).await {
        Ok(()) => {
            // Never log the body: review text is the user's words about
            // someone else's work, and logs are not the place for it.
            log::info!("{repo}#{number} {}", v.describe());
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not be reviewed: {e}");
            Err(e.to_string())
        }
    }
}

/// Comment on a pull request.
#[tauri::command]
pub async fn comment_on_pr(
    client: State<'_, GhClient>,
    id: String,
    repo: String,
    number: u64,
    body: String,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    if body.trim().is_empty() {
        return Err("A comment cannot be empty.".to_string());
    }
    match client.add_comment(&id, &body).await {
        Ok(()) => {
            log::info!("{repo}#{number} commented");
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not be commented on: {e}");
            Err(e.to_string())
        }
    }
}

/// Resolve a review conversation.
///
/// `thread_id` is the THREAD's node id, not the pull request's -- a
/// different node from every other mutation command here.
#[tauri::command]
pub async fn resolve_thread(
    client: State<'_, GhClient>,
    thread_id: String,
    repo: String,
    number: u64,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    match client.resolve_thread(&thread_id).await {
        Ok(()) => {
            log::info!("{repo}#{number} resolved a conversation");
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not resolve a conversation: {e}");
            Err(e.to_string())
        }
    }
}

/// Reopen a resolved review conversation.
///
/// The undo for `resolve_thread`. Resolving is a single click and GitHub
/// offers no confirmation, so without this a mis-click could only be
/// corrected by leaving the app.
#[tauri::command]
pub async fn unresolve_thread(
    client: State<'_, GhClient>,
    thread_id: String,
    repo: String,
    number: u64,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    match client.unresolve_thread(&thread_id).await {
        Ok(()) => {
            log::info!("{repo}#{number} reopened a conversation");
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not reopen a conversation: {e}");
            Err(e.to_string())
        }
    }
}

/// Reply inside a review conversation.
///
/// Not `comment_on_pr`: that starts a new top-level comment, which would
/// strand the answer away from the code it is about.
#[tauri::command]
pub async fn reply_to_thread(
    client: State<'_, GhClient>,
    thread_id: String,
    repo: String,
    number: u64,
    body: String,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    // Matches `comment_on_pr`: GitHub accepts an empty reply and posts a
    // blank comment, which is never what the click meant.
    if body.trim().is_empty() {
        return Err("A reply cannot be empty.".to_string());
    }
    match client.reply_to_thread(&thread_id, &body).await {
        Ok(()) => {
            log::info!("{repo}#{number} replied to a conversation");
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} could not reply to a conversation: {e}");
            Err(e.to_string())
        }
    }
}

/// Map the frontend's verdict name onto the typed verdict.
fn parse_verdict(v: &str) -> Result<ReviewVerdict, String> {
    match v {
        "approve" => Ok(ReviewVerdict::Approve),
        "request_changes" => Ok(ReviewVerdict::RequestChanges),
        "comment" => Ok(ReviewVerdict::Comment),
        other => Err(format!("unknown review verdict: {other}")),
    }
}

/// Everything the detail view shows for one pull request.
///
/// Fetched on open rather than in the poll loop: it is per-PR and only
/// needed while the view is on screen.
/// Map the frontend's action name onto the typed action.
///
/// Shared by the single and batch commands so the two cannot drift into
/// accepting different sets of names -- the batch would otherwise reject
/// an action the kebab menu happily offers.
fn parse_action(action: &str) -> Result<PrAction, String> {
    match action {
        "merge" => Ok(PrAction::Merge),
        "close" => Ok(PrAction::Close),
        "reopen" => Ok(PrAction::Reopen),
        "draft" => Ok(PrAction::ConvertToDraft),
        "ready" => Ok(PrAction::MarkReady),
        "enqueue" => Ok(PrAction::Enqueue),
        "dequeue" => Ok(PrAction::Dequeue),
        other => Err(format!("unknown action: {other}")),
    }
}

/// One pull request's outcome in a batch.
///
/// `error` is `None` on success. A batch reports every outcome rather
/// than a single verdict: partial failure is the normal case here, not
/// the exception -- some mutations are rejected while others apply, and
/// a lone "done" would hide the rejections.
#[derive(Debug, serde::Serialize)]
pub struct BatchOutcome {
    pub repo: String,
    pub number: u64,
    pub error: Option<String>,
}

/// How many mutations may be in flight at once.
///
/// GitHub applies secondary rate limits to concurrent mutations, and a
/// batch is exactly the shape that trips them -- the premise of this
/// feature is that AI-assisted work produces *many* pull requests, so
/// forty at once is a realistic batch, not a pathological one. Four is
/// well inside the limit while still finishing a large batch promptly.
const BATCH_CONCURRENCY: usize = 4;

#[tauri::command]
/// Apply one action to several pull requests.
///
/// Deliberately not a loop over `act_on_pr` from the frontend: that
/// would fire every mutation at once and wake the poll loop once per
/// success. This bounds concurrency and wakes once at the end.
pub async fn act_on_prs(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    prs: Vec<(String, String, u64)>,
    action: String,
) -> Result<Vec<BatchOutcome>, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let act = parse_action(&action)?;

    let mut outcomes = Vec::with_capacity(prs.len());
    for chunk in prs.chunks(BATCH_CONCURRENCY) {
        let mut set = tokio::task::JoinSet::new();
        for (id, repo, number) in chunk {
            let (client, id, repo, number) = (client.clone(), id.clone(), repo.clone(), *number);
            set.spawn(async move {
                let error = match client.mutate_pr(&id, act).await {
                    Ok(()) => {
                        log::info!("{repo}#{number} {}", act.describe());
                        None
                    }
                    Err(e) => {
                        log::warn!("{repo}#{number} could not be {}: {e}", act.describe());
                        Some(e.to_string())
                    }
                };
                BatchOutcome {
                    repo,
                    number,
                    error,
                }
            });
        }
        while let Some(res) = set.join_next().await {
            match res {
                Ok(o) => outcomes.push(o),
                // A panicked task must not vanish silently, or the batch
                // would report fewer outcomes than it was given and the
                // UI would show a PR as neither succeeded nor failed.
                Err(e) => return Err(format!("a batch task failed: {e}")),
            }
        }
    }

    // Once, at the end -- not per success, which would wake the poll loop
    // forty times for a forty-PR batch.
    waker.0.notify_one();
    Ok(outcomes)
}

#[tauri::command]
/// Merge the base branch into a pull request's head.
///
/// Separate from `act_on_pr` because it needs the head OID: GitHub
/// refuses if the branch moved since the caller last saw it, which turns
/// a stale click into a clear error instead of an update to a commit the
/// user never looked at.
pub async fn update_pr_branch(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    id: String,
    repo: String,
    number: u64,
    expected_head: String,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    match client.update_pr_branch(&id, &expected_head).await {
        Ok(()) => {
            log::info!("{repo}#{number} branch updated from base");
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} branch could not be updated: {e}");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
/// Merge this pull request when its checks pass.
///
/// Takes the head OID the row was rendered from. Auto-merge is a
/// DEFERRED write -- it fires unattended, later -- so without the guard
/// a push after enabling would merge a commit the user never saw.
/// Verified live: a stale OID is refused with "expected head oid does
/// not match the current head oid".
pub async fn set_auto_merge(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    id: String,
    repo: String,
    number: u64,
    expected_head: String,
    enable: bool,
) -> Result<(), String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let result = if enable {
        client.enable_auto_merge(&id, &expected_head).await
    } else {
        client.disable_auto_merge(&id).await
    };
    match result {
        Ok(()) => {
            log::info!(
                "{repo}#{number} auto-merge {}",
                if enable { "enabled" } else { "disabled" }
            );
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} auto-merge change refused: {e}");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
/// Delete a merged pull request's head branch.
///
/// The `merged` flag is checked HERE, not trusted from the caller:
/// deleting the head ref of an OPEN pull request closes it off, and this
/// is the last place that can refuse. Measured demand: 31 of the last 60
/// merged PRs on a real account still held a live remote branch.
pub async fn delete_head_branch(
    client: State<'_, GhClient>,
    waker: State<'_, crate::poll::Waker>,
    ref_id: String,
    repo: String,
    number: u64,
    branch: String,
    merged: bool,
) -> Result<(), String> {
    if !merged {
        return Err("refusing to delete the branch of a pull request that has not merged".into());
    }
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    match client.delete_ref(&ref_id).await {
        Ok(()) => {
            log::info!("{repo}#{number} head branch {branch} deleted");
            waker.0.notify_one();
            Ok(())
        }
        Err(e) => {
            log::warn!("{repo}#{number} branch {branch} could not be deleted: {e}");
            Err(e.to_string())
        }
    }
}

/// One pull request's detail, for the view opened by clicking a row.
///
/// Bounded by `poll::FETCH_TIMEOUT`, the same ceiling the poll loop and
/// `refresh_now` use. Added for #790, where clicking a PR showed
/// "Loading pull request…" for over 30 seconds with nothing to act on.
///
/// This is the path that most needed a ceiling and was the only fetch
/// without one. Every other GitHub fetch is either bounded here or runs
/// in the background where a long one costs nobody's attention; this one
/// is a user gesture with a blocked view behind it, and its worst case
/// was the product of four uncapped multipliers -- up to 4 serial check
/// pages, times octocrab's `max_retries: 3` at a 60-second minimum wait
/// on a 429 (`auth.rs`), times TanStack's retries on top. Minutes,
/// legitimately, with no error and no end.
///
/// The reasoning at `refresh_now` applies unchanged and is the reason a
/// transport timeout is not enough on its own: read and write timeouts
/// bound one socket operation, restart on every retry, and never fire at
/// all against a server that trickles bytes. Only a wall-clock ceiling
/// around the whole command bounds what the user is actually waiting on.
///
/// 30s is generous for this fetch and deliberately so: the budget exists
/// to convert an unbounded hang into an actionable error, not to tighten
/// a latency target. A real fetch that needs 25 seconds should still
/// succeed.
///
/// NOT also applied on the mobile companion's forwarding path
/// (`src-mobile`): this bound is inside the command, so a phone's
/// `remote_call` inherits it for the GitHub work itself. The hop from
/// phone to desktop has no timeout of its own and an unreachable desktop
/// is a separate failure with a separate fix -- see the PR for #790.
#[tauri::command]
pub async fn get_pr_detail(
    client: State<'_, GhClient>,
    repo: String,
    number: u64,
) -> Result<PrDetail, String> {
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). Brackets the whole
    // command for the reason `get_reviewing` gives: without it the log
    // holds the individual POSTs and no total, so a 30-second click
    // could not be attributed to the command at all -- and the gap
    // between the summed POSTs and this elapsed time is exactly where
    // octocrab's rate-limit wait hides, which nothing else records.
    // The repository and number are NOT logged: the diagnostic log is
    // something a user pastes into an issue, and a private repository's
    // name is not ours to put in it.
    crate::diag!("[diag] cmd get_pr_detail start");
    let started = std::time::Instant::now();
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let out = match tokio::time::timeout(
        crate::poll::FETCH_TIMEOUT,
        client.fetch_pr_detail(&repo, number),
    )
    .await
    {
        Ok(res) => res.map_err(|e| e.to_string()),
        Err(_) => Err(ClientError::Timeout(crate::poll::FETCH_TIMEOUT.as_secs()).to_string()),
    };
    crate::diag!(
        "[diag] cmd get_pr_detail end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            Ok(d) => format!("ok checks={}/{}", d.checks.len(), d.checks_total),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

/// Repos and their worktrees, unclassified.
///
/// Fast enough to block a view on: ~800ms for 37 repos and 295 worktrees
/// on this machine. Safety classification is four git calls per worktree
/// and takes ~16s across that set, so it is a separate command the UI
/// calls per repo as results arrive.
#[tauri::command]
pub async fn list_worktrees(app: AppHandle) -> Result<Vec<crate::worktrees::Repo>, String> {
    let dirs = get_worktree_dirs(app);
    // Blocking filesystem and subprocess work: keep it off the async
    // runtime's worker threads.
    tauri::async_runtime::spawn_blocking(move || crate::worktrees::scan_dirs_fast(&dirs))
        .await
        .map_err(|e| e.to_string())
}

/// Classify one repo's worktrees. See `list_worktrees`.
///
/// Each worktree is ALSO emitted on `worktree-safety` as its verdict is
/// reached, so a row can fill the moment its own answer exists rather
/// than holding a skeleton until the slowest branch in the repository
/// finishes. That is #830: a 111-worktree repository showed sizes and
/// counted to 111, and the safety column -- the reason the page exists --
/// stayed skeletal indefinitely, because this command returned only when
/// every worktree was done. The return value is kept so a caller that
/// only wants the final set can ignore the events entirely.
///
/// This is the `size_worktrees` treatment arriving at the pass that
/// needed it more. The size pass was split out first because it was
/// assumed to be the only slow one -- `hooks.ts` records the "three
/// orders of magnitude" reasoning -- and classification was left whole
/// on the strength of a ~16s figure. A bound per git call was mistaken
/// for a bound per worktree; `CLASSIFY_TIMEOUT` explains why it is not.
///
/// A `Safety::Unknown` verdict carrying "classification did not finish"
/// is emitted like any other answer, and DELIBERATELY rather than
/// omitted: a skeleton is a promise that a value is coming, and #830 is
/// what that promise looks like when it is never kept. "Could not
/// classify" is an answer. It must never be flattened toward `Safe` --
/// `is_safe()` is a two-variant allowlist precisely so that a verdict we
/// could not reach can never authorise a deletion.
#[tauri::command]
pub async fn classify_worktrees(
    app: AppHandle,
    repo_path: String,
) -> Result<Vec<crate::worktrees::Worktree>, String> {
    // Two failure modes, both real: the join can fail if the blocking
    // task panicked, and classification itself can fail if git refuses.
    // Flattened rather than swallowed, so an unreadable repo surfaces as
    // an error instead of as zero worktrees.
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        crate::worktrees::classify_repo_streaming(&repo_path, &mut |w| {
            // Emitted per worktree rather than batched, for the reason
            // `size_worktrees` gives: batching would reintroduce exactly
            // the wait this exists to remove.
            let _ = app.emit("worktree-safety", w);
            out.push(w.clone());
        })?;
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Disk sizes for one repo's worktrees, as `(path, bytes)` pairs.
///
/// Separate from classification because it is a full tree walk, and the
/// walk is the expensive part of this view by a wide margin: MEASURED,
/// 21.40s for a single 200 GB checkout against 0.78s for a 0.33 GB one.
/// The cost tracks bytes and file count, not worktree count.
///
/// Each pair is ALSO emitted on `worktree-size` as it is measured, so a
/// view can fill a row in the moment its answer exists rather than
/// holding every row on a skeleton until the slowest tree finishes --
/// which is what #754 reported as an indefinite load. The return value
/// is kept so a caller that only wants the final set can ignore the
/// events entirely.
///
/// A `None` size is a worktree whose walk exceeded `SIZE_TIMEOUT`. It is
/// emitted like any other answer, and it is emitted DELIBERATELY rather
/// than omitted: #769 was a repository where a row simply never heard
/// back, and a skeleton with nothing behind it is the failure #754 set
/// out to remove. `None` means "could not measure" and must never be
/// flattened to 0 on the way out -- zero bytes reads as "this tree is
/// empty, delete it", which for an unmeasurable checkout is the most
/// damaging thing this column could say.
#[tauri::command]
pub async fn size_worktrees(
    app: AppHandle,
    repo_path: String,
) -> Result<Vec<(String, Option<u64>)>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        crate::worktrees::size_repo_streaming(&repo_path, &mut |path, bytes| {
            // Emitted per worktree rather than batched: batching would
            // reintroduce exactly the wait this exists to remove.
            let _ = app.emit("worktree-size", (path, bytes));
            out.push((path.to_string(), bytes));
        })?;
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Regenerable build output under the configured scan roots.
///
/// Discovery only -- every `size_bytes` comes back None. The two passes
/// are separate because they differ by three orders of magnitude:
/// measured on a real 221 GB code tree, finding 178 directories takes
/// ~1.5s where sizing them takes ~56s. Blocking the view on the second
/// would repeat the "All repositories never populates" complaint that
/// shaped the worktree view.
#[tauri::command]
pub async fn scan_artifacts(app: AppHandle) -> Result<Vec<crate::artifacts::Artifact>, String> {
    // The SAME roots the worktree view scans. A second directory setting
    // would be one more thing to keep in sync, and a user who has told
    // the app where their code lives has already answered this question.
    let dirs = get_worktree_dirs(app);
    tauri::async_runtime::spawn_blocking(move || crate::artifacts::scan(&dirs))
        .await
        .map_err(|e| e.to_string())
}

/// Sizes for artifact directories, as `(path, bytes, secs_since_write)`.
///
/// Takes explicit paths rather than rescanning, so the caller measures
/// exactly what it is showing -- a rescan here could return a directory
/// the list does not have a row for.
///
/// `secs_since_write` rides along because the walk already stats every
/// entry: asking a second question of the same `metadata()` call is
/// free, and it is the ONLY signal that a build is currently writing
/// there. Build output is gitignored, so no git check can see it.
/// How many sizing walks may run at once.
///
/// These are disk-bound, and the frontend fires one per repository
/// group -- 54 concurrently on a real machine. Measured there: groups of
/// TWO directories took 17.6 seconds, which is contention rather than
/// work, and it blocked an artifact removal behind it for 20 seconds.
///
/// A cap makes the total no slower (the disk is the bottleneck either
/// way) while leaving the blocking pool free for everything else --
/// which is what actually made the UI look frozen.
static SIZE_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[tauri::command]
pub async fn size_artifacts(paths: Vec<String>) -> Result<Vec<(String, u64, Option<u64>)>, String> {
    // Held for the whole walk. `acquire` only fails if the semaphore is
    // closed, which never happens for a static.
    let _permit = SIZE_LIMIT
        .acquire()
        .await
        .map_err(|e| format!("could not schedule the measurement: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        // DIAGNOSTIC LOGGING (Settings > diagnostic log). Per-directory,
        // for the same reason as `size_venvs`: the total says the batch
        // was slow, this says which entry made it slow.
        let started = std::time::Instant::now();
        let total = paths.len();
        let out: Vec<(String, u64, Option<u64>)> = paths
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let each = std::time::Instant::now();
                let (bytes, age) = crate::artifacts::measure(std::path::Path::new(&p));
                crate::diag!(
                    "[diag] size_artifacts {}/{} {}ms {}b",
                    i + 1,
                    total,
                    each.elapsed().as_millis(),
                    bytes
                );
                (p, bytes, age)
            })
            .collect();
        crate::diag!(
            "[diag] size_artifacts total {}ms n={total}",
            started.elapsed().as_millis()
        );
        out
    })
    .await
    .map_err(|e| e.to_string())
}

/// Remove artifact directories, re-verifying each at delete time.
///
/// The scan roots are passed to the backend rather than trusted from the
/// caller: containment is the only thing between a bad path and
/// `remove_dir_all` on an arbitrary directory, so the boundary it checks
/// against must come from settings, not from the request.
#[tauri::command]
pub async fn remove_artifacts(
    app: AppHandle,
    paths: Vec<String>,
) -> Result<Vec<crate::artifacts::ArtifactRemoval>, String> {
    let roots = get_worktree_dirs(app);
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). This is the
    // BACKEND half of the freeze report: paired with the frontend's
    // `ui remove_artifacts` marks, it separates a slow `remove_dir_all`
    // from a slow render. The work is already off the event loop, so if
    // this number is small and the UI one is large, the cost is in the
    // frontend.
    let started = std::time::Instant::now();
    let count = paths.len();
    crate::diag!("[diag] remove_artifacts start n={count}");
    let out = tauri::async_runtime::spawn_blocking(move || {
        crate::artifacts::remove_artifacts(&paths, &roots)
    })
    .await
    .map_err(|e| e.to_string())?;
    crate::diag!(
        "[diag] remove_artifacts done {}ms n={count}",
        started.elapsed().as_millis()
    );
    let failed = out.iter().filter(|o| o.error.is_some()).count();
    log::info!(
        "artifact removal: {} of {} removed",
        out.len() - failed,
        out.len()
    );
    Ok(out)
}

/// Poetry virtualenvs, classified against every directory we can see.
///
/// Discovery only: sizes and idle times come from `size_venvs`, because
/// deciding staleness needs a full walk of each venv and the list should
/// paint before that finishes.
#[tauri::command]
pub async fn scan_venvs(app: AppHandle) -> Result<Vec<crate::caches::Venv>, String> {
    let roots = get_worktree_dirs(app);
    tauri::async_runtime::spawn_blocking(move || {
        let dirs = crate::caches::project_dirs(&roots);
        log::info!(
            "venv scan: {} candidate project directories{}",
            dirs.dirs.len(),
            if dirs.truncated {
                " (TRUNCATED -- orphan verdicts withheld)"
            } else {
                ""
            }
        );
        crate::caches::scan_poetry(&dirs)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Sizes and idle times, as `(path, bytes, idle_secs)`.
///
/// The idle time is the whole reason this is a second pass: it comes
/// from the DEEPEST file mtime, which needs the same walk as the size.
/// Poetry touches a venv's root without writing inside, so the
/// directory's own mtime reports a year-old venv as days old.
#[tauri::command]
pub async fn size_venvs(paths: Vec<String>) -> Result<Vec<(String, u64, Option<u64>)>, String> {
    // Shares the artifact cap: both walk the same disk, and a venv batch
    // competing with a 54-way artifact fan-out is the same contention.
    let _permit = SIZE_LIMIT
        .acquire()
        .await
        .map_err(|e| format!("could not schedule the measurement: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        // DIAGNOSTIC LOGGING (Settings > diagnostic log).
        //
        // PER-VENV, not just a total: these are walked serially in one
        // call, so a single pathological path -- a network mount, a
        // permission wall -- stalls every other row with nothing on
        // screen changing. A total says "slow"; this says WHICH.
        let started = std::time::Instant::now();
        let total = paths.len();
        let out: Vec<(String, u64, Option<u64>)> = paths
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let each = std::time::Instant::now();
                let (bytes, idle) = crate::caches::measure(std::path::Path::new(&p));
                crate::diag!(
                    "[diag] size_venvs {}/{} {}ms",
                    i + 1,
                    total,
                    each.elapsed().as_millis() // Deliberately NO name, not even a basename: a venv
                                               // directory is `<project>-<hash>-py3.13`, so the
                                               // basename IS the project name. The index answers
                                               // "which one was slow" without naming it, and
                                               // Settings promises this log carries no such names.
                );
                (p, bytes, idle)
            })
            .collect();
        crate::diag!(
            "[diag] size_venvs total {}ms n={}",
            started.elapsed().as_millis(),
            total
        );
        out
    })
    .await
    .map_err(|e| e.to_string())
}

/// Remove Poetry virtualenvs, re-verifying each at delete time.
///
/// The project directories are re-walked HERE rather than taken from the
/// request: whether a venv is orphaned depends entirely on that set, and
/// a caller supplying a short one could turn any live venv into a
/// deletion candidate.
#[tauri::command]
pub async fn remove_venvs(
    app: AppHandle,
    paths: Vec<String>,
) -> Result<Vec<crate::caches::VenvRemoval>, String> {
    // MANUAL removal is not gated by a setting.
    //
    // This used to read `remove_stale_venvs`, on the reasoning that a
    // staleness threshold is a guess about intent. That argument holds
    // for AUTOMATIC cleanup, where the app acts on its own -- and it was
    // wrong here. The user is looking at a list, ticking a specific row,
    // and confirming in a dialog: the tick IS the intent, and no other
    // artifact asks permission twice. A Rust `target` costs minutes to
    // rebuild and has no such gate; a virtualenv is `poetry install`.
    //
    // `RemovalPolicy` remains for automatic cleanup, which still needs a
    // threshold it can be conservative about.
    //
    // The safety that matters is untouched and lives in `remove_venv`:
    // re-verified at delete time, symlinks refused, containment inside
    // Poetry's cache enforced. Those are facts about the path rather
    // than guesses about intent.
    let prefs = get_ui_prefs(app.clone());
    let policy = crate::caches::RemovalPolicy {
        allow_stale: true,
        stale_days: crate::poll::stale_venv_days(&prefs),
    };
    let roots = get_worktree_dirs(app);
    let out = tauri::async_runtime::spawn_blocking(move || {
        let dirs = crate::caches::project_dirs(&roots);
        crate::caches::remove_venvs(&paths, &dirs, policy)
    })
    .await
    .map_err(|e| e.to_string())?;
    let failed = out.iter().filter(|o| o.error.is_some()).count();
    log::info!(
        "venv removal: {} of {} removed",
        out.len() - failed,
        out.len()
    );
    Ok(out)
}

/// Record that a human read an assessment for this worktree.
///
/// Split out of `claudify_command`, which used to do it as a side effect
/// of copying the prompt. That conflated "I asked for an assessment"
/// with "I read one" -- and the flag it sets is what unlocks removing a
/// worktree past the safety gate, which `remove_worktree_forced`
/// describes as needing "the record that a human looked at what would be
/// lost".
///
/// Keyed by the head OID it was assessed AT, so the mark expires the
/// moment the branch moves: a verdict about different commits is not a
/// verdict about these ones.
#[tauri::command]
pub fn mark_assessed(app: AppHandle, worktree_path: String) -> Result<(), String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    let mut seen: std::collections::BTreeMap<String, String> =
        settings::get(&conn, settings::keys::ASSESSED_WORKTREES)
            .ok()
            .flatten()
            .unwrap_or_default();
    let head = crate::worktrees::head_oid(&worktree_path)
        .map_err(|e| format!("could not read the worktree's head: {e}"))?;
    seen.insert(worktree_path, head);
    settings::set(&conn, settings::keys::ASSESSED_WORKTREES, &seen).map_err(|e| e.to_string())
}

/// Forget that a worktree was assessed.
///
/// The mark is what turns Claudify into "Remove anyway…", and it
/// persists across restarts -- so a single exploratory click removed the
/// only way to copy that worktree's prompt, permanently, until the
/// branch happened to move. This is the way back.
///
/// Removing a mark is the SAFE direction: it re-locks the force-removal
/// path rather than unlocking it, so it needs no confirmation of its
/// own.
#[tauri::command]
pub fn clear_assessed(app: AppHandle, worktree_path: String) -> Result<(), String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    let mut seen: std::collections::BTreeMap<String, String> =
        settings::get(&conn, settings::keys::ASSESSED_WORKTREES)
            .ok()
            .flatten()
            .unwrap_or_default();
    seen.remove(&worktree_path);
    settings::set(&conn, settings::keys::ASSESSED_WORKTREES, &seen).map_err(|e| e.to_string())
}

/// What automatic cleanup would remove, run now.
///
/// PREVIEW ONLY: `cleanup::propose` has no removal path, so this command
/// cannot delete regardless of what it is passed. That is the property
/// making Phase 1 reviewable on the predicate's merits alone.
///
/// Writes the result to the ledger before returning it, so the record
/// exists whether or not anyone is looking at the window when the pass
/// runs.
#[tauri::command]
pub async fn preview_cleanup(app: AppHandle) -> Result<Vec<crate::cleanup::LedgerEntry>, String> {
    let roots = get_worktree_dirs(app.clone());
    let db = db_path(&app);
    let now = chrono::Utc::now().to_rfc3339();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db).map_err(|e| e.to_string())?;
        let prefs = crate::cleanup::prefs(&conn);
        let entries = crate::cleanup::propose(&prefs, &roots, &now);
        crate::cleanup::record(&conn, &entries);
        log::info!("cleanup preview: {} entries", entries.len());
        Ok(entries)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The cleanup ledger, newest first.
#[tauri::command]
pub fn cleanup_log(app: AppHandle) -> Result<Vec<crate::cleanup::LedgerEntry>, String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    crate::cleanup::recent(&conn, 200).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_cleanup_prefs(app: AppHandle) -> crate::cleanup::CleanupPrefs {
    open_db(&db_path(&app))
        .ok()
        .map(|c| crate::cleanup::prefs(&c))
        .unwrap_or_default()
}

#[tauri::command]
pub fn set_cleanup_prefs(
    app: AppHandle,
    prefs: crate::cleanup::CleanupPrefs,
) -> Result<(), String> {
    // Remove mode is NOT accepted. The type carries the variant so the
    // ledger and settings shapes do not change in Phase 2, but nothing
    // in Phase 1 may store it -- a setting that does nothing is worse
    // than one that does not exist, because the user believes it.
    if prefs.mode == crate::cleanup::CleanupMode::Remove {
        return Err("automatic removal is not available yet; this build previews only".into());
    }
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    settings::set(&conn, settings::keys::CLEANUP_PREFS, &prefs).map_err(|e| e.to_string())
}

/// Which dependencies are out of date in one repository.
///
/// On demand, never on a timer: these commands hit package registries and
/// take seconds on a large tree. That is a per-repo click, not something
/// to do in the background across every repository.
#[tauri::command]
pub async fn check_packages(
    repo_path: String,
) -> Result<Vec<crate::packages::ProjectReport>, String> {
    // TWO PHASES.
    //
    // The blocking pass spawns each ecosystem's tool and parses its
    // output. Terraform and Swift have no such tool, so they come back
    // with `latest == current` and `Bump::Unknown`, and `registry::
    // enrich` fills those in over HTTP afterwards.
    //
    // Split this way rather than made async throughout because the
    // subprocess work must stay off the event loop, and the network work
    // must not sit inside a blocking task.
    let mut reports = tauri::async_runtime::spawn_blocking(move || {
        let reports = crate::packages::run::check_repo(std::path::Path::new(&repo_path));
        // Counts only -- never package names, which would put a private
        // dependency list in a log meant to be shared.
        log::info!(
            "package check: {} projects, {} outdated",
            reports.len(),
            reports
                .iter()
                .flat_map(|p| &p.reports)
                .map(|r| r.outdated.len())
                .sum::<usize>()
        );
        reports
    })
    .await
    .map_err(|e| e.to_string())?;

    // Phase two. Only touches rows whose ecosystem needs a registry, and
    // a failed lookup leaves the row at `Bump::Unknown` rather than
    // claiming it is current.
    crate::packages::registry::enrich(&mut reports).await;

    Ok(reports)
}

/// Push an update run's branch and open a pull request.
///
/// PHASE 2, and the first command in this app that writes to a shared
/// remote. Everything before it was local: worktrees, removals and
/// applies are all undoable by the user alone, and this is not.
///
/// Takes a report from `apply_package_updates` rather than doing the
/// work itself, so the user has seen what landed before anything is
/// pushed. That separation is the point of the phasing.
#[tauri::command]
pub async fn open_update_pr(
    client: State<'_, GhClient>,
    repo_path: String,
    report: crate::packages::apply::RunReport,
) -> Result<String, String> {
    open_update_pr_inner(client.inner(), &repo_path, report).await
}

/// The body of `open_update_pr`, callable without a `State`.
///
/// Split out so the background run (`apply_updates_in_background`) can
/// open the pull request with the SAME refusals -- nothing applied, and
/// an ecosystem whose resolved constraint cannot be read back. A second
/// implementation would be a second set of rules to keep in step.
pub(crate) async fn open_update_pr_inner(
    client: &GhClient,
    repo_path: &str,
    report: crate::packages::apply::RunReport,
) -> Result<String, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let repo_path = repo_path.to_string();

    // Nothing applied, nothing to open. A pull request with an empty
    // diff is noise, and GitHub refuses it anyway ("No commits
    // between") -- better to say so before pushing.
    if report.results.iter().all(|r| r.error.is_some()) {
        return Err("no updates were applied, so there is nothing to open".into());
    }

    // Only ecosystems whose resolved constraint can be READ BACK.
    //
    // The body's whole value is stating what actually landed. Poetry,
    // uv, .NET and CocoaPods report `resolved_constraint` as None --
    // reading those manifests safely needs a real TOML/XML parser -- so
    // a description would have to say "not verified" about every row,
    // which is not a description worth opening a pull request with.
    //
    // Refused BEFORE the push, so a run that cannot be described does
    // not leave a branch on the remote.
    if !report
        .ecosystems
        .iter()
        .all(|e| crate::packages::pr::can_describe(*e))
    {
        return Err(
            "a pull request can only be opened for npm and yarn so far: the other \
             ecosystems do not report what version actually landed, and the \
             description would have to guess."
                .into(),
        );
    }

    let worktree = std::path::PathBuf::from(&report.worktree);
    let base = crate::packages::apply::default_branch(&worktree)
        .ok_or("could not determine the default branch from origin/HEAD")?;
    let slug = crate::worktrees::repo_identity(&repo_path)
        .ok_or("could not determine owner/repo from the git remote")?;

    // Committed, pushed, THEN opened. A failure at any step leaves the
    // worktree in place with its changes intact, which is what phase 1
    // already delivered -- so a partial run costs nothing that was not
    // already there.
    crate::packages::apply::commit_all(&worktree, &crate::packages::pr::title(&report.results))?;
    crate::packages::apply::push_branch(&worktree, &report.branch)?;

    let url = client
        .create_pull_request(
            &slug,
            &report.branch,
            &base,
            &crate::packages::pr::title(&report.results),
            &crate::packages::pr::body(&report),
        )
        .await
        .map_err(|e| e.to_string())?;

    log::info!("opened {url} from {}", report.branch);
    Ok(url)
}

/// The updates as markdown, for handing to an agent.
#[tauri::command]
pub fn packages_markdown(
    repo_path: String,
    reports: Vec<crate::packages::ProjectReport>,
    filter: crate::packages::markdown::Filter,
) -> String {
    crate::packages::markdown::render(&repo_path, &reports, filter)
}

/// Create a worktree and apply dependency updates in it.
///
/// Phase 1 of the update wizard: it does NOT push and does NOT open a
/// pull request. The worktree is left in place and its path is returned,
/// because what these package managers actually do to a checkout is the
/// thing being found out.
///
/// The FIRST command in this app that runs a package manager in a mode
/// that writes, which is why it carries the same care the destructive
/// git paths do.
#[tauri::command]
pub async fn apply_package_updates(
    repo_path: String,
    requests: Vec<crate::packages::apply::UpdateRequest>,
) -> Result<crate::packages::apply::RunReport, String> {
    let repo = repo_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::packages::apply::run(std::path::Path::new(&repo), &requests)
    })
    .await
    .map_err(|e| e.to_string())?;

    // Logged with the repository and branch, so "where did that
    // worktree come from?" has an answer -- the same reason
    // `remove_worktree` logs.
    match &result {
        Ok(r) => log::info!(
            "applied {} update(s) in {} on branch {}",
            r.results.len(),
            r.worktree,
            r.branch
        ),
        Err(e) => log::warn!("update run in {repo_path} refused: {e}"),
    }
    result
}

/// Reveal the diagnostic log in the file manager.
///
/// A command rather than the opener plugin's `open-url`: that is
/// ACL-gated to the http/https scope (see `capabilities/default.json`),
/// and revealing a local file would need a new grant. App commands
/// registered through `generate_handler!` are not ACL-gated, so this
/// keeps the capability surface unchanged.
///
/// Returns the PATH on success, so the caller can show it even where
/// revealing is unsupported -- being told where the file is beats a
/// button that silently does nothing.
#[tauri::command]
pub fn reveal_log(app: AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("could not locate the log directory: {e}"))?;
    let file = dir.join("headstate.log");
    let shown = file.to_string_lossy().into_owned();
    // Reveal the FILE, not just the directory, so the user does not have
    // to find it among rotated siblings.
    match tauri_plugin_opener::reveal_item_in_dir(&file) {
        Ok(()) => Ok(shown),
        // The path is still useful when revealing is unsupported, so
        // this reports where to look rather than only that it failed.
        Err(e) => Err(format!("could not open {shown}: {e}")),
    }
}

/// Every CLAUDE.md in a repository, with its import tree resolved.
#[tauri::command]
pub async fn scan_claude_md(repo_path: String) -> Result<Vec<crate::claudemd::ClaudeFile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::claudemd::scan_repo(std::path::Path::new(&repo_path))
    })
    .await
    .map_err(|e| e.to_string())
}

/// The text of one file, for rendering.
///
/// Read fresh rather than carried in the scan: the scan holds every file
/// in a repository, and shipping all of their contents to the frontend
/// to display one is a lot of bytes crossing the bridge for nothing.
#[tauri::command]
pub fn read_claude_md(path: String) -> Result<String, String> {
    // No containment check because there is no write here and no
    // deletion -- this reads a path the user picked from a list the app
    // produced. The risk a containment check guards against elsewhere
    // (`remove_dir_all` on an arbitrary path) does not exist for a read.
    std::fs::read_to_string(&path).map_err(|e| format!("could not read {path}: {e}"))
}

/// Remove a worktree, refusing anything not provably safe.
///
/// The safety gate is re-evaluated inside `remove_worktree` rather than
/// trusted from whatever the UI last saw: a scan is a snapshot, and the
/// user may have started editing since.
///
/// Logged with path and branch, so "where did that go?" has an answer.
#[tauri::command]
pub async fn remove_worktree(repo_path: String, worktree_path: String) -> Result<(), String> {
    let wt = worktree_path.clone();
    let repo = repo_path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::worktrees::remove_worktree(&repo, &wt))
            .await
            .map_err(|e| e.to_string())?;

    match &result {
        Ok(()) => log::info!("removed worktree {worktree_path}"),
        Err(e) => log::warn!("refused to remove worktree {worktree_path}: {e}"),
    }
    result
}

/// Fast-forward a checkout to its upstream.
///
/// The FIRST command that writes to a local checkout, so it carries the
/// same care the destructive ones do: it refuses on a dirty tree, it
/// fast-forwards only, and it returns git's own refusal rather than a
/// generic message. `pull_checkout` re-checks the state itself rather
/// than trusting the scan.
#[tauri::command]
pub async fn pull_checkout(path: String) -> Result<String, String> {
    let p = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::worktrees::pull_checkout(&p))
        .await
        .map_err(|e| e.to_string())?;

    match &result {
        Ok(_) => log::info!("updated checkout {path}"),
        Err(e) => log::warn!("refused to update checkout {path}: {e}"),
    }
    result
}

/// Refresh one repository's remote refs without moving a branch (#788).
///
/// The counterpart to `pull_checkout`, and the reason it is a separate
/// command rather than a flag on that one: the two answer different
/// questions. `pull_checkout` MOVES the checkout, which is what you want
/// once you know you are behind; this only makes the page's comparison
/// true again, which is what you want before you know anything. Until
/// now the only way to refresh `origin/*` from inside the app was to
/// perform the merge, so finding out whether you were behind required
/// ceasing to be behind.
///
/// Does no rescan of its own. The invalidation that repaints the rows
/// belongs to the frontend hook, which already owns the query keys --
/// emitting a scan from here would race the one the hook triggers and
/// double the work for one click.
#[tauri::command]
pub async fn fetch_refs(path: String) -> Result<String, String> {
    let p = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::worktrees::fetch_refs(&p))
        .await
        .map_err(|e| e.to_string())?;

    match &result {
        Ok(_) => log::info!("fetched refs for {path}"),
        Err(e) => log::warn!("could not fetch refs for {path}: {e}"),
    }
    result
}

/// Delete an orphaned worktree directory.
///
/// Separate from `remove_worktree` because git cannot do it: the
/// repository that owned the checkout is gone, so there is nothing to
/// run `git worktree remove` against. That makes it a plain recursive
/// delete, and `remove_orphan` re-derives orphan status itself rather
/// than trusting this call.
///
/// The scan roots are passed to the backend rather than trusted from the
/// caller, the same rule `remove_artifacts` above states: containment is
/// one of the two things between a bad path and `remove_dir_all` on an
/// arbitrary directory, so the boundary it checks against must come from
/// settings, not from the request. This call used to pass the path alone
/// (#854) -- the one recursive delete in the app with neither a symlink
/// check nor a containment check, reachable from a paired peer.
#[tauri::command]
pub async fn remove_orphan(app: AppHandle, path: String) -> Result<(), String> {
    let roots = get_worktree_dirs(app);
    let p = path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::worktrees::remove_orphan(&p, &roots))
            .await
            .map_err(|e| e.to_string())?;

    match &result {
        Ok(()) => log::info!("removed orphaned worktree {path}"),
        Err(e) => log::warn!("refused to remove orphan {path}: {e}"),
    }
    result
}

/// Directories scanned for git checkouts.
///
/// Defaults to `~/code` when unset, so the app works with no
/// configuration on a machine that follows that convention -- and says
/// what it scanned rather than silently finding nothing.
#[tauri::command]
pub fn get_worktree_dirs(app: AppHandle) -> Vec<String> {
    open_db(&db_path(&app))
        .ok()
        .and_then(|c| settings::get::<Vec<String>>(&c, settings::keys::WORKTREE_DIRS).ok())
        .flatten()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(default_worktree_dirs)
}

/// `~/code` if it exists, else nothing.
///
/// Returning a path that does not exist would make the worktrees view
/// report "no repos found" for a directory the user never chose.
pub fn default_worktree_dirs() -> Vec<String> {
    crate::auth::home_dir()
        .map(|h| h.join("code"))
        .filter(|p| p.is_dir())
        .map(|p| vec![p.to_string_lossy().into_owned()])
        .unwrap_or_default()
}

/// Replace the scanned directories.
///
/// Non-existent paths are rejected rather than stored: a typo should fail
/// visibly here, not silently produce an empty worktrees view later.
#[tauri::command]
/// Build history: what was built, how long it took, and how much came
/// from cache.
///
/// The context and revision ARE resolved here, in parallel.
///
/// They were not, and that was a silent bug: `parse_history` hardcodes
/// `context: None, revision: None`, and `enrich` -- the only thing that
/// fills them -- was called from exactly one place, an `#[ignore]`d
/// test. `buildForImage` filters on `b.revision &&`, so with revision
/// always null the build fold in the expanded image row NEVER rendered.
/// The tests passed because the fixture injects a synthetic revision.
///
/// So the "half that mattered" kept from the retired Builds page (#365)
/// was never actually delivered, which is why the Docker surface reads
/// as having little to say.
///
/// Parallel because `inspect` is a subprocess: MEASURED at ~2s per
/// record serially, which blew a two-minute timeout across fifty
/// records. Eight workers mirrors `CLASSIFY_WORKERS` in the worktree
/// scanner, whose author measured 12 and 16 as REGRESSIONS -- the number
/// is empirical, not a core count.
///
/// BLOCKING work, so it goes to a blocking thread. Every command in this
/// module used to be a plain `fn`, which runs on the async runtime's
/// worker and stalls it -- clicking Docker in the menu froze the WHOLE
/// UI for seconds, not just this view (#496). `list_branches` already
/// had the right shape; these did not.
pub async fn docker_builds() -> Result<Vec<crate::docker::Build>, String> {
    tauri::async_runtime::spawn_blocking(docker_builds_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn docker_builds_blocking() -> Result<Vec<crate::docker::Build>, String> {
    const ENRICH_WORKERS: usize = 8;

    let mut builds = crate::docker::docker(&["buildx", "history", "ls", "--format", "{{json .}}"])
        .map(|out| crate::docker::parse_history(&out))?;

    let chunk = builds.len().div_ceil(ENRICH_WORKERS).max(1);
    std::thread::scope(|scope| {
        for part in builds.chunks_mut(chunk) {
            scope.spawn(move || {
                for b in part {
                    crate::docker::enrich(b);
                }
            });
        }
    });

    Ok(builds)
}

#[tauri::command]
/// Whether Docker can be talked to.
///
/// A stopped daemon is a state, not an error: reporting it as a failure
/// -- or as an empty image list -- would say the machine is clean when
/// the truth is that we could not ask.
pub async fn docker_state() -> crate::docker::DockerState {
    // See `docker_builds`: a plain `fn` here blocks the async runtime.
    tauri::async_runtime::spawn_blocking(crate::docker::state)
        .await
        // A join failure means we could not ASK, which is precisely
        // what `Unknown` means -- never `NotRunning`, which would tell
        // the user to start a daemon that may well be running.
        .unwrap_or_else(|e| crate::docker::DockerState::Unknown(e.to_string()))
}

#[tauri::command]
/// Images with provenance and in-use resolved.
///
/// Resolved against the same directories the worktrees view scans, so a
/// machine configured once works for both.
pub async fn docker_images(app: AppHandle) -> Result<Vec<crate::docker::Image>, String> {
    let dirs = get_worktree_dirs(app);
    // The heaviest of these: `scan_dirs_fast` is the same full worktree
    // expansion the Worktrees view pays ~2.6s for, and `classify` then
    // runs git per repository on top of it. Blocking the runtime on
    // that is what froze the app on every switch to this view.
    tauri::async_runtime::spawn_blocking(move || docker_images_blocking(dirs))
        .await
        .map_err(|e| e.to_string())?
}

fn docker_images_blocking(dirs: Vec<String>) -> Result<Vec<crate::docker::Image>, String> {
    // EXPANDED into repositories, not passed as scan roots.
    //
    // `classify` resolves a SHA-shaped image tag by running git in each
    // path it is given, so handing it `~/code` asked git about a
    // directory that is not a repository -- every lookup failed, no
    // image resolved an origin, and the whole Docker page's provenance
    // was silently empty. MEASURED on a real machine: 0 of 24 images
    // resolved an origin with the roots, against 20 of 26 with the
    // repositories.
    //
    // `scan_dirs_fast` is the same expansion the Worktrees view uses,
    // which is why that view worked and this one did not.
    let repos: Vec<std::path::PathBuf> = crate::worktrees::scan_dirs_fast(&dirs)
        .into_iter()
        .map(|r| std::path::PathBuf::from(r.path))
        .collect();
    crate::docker::classify(&repos)
}

#[tauri::command]
/// Where the disk actually went. Images are only part of it.
pub async fn docker_disk_usage() -> Result<crate::docker::DiskUsage, String> {
    // See `docker_builds`.
    tauri::async_runtime::spawn_blocking(|| {
        crate::docker::docker(&["system", "df"]).map(|out| crate::docker::disk_usage(&out))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
/// Remove images by ID, reporting each independently.
///
/// In-use is re-checked per image at removal time, not trusted from the
/// listing: a container may have started since.
pub fn docker_remove_images(ids: Vec<String>) -> Vec<crate::docker::RemovalOutcome> {
    let outcomes = crate::docker::remove_images(&ids);
    let failed = outcomes.iter().filter(|o| o.error.is_some()).count();
    log::info!(
        "docker: removed {} of {} images",
        outcomes.len() - failed,
        outcomes.len()
    );
    outcomes
}

#[tauri::command]
/// Volumes attached to nothing.
pub fn docker_dangling_volumes() -> Result<Vec<crate::docker::DanglingVolume>, String> {
    crate::docker::dangling_volumes()
}

#[tauri::command]
/// Remove one volume. Never bulk: a wrongly deleted volume costs data,
/// where a wrongly deleted image costs a rebuild.
pub fn docker_remove_volume(name: String) -> Result<(), String> {
    log::warn!("docker: removing volume {name}");
    crate::docker::remove_volume(&name)
}

#[tauri::command]
/// Clear build cache, returning what was actually freed.
pub fn docker_prune_cache(until: Option<String>) -> Result<u64, String> {
    let freed = crate::docker::prune_build_cache(until.as_deref())?;
    log::info!("docker: build cache prune freed {freed} bytes");
    Ok(freed)
}

#[tauri::command]
/// Containers a restart would stop, so the confirmation can name them.
pub fn docker_running_containers() -> Result<Vec<String>, String> {
    crate::docker::running_containers()
}

#[tauri::command]
/// Restart the Docker engine.
pub fn docker_restart() -> Result<(), String> {
    log::warn!("docker: restarting the engine");
    crate::docker::restart_engine()
}

#[tauri::command]
/// Start a stopped engine.
pub fn docker_start() -> Result<(), String> {
    crate::docker::start_engine()
}

#[tauri::command]
/// Remove several worktrees, reporting each one's outcome.
///
/// The per-worktree safety gate is unchanged: this is N safe deletions,
/// not one bulk deletion. Each is re-checked at delete time, so a
/// worktree that went dirty since the scan is refused while the rest
/// proceed.
pub async fn remove_worktrees(
    app: AppHandle,
    repo_path: String,
    worktree_paths: Vec<String>,
) -> Result<Vec<crate::worktrees::RemovalOutcome>, String> {
    // `spawn_blocking`, unlike the previous version. Removal is
    // sequential git plumbing at a few hundred milliseconds each, so
    // ~100 worktrees blocked the async runtime for about 30 seconds --
    // which also stalled the poll loop and every other command. The
    // single-worktree command already did this; the bulk one, which
    // blocks far longer, did not.
    tauri::async_runtime::spawn_blocking(move || {
        let outcomes = crate::worktrees::remove_worktrees_with_progress(
            &repo_path,
            &worktree_paths,
            |done, total| {
                // Counts only -- never paths. A progress event is not a
                // place to leak what the user is working on.
                let _ = app.emit("worktree-removal-progress", (done, total));
            },
        );
        let failed = outcomes.iter().filter(|o| o.error.is_some()).count();
        log::info!(
            "bulk removal: {} of {} removed",
            outcomes.len() - failed,
            outcomes.len()
        );
        outcomes
    })
    .await
    .map_err(|e| format!("bulk removal failed to run: {e}"))
}

#[tauri::command]
/// Remove a worktree the safety gate refuses.
///
/// Reached only from a confirmation the user opened after reading an
/// assessment of this specific worktree. The flag is not a convenience:
/// it is the record that a human looked at what would be lost.
pub async fn remove_worktree_forced(
    app: AppHandle,
    repo_path: String,
    worktree_path: String,
) -> Result<(), String> {
    crate::worktrees::remove_worktree_forced(&repo_path, &worktree_path)?;
    // Drop the mark: the worktree is gone, so keeping it would leave a
    // stale entry that outlives the thing it described.
    if let Ok(conn) = open_db(&db_path(&app)) {
        let mut seen: std::collections::BTreeMap<String, String> =
            settings::get(&conn, settings::keys::ASSESSED_WORKTREES)
                .ok()
                .flatten()
                .unwrap_or_default();
        if seen.remove(&worktree_path).is_some() {
            let _ = settings::set(&conn, settings::keys::ASSESSED_WORKTREES, &seen);
        }
    }
    log::warn!("{worktree_path} removed past the safety gate");
    Ok(())
}

#[tauri::command]
/// Clear a worktree's lock (#775).
///
/// Reached only from a confirmation that names the holder, the age, and
/// what the worktree would be underneath -- the reading #753 wanted
/// before anyone clears a claim, which is why it declined a bare
/// button.
///
/// Removes NOTHING. It clears a guard, and the safety gate is untouched
/// by it: the worktree is re-classified afterwards and is removable
/// only if it earns that on its own. Logged at `info` rather than
/// `warn` for the same reason -- this is a reversible operation, and
/// reserving `warn` for the unrecoverable one keeps that signal worth
/// reading.
///
/// `spawn_blocking`: two git calls, one of which lists every worktree
/// in the repository.
pub async fn unlock_worktree(repo_path: String, worktree_path: String) -> Result<(), String> {
    let repo = repo_path.clone();
    let wt = worktree_path.clone();
    tauri::async_runtime::spawn_blocking(move || crate::worktrees::unlock_worktree(&repo, &wt))
        .await
        .map_err(|e| format!("unlock failed to run: {e}"))??;
    log::info!("{worktree_path} unlocked");
    Ok(())
}

#[tauri::command]
/// Clear a repository's stale worktree registrations (#793).
///
/// Takes no worktree path, and that is the whole shape of the thing:
/// `git worktree prune` is repo-wide, so a per-row command would have
/// promised a scope git does not offer. The UI matches it with one
/// header affordance carrying the count.
///
/// No confirmation dialog behind it, unlike every other cleanup command
/// here. There is nothing to confirm: each registration it clears
/// describes a directory git has already reported gone, so there is no
/// tree to lose work from and no branch or commit is touched. A dialog
/// asking "are you sure?" about an operation with no recoverable loss
/// teaches the user to click through the dialogs that do matter.
///
/// Logged at `info` rather than `warn` for the same reason
/// `unlock_worktree` is: `warn` is reserved here for the unrecoverable
/// action, and spending it on bookkeeping makes that signal worth less.
/// The COUNT is logged, because "pruned 0" and "pruned 12" are different
/// events on a support log and a bare "pruned" is neither.
///
/// `spawn_blocking`: three git calls, two of which list every worktree
/// in the repository -- ~150 on a real one.
pub async fn prune_worktrees(repo_path: String) -> Result<u64, String> {
    let repo = repo_path.clone();
    let cleared =
        tauri::async_runtime::spawn_blocking(move || crate::worktrees::prune_worktrees(&repo))
            .await
            .map_err(|e| format!("prune failed to run: {e}"))??;
    log::info!("{repo_path}: pruned {cleared} stale worktree registration(s)");
    Ok(cleared)
}

/// Everything the app already knows about one worktree's unmerged work.
///
/// `claudify_command` has always computed this whole struct and then
/// discarded all of it except a shell string -- so the app could say
/// "+240/-18 across 11 files, 4 commits ahead, last touched 3 weeks
/// ago" and instead asked the user to leave, paste a command into a
/// terminal, and wait for an agent to rediscover it.
///
/// `canClaudify` counts 124 of 268 worktrees in that state, which is the
/// largest single group. Claude Code stays for the genuine judgment
/// calls; these numbers triage the easy majority first.
///
/// `spawn_blocking`: several git calls per worktree, and it is opened
/// per row rather than per scan.
#[tauri::command]
pub async fn assess_worktree(
    repo_path: String,
    worktree_path: String,
    branch: String,
) -> Result<crate::worktrees::Assessment, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::worktrees::assess(&repo_path, &worktree_path, &branch)
    })
    .await
    .map_err(|e| format!("could not assess this worktree: {e}"))
}

#[tauri::command]
/// The shell command that hands a worktree to Claude Code.
///
/// Returns text for the clipboard rather than spawning anything.
/// Spawning a terminal is not portable: macOS has no default-terminal
/// concept at all (no LaunchServices handler exists, so a machine with
/// both Terminal.app and iTerm gives no way to know which the user
/// wants), and on Linux `x-terminal-emulator` is Debian-only while
/// `gio open` on a shell script opens an editor. The clipboard works
/// identically everywhere and lands the user in their OWN shell.
///
/// It also sidesteps PATH: `claude` lives in `~/.local/bin`, outside a
/// GUI app's PATH, but the pasted command runs in a login shell where it
/// resolves fine.
pub fn claudify_command(
    repo_path: String,
    worktree_path: String,
    branch: String,
) -> ClaudifyCommand {
    let facts = crate::worktrees::assess(&repo_path, &worktree_path, &branch);
    // Fall back to the bare name: the command is going to a login shell,
    // which resolves it even when this process could not.
    let claude = crate::auth::find_claude();
    let installed = claude.is_some();
    let bin = claude
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "claude".to_string());

    // NOTE: copying the command deliberately does NOT record an
    // assessment.
    //
    // It used to. The mark gates "Remove anyway…", and `commands.rs`
    // describes that flag as "the record that a human looked at what
    // would be lost" -- but copying a prompt is the START of an
    // assessment, not the end of one. Marking here armed a force-remove
    // button on a worktree nobody had actually read a verdict for, and
    // it did so seconds later when the query refetched, swapping a
    // narrow "Claudify" for a wide "Remove anyway…" and re-flowing every
    // column in the table.
    //
    // `mark_assessed` is what records it, called once the user says they
    // have read the result.

    ClaudifyCommand {
        command: facts.command(&bin),
        claude_installed: installed,
    }
}

#[tauri::command]
/// Worktrees that have been assessed and are still at the head they were
/// assessed at.
///
/// A branch that has moved since is dropped: the assessment described a
/// different state, and offering an override on a stale verdict is
/// exactly the mistake this feature could otherwise introduce.
pub fn assessed_worktrees(app: AppHandle) -> Vec<String> {
    let Ok(conn) = open_db(&db_path(&app)) else {
        return Vec::new();
    };
    let seen: std::collections::BTreeMap<String, String> =
        settings::get(&conn, settings::keys::ASSESSED_WORKTREES)
            .ok()
            .flatten()
            .unwrap_or_default();

    seen.into_iter()
        .filter(|(path, oid)| crate::worktrees::head_oid(path).is_ok_and(|current| &current == oid))
        .map(|(path, _)| path)
        .collect()
}

/// The clipboard payload, plus whether Claude Code was actually found.
///
/// `claude_installed` is advisory only: the command is copied either way,
/// because a user may be pasting it on another machine.
#[derive(Debug, serde::Serialize)]
pub struct ClaudifyCommand {
    pub command: String,
    pub claude_installed: bool,
}

#[tauri::command]
pub fn set_worktree_dirs(app: AppHandle, dirs: Vec<String>) -> Result<Vec<String>, String> {
    let ok = validate_dirs(dirs)?;
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    settings::set(&conn, settings::keys::WORKTREE_DIRS, &ok).map_err(|e| e.to_string())?;
    log::info!("worktree directories set to {} path(s)", ok.len());
    Ok(ok)
}

/// Trim, drop blanks, and reject anything that is not a directory.
///
/// Split from the command so it is testable without an AppHandle. A typo
/// must fail HERE, visibly, rather than being stored and producing an
/// empty worktrees view that looks like "you have no worktrees".
pub fn validate_dirs(dirs: Vec<String>) -> Result<Vec<String>, String> {
    let (ok, bad): (Vec<String>, Vec<String>) = dirs
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .partition(|d| std::path::Path::new(d).is_dir());

    if bad.is_empty() {
        Ok(ok)
    } else {
        Err(format!("not a directory: {}", bad.join(", ")))
    }
}

/// Tell the poll loop whether the active view needs live PR data.
///
/// Switching BACK to a PR view wakes the loop, so the list is fresh
/// immediately rather than after up to a full background interval.
/// Switching away does not wake it -- there is nothing to hurry for.
#[tauri::command]
pub fn set_view_needs_github(
    needs: bool,
    state: State<'_, crate::poll::ViewNeedsGithub>,
    waker: State<'_, crate::poll::Waker>,
) {
    let was = state.0.swap(needs, std::sync::atomic::Ordering::Relaxed);
    if needs && !was {
        waker.0.notify_one();
    }
}

/// The configured focused poll interval, in seconds.
#[tauri::command]
/// The newest published release, when it is newer than this build.
///
/// Distribution is dmg/exe/deb/AppImage, so no package manager carries
/// updates -- a user who installed a version with a launch-blocking bug
/// had no mechanism at all to discover the fix. That is not
/// hypothetical: v1.0.0 never left the splash screen on a second
/// machine, and v2.0.0 emptied both PR views on upgrade.
///
/// Unauthenticated and cheap: the releases endpoint needs no token, and
/// this runs once at startup rather than on the poll loop.
pub async fn latest_release(app: AppHandle) -> Option<String> {
    // The RUNTIME version, not CARGO_PKG_VERSION. The release workflow
    // stamps the tag into the manifests at build time and never commits
    // them, so the compiled-in constant reads 0.1.0 in a dev build and
    // would report every release as an update.
    let current = app.package_info().version.to_string();
    // Through the authenticated client, which already exists -- rather
    // than adding an HTTP dependency for one request. The endpoint is
    // public, so this works whether or not the token has any scopes.
    let json: serde_json::Value = octocrab::instance()
        .get("/repos/pktstorm/headstate/releases/latest", None::<&()>)
        .await
        .ok()?;
    let tag = json.get("tag_name")?.as_str()?.trim_start_matches('v');

    // A plain inequality, not a semver comparison. The published tag is
    // the only thing that ever appears here, and a wrong answer costs a
    // spurious "update available" rather than anything harmful -- where
    // pulling in a semver crate for one string compare would not repay
    // itself.
    (tag != current && !current.is_empty()).then(|| tag.to_string())
}

#[tauri::command]
pub fn get_poll_interval(state: State<'_, crate::poll::PollInterval>) -> u64 {
    state.0.load(std::sync::atomic::Ordering::Relaxed)
}

/// Which desktop notifications the user wants.
///
/// Absent means everything on, matching what the app did before this
/// setting existed -- an upgrade must not silently mute a feature.
#[tauri::command]
pub fn get_notify_prefs(app: AppHandle) -> crate::poll::NotifyPrefs {
    open_db(&db_path(&app))
        .ok()
        .and_then(|c| crate::store::settings::get(&c, settings::keys::NOTIFY_PREFS).ok())
        .flatten()
        .unwrap_or_default()
}

/// Interface preferences.
#[tauri::command]
pub fn get_ui_prefs(app: AppHandle) -> crate::poll::UiPrefs {
    read_ui_prefs(&app)
}

/// Read interface preferences, or the defaults if unreadable.
///
/// Shared with the window event handler, which needs
/// `close_hides_to_tray` and runs outside any command. Every failure
/// path returns the default, which is the app's pre-existing behaviour:
/// a database problem must not silently start QUITTING an app the user
/// expects to hide.
pub fn read_ui_prefs(app: &AppHandle) -> crate::poll::UiPrefs {
    open_db(&db_path(app))
        .ok()
        .and_then(|c| crate::store::settings::get(&c, settings::keys::UI_PREFS).ok())
        .flatten()
        .unwrap_or_default()
}

/// Whether the app is registered to start at login.
///
/// Asked of the OS rather than stored: the user can disable it from
/// System Settings, and a stored flag would then disagree with reality.
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Register or unregister start-at-login.
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let res = if enabled { mgr.enable() } else { mgr.disable() };
    res.map_err(|e| e.to_string())?;
    log::info!("start at login: {enabled}");
    Ok(())
}

/// Change interface preferences.
#[tauri::command]
pub fn set_ui_prefs(app: AppHandle, prefs: crate::poll::UiPrefs) -> Result<(), String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    crate::store::settings::set(&conn, settings::keys::UI_PREFS, &prefs)
        .map_err(|e| e.to_string())?;
    // Applied immediately rather than at the next launch. Someone who
    // just ticked the box to capture a problem should get the log for
    // the problem they are currently reproducing, not the next one.
    crate::diag::set_enabled(prefs.diagnostic_logging);
    log::info!(
        "ui: {} view(s) hidden, close_hides_to_tray={}, diagnostics={}",
        prefs.hidden_views.len(),
        prefs.close_hides_to_tray,
        prefs.diagnostic_logging
    );
    Ok(())
}

/// Change which desktop notifications are sent.
///
/// No waker: the poll loop reads this per tick, so the next poll picks it
/// up without being nudged. Unlike the poll interval there is nothing
/// in-memory to update -- the loop is the only reader.
#[tauri::command]
pub fn set_notify_prefs(app: AppHandle, prefs: crate::poll::NotifyPrefs) -> Result<(), String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    crate::store::settings::set(&conn, settings::keys::NOTIFY_PREFS, &prefs)
        .map_err(|e| e.to_string())?;
    // Counts only -- which repos break is not a setting and not logged.
    log::info!(
        "notifications: enabled={} ci={} conflicts={}",
        prefs.enabled,
        prefs.ci_failed,
        prefs.conflicted
    );
    Ok(())
}

/// Set the focused poll interval, clamped to the allowed range.
///
/// Wakes the poll loop so a SHORTENED interval takes effect immediately
/// rather than after the previous, longer sleep expires -- otherwise
/// dropping from an hour to a minute would appear to do nothing for up to
/// an hour. Returns the value actually applied, so the UI reflects the
/// clamp rather than showing a number the backend rejected.
#[tauri::command]
pub fn set_poll_interval(
    app: AppHandle,
    secs: u64,
    state: State<'_, crate::poll::PollInterval>,
    waker: State<'_, crate::poll::Waker>,
) -> u64 {
    let applied = crate::poll::clamp_interval(secs);
    state.0.store(applied, std::sync::atomic::Ordering::Relaxed);

    // Persist so the choice survives a relaunch. A write failure is
    // logged, not surfaced: the setting is already live in memory, and
    // refusing the change because the disk is unhappy would be worse than
    // forgetting it next launch.
    match open_db(&db_path(&app))
        .and_then(|c| crate::store::settings::set(&c, settings::keys::POLL_INTERVAL_SECS, &applied))
    {
        Ok(()) => log::info!("poll interval set to {applied}s"),
        Err(e) => log::warn!("poll interval set to {applied}s but not persisted: {e}"),
    }

    waker.0.notify_one();
    applied
}

/// How many pull requests await the user's review.
///
/// The sidebar badge needs a number on EVERY view, including ones that
/// show no pull requests. Asking for the count rather than the list
/// costs 1 rate-limit point against 6, and ~0.9s against ~4s.
#[tauri::command]
pub async fn count_reviewing(client: State<'_, GhClient>) -> Result<u64, String> {
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). Cheap and runs on every
    // view, so it doubles as a liveness check: if the badge count keeps
    // returning quickly while the list hangs, the account and token are
    // fine and the problem is specific to the heavy query.
    let started = std::time::Instant::now();
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let out = client.count_reviewing().await.map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd count_reviewing {}ms {:?}",
        started.elapsed().as_millis(),
        out
    );
    out
}

/// The cached review list, so To review paints real content instead of
/// an empty panel while the live query runs.
///
/// Never talks to GitHub -- the mirror of `get_cached` for the other
/// list. The query it stands in for takes ~20s on a 60-PR queue and
/// cannot be made meaningfully faster (see #328), so the only way to
/// stop the user staring at nothing is to have something to show.
#[tauri::command]
pub fn get_cached_reviewing(app: AppHandle) -> Result<CachedSnapshot, String> {
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    let out = load_snapshot_marked(&conn, CachedList::Reviewing).map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd get_cached_reviewing {}",
        match &out {
            Ok(v) => format!("ok n={} stale={:?}", v.prs.len(), v.stale_secs),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

#[tauri::command]
pub async fn get_reviewing(
    app: AppHandle,
    client: State<'_, GhClient>,
) -> Result<Vec<PullRequest>, String> {
    // DIAGNOSTIC LOGGING (Settings > diagnostic log). Brackets the whole
    // command, so the log distinguishes the three ways To review can
    // appear stuck: the command was never invoked (no start line), it
    // is still running (a start with no end), or it returned promptly
    // and the delay is in the frontend (a fast start/end pair).
    crate::diag!("[diag] cmd get_reviewing start");
    let started = std::time::Instant::now();
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let out = client
        .fetch_reviewing_with_shortfall()
        .await
        .map(|(prs, short)| {
            // Tell the UI when the list is SHORT. The 100 -> 50 fallback
            // returns fewer pull requests than exist and everything
            // downstream presented that as complete -- the v3.5.3 log
            // caught 50 shown against a count of 62, with twelve gone
            // silently. Emitted even when zero, so a recovered fetch
            // clears a banner an earlier one raised.
            if let Err(e) = app.emit("reviewing-short", short) {
                log::warn!("failed to emit reviewing-short: {e}");
            }
            // Cache it, so the next visit to To review paints from disk
            // instead of waiting out the query again. The measurements
            // on #328 are what make this the fix: the query itself
            // cannot be made fast (a bare 25-item search already costs
            // 6.2s, and trimming fields measured as noise), so the win
            // has to come from not blocking on it.
            //
            // A failed write is logged and swallowed: the caller has
            // real pull requests in hand, and refusing to return them
            // because a cache write failed would turn a slow path into
            // a broken one.
            match open_db(&db_path(&app)) {
                Ok(conn) => {
                    if let Err(e) = save_snapshot(&conn, CachedList::Reviewing, &prs) {
                        log::warn!("could not cache the review list: {e}");
                    }
                }
                Err(e) => log::warn!("could not open the store to cache the review list: {e}"),
            }
            prs
        })
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd get_reviewing end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            Ok(v) => format!("ok n={}", v.len()),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

#[tauri::command]
pub async fn get_periods(client: State<'_, GhClient>) -> Result<Periods, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    client
        .fetch_periods(chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_history(client: State<'_, GhClient>, days: i64) -> Result<History, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let days = clamp_days(days);
    client
        .fetch_history(chrono::Utc::now(), days)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_merged_detail(client: State<'_, GhClient>) -> Result<MergedDetail, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    client
        .fetch_merged_detail()
        .await
        .map_err(|e| e.to_string())
}

/// A COMPLETE count of pull requests for one subject and scope (#824).
///
/// The first command on the hardened stats layer, and deliberately the
/// only one: it is the narrowest thing that exercises all eight
/// protections end to end -- parameterised subject and scope,
/// connection-first routing, probe-driven slicing, bounded read
/// concurrency, a wall-clock ceiling, metered spend, an honest
/// partiality report, and the persistence cache.
///
/// No UI calls this yet. #825 builds the sidebar that chooses a scope and
/// #826 the views that render leaderboards; shipping the command now is
/// what makes the layer reachable and testable rather than dead code
/// waiting on two other PRs.
///
/// # Arguments
///
/// `subject` is a login, or `None` for the viewer -- NOT for "everyone".
/// A leaderboard's "everyone" is a different question and is
/// `StatsQuery`'s `None` subject; exposing that through this command would
/// make one parameter mean two things, so #826 gets its own command for it
/// rather than an overloaded flag here.
///
/// `scope_kind` is one of `repo`, `org`, `user`, `all`, with `scope_value`
/// carrying `owner/name` or the org/user login. Strings rather than a
/// tagged enum because this is the Tauri boundary: the phone's
/// `remote_call` passes JSON, and `surface::Args` reads scalars.
///
/// # Why the window is clamped
///
/// `clamp_days` exists for exactly this reason on `get_history`
/// (`commands.rs:37-46`): a Tauri command is a public surface, and an
/// unbounded value builds an arbitrarily large plan. Here the blast
/// radius is worse than a long query -- a 100-year window is probed,
/// subdivided, and probed again. The same clamp applies, and the slicer's
/// own `MAX_DEPTH` is the second line of defence.
#[tauri::command]
pub async fn stats_count(
    app: AppHandle,
    client: State<'_, GhClient>,
    subject: Option<String>,
    scope_kind: String,
    scope_value: Option<String>,
    measure: String,
    days: i64,
) -> Result<crate::github::stats::Outcome, String> {
    use crate::github::stats::{Budget, Measure, Scope, Slice, StatsQuery, Subject};

    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let days = clamp_days(days);

    let subject = match subject {
        // An empty string is a caller mistake, not a request for the
        // viewer: treating it as `@me` would silently answer a different
        // question than the one asked.
        Some(s) if s.trim().is_empty() => return Err("subject must not be empty".into()),
        Some(s) => Subject::Login(s),
        None => Subject::Viewer,
    };
    let scope = match (scope_kind.as_str(), scope_value) {
        ("repo", Some(v)) => Scope::Repo(v),
        ("org", Some(v)) => Scope::Org(v),
        ("user", Some(v)) => Scope::Personal(v),
        ("all", _) => Scope::All,
        (k, None) => return Err(format!("scope {k} needs a value")),
        (k, _) => return Err(format!("unknown scope: {k}")),
    };
    let measure = match measure.as_str() {
        "merged" => Measure::Merged,
        "opened" => Measure::Opened,
        other => return Err(format!("unknown measure: {other}")),
    };

    let now = chrono::Utc::now();
    // The window ends YESTERDAY, matching `query::period_ranges`
    // (`query.rs:247-252`): today is still accumulating, so including it
    // compares a partial day against complete ones. It is also what makes
    // the answer CACHEABLE -- see `store::stats::is_closed`.
    let end = now - chrono::Duration::days(1);
    let start = end - chrono::Duration::days(days - 1);
    let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y-%m-%d").to_string();
    let window = Slice::new(fmt(start), fmt(end));

    let q = StatsQuery::new(Some(subject), scope, measure);

    // Constructed BEFORE the viewer lookup, which is a change of order and
    // the point of it (#844): that lookup spends a rate-limit point, and
    // constructing the accumulator afterwards meant the point was spent
    // outside anything that could count it -- so `Spend.points` understated
    // by one per call while `is_exact()` returned true.
    let budget = Budget::new();
    // The cache key needs `@me` RESOLVED, because two accounts on one
    // machine share this database and a row keyed on the literal would be
    // served to whichever asked second. `fetch_viewer_metered` is one cheap
    // request and its result never changes for a session.
    let viewer = client
        .fetch_viewer_metered(&budget)
        .await
        .map_err(|e| e.to_string())?;
    // `Kind::Count`, not a bare query key: #836 put boards and series in
    // the same table, and a board in this scope has no subject at all, so
    // its key would otherwise be byte-identical to a whole-scope count's.
    let key = crate::store::stats::key(crate::store::stats::Kind::Count, &q.cache_key(&viewer));

    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    // Before any read: if the token now belongs to someone else, the rows
    // in this table are the previous user's (#840). Non-fatal -- the
    // answer below is correct either way, and failing a stats load
    // because a cache could not be tidied would be the tail wagging the
    // dog.
    note_stats_viewer(&conn, &viewer);
    if let Ok(Some(hit)) = crate::store::stats::get(&conn, &key, &window.from, &window.to, now) {
        if let Ok(mut cached) = serde_json::from_str::<crate::github::stats::Outcome>(&hit.payload)
        {
            crate::diag!("[diag] cmd stats_count cache hit total={}", hit.total);
            // A cache HIT still cost the viewer lookup, so it reports that
            // rather than the spend of the load that originally filled the
            // row. Stating a stale figure would be worse than either: a user
            // reading "9 points" on a request that spent 1 cannot tell
            // caching is working, and the number is not about this call at
            // all. Overwritten rather than added to, because the cached
            // value's requests happened in a different hour.
            cached.spend = budget.snapshot();
            return Ok(cached);
        }
        // A payload that will not parse is a shape change across an
        // upgrade. Dropped and re-fetched rather than erroring: the
        // cache is an optimisation and must never be able to break the
        // feature it accelerates.
        log::warn!("discarding an unreadable stats cache row");
    }

    // REFUSE before spending, so a load cannot be the thing that starves
    // the poll loop. The projection is the probe rounds plus one request
    // per chunk of slices, all at the measured 1 point each -- small, but
    // the point of the check is the one case where `remaining` is already
    // near the floor because something else spent it.
    //
    // The viewer lookup above has ALREADY landed by now, so `budget` carries
    // a real `remaining` here and `permits` is gating on this session's own
    // most recent reading rather than on the process-wide one alone (#843).
    let projected = u64::try_from(days).unwrap_or(u64::MAX) / 5 + 8;
    if !budget.permits(projected) {
        return Err(format!(
            "GitHub budget too low for this scope (needs about {projected} points, \
             keeping {} in reserve for background refresh)",
            crate::github::stats::budget::RESERVE
        ));
    }

    crate::diag!("[diag] cmd stats_count start days={days}");
    let started = std::time::Instant::now();
    let out = crate::github::stats::load_count(&client, &q, window.clone(), &budget)
        .await
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd stats_count end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            Ok(o) => format!(
                "ok total={} complete={} slices={} rounds={} points={}",
                o.total,
                o.is_complete(),
                o.slices,
                o.rounds,
                o.spend.points
            ),
            Err(e) => format!("err: {e}"),
        }
    );

    if let Ok(o) = &out {
        // Cached on success only. A failed load has nothing worth
        // remembering, and a partial one is stored WITH its partiality
        // (`complete`) so it cannot be read back as a confident number.
        if let Ok(payload) = serde_json::to_string(o) {
            if let Err(e) = crate::store::stats::put(
                &conn,
                &key,
                &window.from,
                &window.to,
                o.total,
                o.is_complete(),
                &payload,
                now,
            ) {
                // Non-fatal: the answer is already correct, and failing
                // the command because the cache could not be written
                // would turn an optimisation into a liability.
                log::warn!("could not cache the stats answer: {e}");
            }
        }
    }
    out
}

/// The scope hierarchy the PR Stats sidebar renders (#825).
///
/// Organisations with their repositories and members, plus the viewer's own
/// repositories. No statistics: this is the DISCOVERY half of
/// `hooks.ts:712-717`'s rule, and it runs on entering the view, so it has
/// to be cheap enough that arriving at a page costs nothing anyone would
/// notice. MEASURED at **2 rate-limit points and ~1.6s total** for this
/// account's whole hierarchy (2 orgs, 8 members, 59 org repositories, 6
/// personal ones) -- see `github::stats::tree` for the figures per request.
///
/// # Why this replaces the local-repo list rather than adding to it
///
/// The sidebar listed `repoCounts(prs)` -- repositories where the viewer
/// has an OPEN PR (`src/lib/repos.ts`). That list cannot hold an
/// organisation or a person, so the second audience #823 names ("how is my
/// team doing?") had nowhere to be asked from, and a repository with no
/// current PR was missing even though its history is what a lead wants.
///
/// # No `Budget::permits` check, unlike `stats_count`
///
/// `stats_count` refuses to start when the remaining budget is near the
/// `RESERVE` floor, because a scope load can cost dozens of points and the
/// thing being protected is the poll loop's standing obligation. This costs
/// **2**, measured, and refusing it would mean a sidebar that cannot draw
/// its own rows -- leaving the user no way to see WHICH scope they might
/// load, on the one screen whose job is to say what exists. Two points is
/// inside the noise of a single poll, and the expensive thing the user
/// might click from here is still gated by `stats_count`'s own check.
///
/// # Not cached, deliberately
///
/// `stats_count` caches through `store::stats` because a closed window's
/// answer cannot change and recomputing it per navigation is waste. This
/// does not, and the asymmetry is the point: membership changes, people
/// join, repositories are created, and a sidebar built from a stale roster
/// offers scopes that may no longer exist. At 2 points against a
/// 5,000-point hourly budget the freshness is worth more than the saving --
/// and TanStack Query's `staleTime` on the frontend already stops it
/// re-running within a session, which is the layer where "do not re-ask
/// while the user is still here" belongs.
#[tauri::command]
pub async fn stats_tree(client: State<'_, GhClient>) -> Result<crate::github::stats::Tree, String> {
    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;

    crate::diag!("[diag] cmd stats_tree start");
    let started = std::time::Instant::now();
    // The wall-clock ceiling is INSIDE `load_tree`, the way `load_count`
    // and `load_detail` carry theirs: the layer that knows how many
    // requests it issues is the layer that should bound them, and a
    // ceiling applied out here would be a second place for a future
    // third request to escape.
    let out = crate::github::stats::load_tree(&client)
        .await
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd stats_tree end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            // Counts and logins, never a repository NAME: this repo's
            // privacy rule applies to the diagnostic log too, and the
            // shape of the tree is what a reader of the log needs.
            Ok(t) => format!(
                "ok orgs={}/{} personal={}/{} unreadable={} complete={} points={}",
                t.orgs.len(),
                t.orgs_total,
                t.personal.len(),
                t.personal_total,
                t.unreadable_orgs().count(),
                t.is_complete(),
                t.spend.points
            ),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

/// A scope and window parsed from the Tauri boundary's scalars.
///
/// Shared by `stats_board` and `stats_series` so the two cannot disagree
/// about what a window is. That matters more than it sounds: the two
/// render on the SAME page, and a board covering 30 days beside a chart
/// covering 31 would produce a page whose own numbers contradict each
/// other with nothing on screen to explain it.
struct ScopeRequest {
    scope: crate::github::stats::Scope,
    window: crate::github::stats::Slice,
    /// Every day in the window, oldest first, `YYYY-MM-DD`.
    days: Vec<String>,
}

/// Parse the scope scalars and derive the window.
///
/// `scope_kind` is one of `repo`, `org`, `user`, `all`, with `scope_value`
/// carrying `owner/name` or the org/user login -- exactly the strings
/// `StatsSidebar` writes through `setStatsScope`, so a clicked row needs no
/// translation. Strings rather than a tagged enum because this is the Tauri
/// boundary and the phone's `remote_call` passes JSON scalars
/// (`surface::Args`).
///
/// `days` is clamped for `clamp_days`' reason (`commands.rs:37-46`): a
/// Tauri command is a public surface and an unbounded value builds an
/// arbitrarily large plan. Here the blast radius is worse than one long
/// query, because the planner probes, subdivides and probes again.
fn parse_scope_request(
    scope_kind: &str,
    scope_value: Option<String>,
    days: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ScopeRequest, String> {
    use crate::github::stats::{Scope, Slice};

    let scope = match (scope_kind, scope_value) {
        ("repo", Some(v)) => Scope::Repo(v),
        ("org", Some(v)) => Scope::Org(v),
        ("user", Some(v)) => Scope::Personal(v),
        ("all", _) => Scope::All,
        (k, None) => return Err(format!("scope {k} needs a value")),
        (k, _) => return Err(format!("unknown scope: {k}")),
    };
    let days = clamp_days(days);
    // The window ends YESTERDAY, matching `query::period_ranges`
    // (`query.rs:247-252`) and `stats_count`: today is still accumulating,
    // so including it compares a partial day against complete ones. It is
    // also what makes an answer CACHEABLE -- see `store::stats::is_closed`.
    let end = now - chrono::Duration::days(1);
    let start = end - chrono::Duration::days(days - 1);
    let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y-%m-%d").to_string();
    let all_days: Vec<String> = (0..days)
        .map(|i| fmt(start + chrono::Duration::days(i)))
        .collect();
    Ok(ScopeRequest {
        scope,
        window: Slice::new(fmt(start), fmt(end)),
        days: all_days,
    })
}

/// Note the current `@me` identity, dropping another user's cached rows.
///
/// A thin wrapper over `store::stats::note_viewer` so the three stats
/// commands share one call site's error handling rather than each
/// swallowing the `Result` its own way.
///
/// Failure is logged and swallowed for the reason every other cache
/// interaction here is: the answer the command returns is correct
/// regardless, and turning a tidy-up into a command failure would make an
/// optimisation a liability. The cost of the failure is that a departed
/// user's rows stay until the next successful call -- they are never
/// SERVED, because keys resolve `@me` to a login (`Subject::cache_key`).
fn note_stats_viewer(conn: &rusqlite::Connection, viewer: &str) {
    match crate::store::stats::note_viewer(conn, viewer) {
        Ok(0) => {}
        Ok(n) => log::info!("the stats cache dropped {n} rows for a previous identity"),
        Err(e) => log::warn!("could not record the stats viewer: {e}"),
    }
}

/// Upper bound on what a board load will spend, in rate-limit points.
///
/// # Why this is not `stats_count`'s figure
///
/// `stats_count` projects `days / 5 + 8`, which is right for the COUNT path:
/// that path subdivides only when a slice approaches the 1,000-result cap, so
/// a 30-day window is usually one or two slices and the 8 covers the probes.
///
/// A board subdivides to the PAGE (see `fetch::plan_to`), so its plan is far
/// finer and its request count is driven by the DAY COUNT rather than by how
/// near the cap the window is. The worst case is one slice per day, which is
/// the floor the date grammar imposes -- GitHub has no sub-day range -- and
/// it is reached by any scope busy enough to exceed a 50-node page every day.
///
/// So the bound is computed from the two fan-outs rather than guessed:
///
/// - **Probes.** One request per `query::ALIAS_CHUNK` slices per round, and
///   at most `MAX_PROBE_ROUNDS` rounds before every slice is a single day.
/// - **Detail.** One request per `board::BOARD_ALIAS_CHUNK` slices.
/// - **Plus one** for the `fetch_viewer` call the split needs.
///
/// I found the old arithmetic wrong by checking it against this: `days/5 + 8`
/// gives 14 for a 30-day window whose worst case is ~15 requests, and 26 for
/// a 90-day window whose worst case is ~45. Under-projecting is the dangerous
/// direction, because the check exists precisely to stop a load starting that
/// then runs the budget below `RESERVE` and starves the poll loop -- the one
/// part of the app with a standing obligation.
///
/// For reference, the MEASURED figure on a real 30-day org window (569 merged
/// pull requests, 22 slices) was **9 points**, against the 24 this bounds it
/// at. Deliberately loose: a refusal here costs the user a page they asked
/// for, so the bound should be wrong in the direction of letting a real load
/// through, while still being an upper bound rather than a typical one.
fn board_projection(days: i64) -> u64 {
    let days = u64::try_from(days).unwrap_or(u64::MAX);
    let alias_chunk = crate::github::stats::query::ALIAS_CHUNK as u64;
    let detail_chunk = crate::github::stats::board::BOARD_ALIAS_CHUNK as u64;
    // Worst case is one slice per day: the date grammar cannot cut finer.
    let slices = days.max(1);
    let probes = slices.div_ceil(alias_chunk) * MAX_PROBE_ROUNDS;
    let detail = slices.div_ceil(detail_chunk);
    // +1 for `fetch_viewer`.
    probes + detail + 1
}

/// Probe rounds the board's projection assumes, as an upper bound.
///
/// `slice::MAX_DEPTH` is 24 and is the RECURSION guard, not a realistic
/// round count -- projecting against it would refuse almost every load. The
/// planner splits proportionally and is capped at `ALIAS_CHUNK` pieces per
/// split, so reaching one-day slices from a 90-day window takes
/// ceil(log10(90)) = 2 rounds in principle and measured 3 on a real 30-day
/// window. 4 is that measured figure plus one.
const MAX_PROBE_ROUNDS: u64 = 4;

/// Per-author aggregates for one scope: #826's Mine and Others views and
/// the three leaderboards, in ONE load.
///
/// # Why one command and not two
///
/// Mine and Others are the same measurement partitioned two ways, not two
/// measurements. The board carries every author who appears in the window;
/// "Mine" is the viewer's row and "Others" is the rest
/// (`Board::row_for` / `Board::others`). Issuing a narrowed
/// `author:@me` query as well would double the cost of the page to
/// recompute a row the board already holds, and the two answers could then
/// disagree -- a Mine figure that does not match the viewer's own entry on
/// the leaderboard beside it is the kind of contradiction a reader cannot
/// resolve and will not trust.
///
/// It is also why `subject` is not a parameter here. A board asks about
/// EVERYONE; `scope.rs` has a test named for the mistake of constraining
/// one to a single author, which renders a board with one name on it. The
/// viewer's login is resolved server-side so the split can be made, and a
/// member row's subject is applied by the UI to that same board rather than
/// by re-querying.
///
/// # Cost, and why this one is gated
///
/// Unlike `stats_tree`, this is the expensive click. The probe rounds plus
/// one detail request per `board::BOARD_ALIAS_CHUNK` slices, each measured
/// at 1 point, so the projection below is the same shape `stats_count`
/// uses. `Budget::permits` refuses before spending, because the thing being
/// protected is the poll loop's standing obligation and a leaderboard is
/// something the user asked for once.
///
/// # Cached for a CLOSED window, since #836
///
/// It was not, and the reason recorded here was scope: "a board is not a
/// number… caching it properly needs a migration of its own". #836 is that
/// follow-up, and no migration turned out to be needed -- migration 10's
/// `payload` is already an opaque JSON blob and `complete` already carries
/// partiality, so the only genuine blocker was the key. A board has no
/// subject (it is about everyone), so `StatsQuery::cache_key` would have
/// given it the same string as the scope's whole-population COUNT;
/// `store::stats::Kind` is the discriminator that fixes it.
///
/// The asymmetry #836 objected to was that the cache covered the CHEAPEST
/// query and skipped this one. `board_projection` bounds a 90-day board at
/// ~45 points against `stats_count`'s ~26, and the node fetching is
/// "seconds over a busy org" -- so a cold start was re-spending the full
/// board cost for a leaderboard over a month that had already ended.
///
/// Two things it is careful NOT to do, both from #836's acceptance list:
///
/// - **An open window is not cached as if final.** `store::stats::get`
///   already bounds an open window to `OPEN_WINDOW_TTL_SECS`, so the write
///   is safe either way; it is still worth saying that the window here
///   ends YESTERDAY (`parse_scope_request`), which is what makes most
///   requests land on a closed window at all.
/// - **A partial board is stored WITH its partiality.** `board.complete`
///   goes into the `complete` column, so a read-back cannot launder a
///   capped roster into a confident one.
///
/// The roster question the old comment raised -- whether a membership
/// change should invalidate a closed window's board -- answers itself:
/// the board is built from the pull requests merged in the window, not
/// from a roster, so who is in the org TODAY cannot change who merged
/// something in August. (That is not true of the reviews-GIVEN board,
/// which takes its logins as an argument; `stats_reviewers` is not cached
/// here for exactly that reason.)
#[tauri::command]
pub async fn stats_board(
    app: AppHandle,
    client: State<'_, GhClient>,
    scope_kind: String,
    scope_value: Option<String>,
    measure: String,
    days: i64,
) -> Result<StatsBoard, String> {
    use crate::github::stats::{Budget, Measure};

    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let measure = match measure.as_str() {
        "merged" => Measure::Merged,
        "opened" => Measure::Opened,
        other => return Err(format!("unknown measure: {other}")),
    };
    let now = chrono::Utc::now();
    let req = parse_scope_request(&scope_kind, scope_value, days, now)?;

    // Constructed BEFORE the viewer lookup, which spends a point that used to
    // land outside any accumulator (#844) -- `board_projection` already
    // budgeted for it (`// +1 for fetch_viewer.`), so the projection knew
    // about a request the accounting did not.
    let budget = Budget::new();
    // Resolved so the UI can split the board into Mine and Others. One
    // cheap request whose answer never changes for a session, and the same
    // call `stats_count` makes for its cache key -- the viewer's login is
    // genuinely needed here rather than avoidable, because `@me` is a
    // qualifier GitHub resolves and not a login the UI can compare a row
    // against.
    let viewer = client
        .fetch_viewer_metered(&budget)
        .await
        .map_err(|e| e.to_string())?;

    // A board has no subject, so `cache_key` fills the subject slot with
    // `*` and the key reads `board|merged|*|org:X`. `Kind::Board` is what
    // keeps that out of the whole-scope COUNT's row, which is the same
    // string bar the prefix.
    //
    // Note what this means and why it is safe: unlike `stats_count`'s key,
    // this one does NOT carry the viewer's login -- there is no subject for
    // it to be resolved into. Two accounts on one machine share this
    // database (`Subject::cache_key`'s doc records that as a real case), so
    // in principle one could read a board row the other wrote, and a
    // `StatsBoard` payload carries the viewer it was split into Mine and
    // Others against -- which would put the reader's own work under
    // "Others" and show "no activity" for Mine.
    //
    // `note_stats_viewer` below is what closes it, and it has to run before
    // the read for exactly this reason: a changed identity drops every row
    // in the table, so the only rows this read can find were written by the
    // account now asking. That ordering is load-bearing rather than tidy.
    //
    // The alternative -- putting the viewer in the key -- was rejected: it
    // would give every account its own copy of an answer that is identical
    // for all of them (a board is about everyone in the scope), so the
    // second user would pay the full ~45-point load to recompute a
    // leaderboard already on disk.
    let q = crate::github::stats::StatsQuery::new(None, req.scope.clone(), measure);
    let key = crate::store::stats::key(crate::store::stats::Kind::Board, &q.cache_key(&viewer));
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    note_stats_viewer(&conn, &viewer);
    if let Ok(Some(hit)) =
        crate::store::stats::get(&conn, &key, &req.window.from, &req.window.to, now)
    {
        if let Ok(cached) = serde_json::from_str::<StatsBoard>(&hit.payload) {
            crate::diag!(
                "[diag] cmd stats_board cache hit authors={} complete={}",
                cached.board.rows.len(),
                cached.board.complete
            );
            return Ok(cached);
        }
        // Same handling as `stats_count`'s: a payload that will not parse
        // is a shape change across an upgrade, dropped and re-fetched
        // rather than erroring. A cache must never be able to break the
        // feature it accelerates.
        log::warn!("discarding an unreadable stats board cache row");
    }

    let budget = Budget::new();
    let projected = board_projection(clamp_days(days));
    if !budget.permits(projected) {
        return Err(format!(
            "GitHub budget too low for this scope (needs about {projected} points, \
             keeping {} in reserve for background refresh)",
            crate::github::stats::budget::RESERVE
        ));
    }

    crate::diag!("[diag] cmd stats_board start kind={scope_kind} days={days}");
    let started = std::time::Instant::now();
    // `req.window` is CLONED rather than moved: the cache write below
    // needs the same window the fetch was for, and re-deriving it from
    // `days` there would be a second computation of the same dates that a
    // midnight boundary could make disagree with this one.
    let window = req.window.clone();
    let out = crate::github::stats::load_board(&client, &req.scope, measure, window, &budget)
        .await
        .map(|board| StatsBoard { viewer, board })
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd stats_board end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            // Counts and flags, never a login: this is a public repo and
            // the privacy rule applies to the diagnostic log too. The
            // shape of the board is what a reader of the log needs, and
            // naming colleagues in it would be the one place this feature
            // could leak a roster.
            Ok(b) => format!(
                "ok authors={} total={} retrieved={} complete={} short={} refused={} \
                 slices={} rounds={} points={}",
                b.board.rows.len(),
                b.board.total,
                b.board.retrieved,
                b.board.complete,
                b.board.truncated_slices.len(),
                b.board.refused_fields,
                b.board.slices,
                b.board.rounds,
                b.board.spend.points
            ),
            Err(e) => format!("err: {e}"),
        }
    );

    if let Ok(b) = &out {
        // Cached on success only, and the board's OWN `complete` flag goes
        // into the `complete` column -- a partial roster must not read
        // back as a confident one. `store::stats::get` is what bounds an
        // open window, so there is no separate is-it-closed check here:
        // one decision, in one place, rather than two that can disagree.
        //
        // The whole `StatsBoard` is serialised, viewer login included,
        // because the split has to travel with the board it was made
        // against (see `StatsBoard`).
        if let Ok(payload) = serde_json::to_string(b) {
            if let Err(e) = crate::store::stats::put(
                &conn,
                &key,
                &req.window.from,
                &req.window.to,
                b.board.total,
                b.board.complete,
                &payload,
                now,
            ) {
                log::warn!("could not cache the stats board: {e}");
            }
        }
    }
    out
}

/// A board plus the viewer's login, which is what splits it into Mine and
/// Others.
///
/// The login travels WITH the board rather than being fetched separately by
/// the UI, because the two have to agree. A board fetched for one account
/// and split by a login cached from another -- two accounts on one machine,
/// which `Subject::cache_key`'s doc comment records as a real case -- would
/// put the viewer's own work under "Others" and show "no activity" for
/// Mine. Shipping them together makes that unrepresentable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsBoard {
    pub viewer: String,
    #[serde(flatten)]
    pub board: crate::github::stats::Board,
}

/// The scoped daily activity series: merged and opened counts per day.
///
/// The scoped counterpart to `get_history`, which is hardcoded
/// `author:@me`. A SEPARATE command from `stats_board` deliberately, and
/// that is the progressive-rendering requirement rather than a style
/// choice: `StatsPage.tsx:12-22` records that three independent queries
/// rendering as each lands beat one combined gate, because the costs differ
/// enough that blocking on the slowest left the fast numbers finished and
/// invisible (1.6s / 3.7s / 3.7s). This series is count-only and measured
/// at 1.4-1.5s per 10-day chunk, where a board over a busy org is seconds
/// of node fetching -- so folding them together would hide the chart behind
/// the leaderboard for no reason.
///
/// `subject` is accepted here, unlike on `stats_board`, and the asymmetry
/// is the point: a chart is about ONE line, so "this person's activity in
/// this org" is a legitimate and cheap question, while a leaderboard is
/// about everyone by definition. `None` means the whole scope.
///
/// # Cached, since #836
///
/// The third of the three, and the one whose answer is most obviously a
/// constant: a chart of last month is a row of numbers about days that
/// have ended. `store::stats::Kind::Series` keys it apart from the count
/// and the board over the same scope and window.
///
/// `total` on the cached row is the sum of the daily MERGED counts. The
/// column is part of `store::stats`' shape and the series has no single
/// headline figure, so something has to go there; merged is the measure
/// this command's own query defaults to, and nothing reads the column back
/// -- `Cached::payload` is what the command deserialises. Stated here
/// rather than left to a reader to infer, because a number in a column
/// named `total` invites exactly that inference.
///
/// Partiality is `Series::is_complete()`, which is false when any day
/// failed or any field was refused. A series missing two days of thirty is
/// still worth drawing (see `load_series`) -- but it must not be cached as
/// though it were whole, or the missing days become permanent.
#[tauri::command]
pub async fn stats_series(
    app: AppHandle,
    client: State<'_, GhClient>,
    subject: Option<String>,
    scope_kind: String,
    scope_value: Option<String>,
    days: i64,
) -> Result<crate::github::stats::Series, String> {
    use crate::github::stats::{Budget, Measure, StatsQuery, Subject};

    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let subject = match subject {
        // An empty string is a caller mistake, not a request for everyone:
        // treating it as `None` would silently widen a chart about one
        // person into one about a whole organisation, which looks like
        // that person having a remarkable month.
        Some(s) if s.trim().is_empty() => return Err("subject must not be empty".into()),
        Some(s) => Some(Subject::Login(s)),
        None => None,
    };
    let now = chrono::Utc::now();
    let req = parse_scope_request(&scope_kind, scope_value, days, now)?;

    let budget = Budget::new();
    // One request per `ALIAS_CHUNK` days, at the measured 1 point each,
    // plus slack. Far cheaper than a board, and gated anyway: the check
    // exists for the case where something else has already spent the
    // budget down to the reserve, which is independent of how cheap this
    // particular call is.
    let projected = u64::try_from(clamp_days(days)).unwrap_or(u64::MAX)
        / crate::github::stats::query::ALIAS_CHUNK as u64
        + 2;
    if !budget.permits(projected) {
        return Err(format!(
            "GitHub budget too low for this chart (needs about {projected} points, \
             keeping {} in reserve for background refresh)",
            crate::github::stats::budget::RESERVE
        ));
    }

    // `Measure::Merged` names the query's DEFAULT measure and is
    // immediately overridden per alias by `series_query`, which asks for
    // both. Passed rather than defaulted inside the document builder so
    // there is no measure a caller can set here and have silently ignored.
    let q = StatsQuery::new(subject, req.scope, Measure::Merged);

    // The viewer is needed for the KEY, not for the answer -- a series
    // about a named subject does not otherwise care who is asking. It is
    // the same one cheap request `stats_count` and `stats_board` make, and
    // `cache_key` needs `@me` resolved for the reason those two record:
    // two accounts on one machine share this database.
    //
    // The key's MEASURE slot reads `merged` for every series, and that is
    // correct rather than a bug: `q` carries `Measure::Merged` only as the
    // document's default, and `series_query` overrides it per alias to ask
    // for both. So one cached row holds both measures -- there is no
    // opened-only series for it to collide with. `stats_count` and
    // `stats_board` DO vary by measure, and theirs is in the key because
    // they pass the real one.
    //
    // The subject IS in the key, unlike the board's: a series draws one
    // line, so "this person in this org" and "the whole org" are different
    // charts and must not share a row.
    let viewer = client
        .fetch_viewer_metered(&budget)
        .await
        .map_err(|e| e.to_string())?;
    let key = crate::store::stats::key(crate::store::stats::Kind::Series, &q.cache_key(&viewer));
    let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
    note_stats_viewer(&conn, &viewer);
    if let Ok(Some(hit)) =
        crate::store::stats::get(&conn, &key, &req.window.from, &req.window.to, now)
    {
        if let Ok(cached) = serde_json::from_str::<crate::github::stats::Series>(&hit.payload) {
            crate::diag!(
                "[diag] cmd stats_series cache hit points={}",
                cached.points.len()
            );
            return Ok(cached);
        }
        log::warn!("discarding an unreadable stats series cache row");
    }

    crate::diag!("[diag] cmd stats_series start kind={scope_kind} days={days}");
    let started = std::time::Instant::now();
    let out = crate::github::stats::load_series(&client, &q, &req.days, &budget)
        .await
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd stats_series end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            Ok(s) => format!(
                "ok points={} failed={} refused={} complete={} points_spent={}",
                s.points.len(),
                s.failed_days.len(),
                s.refused_fields,
                s.is_complete(),
                s.spend.points
            ),
            Err(e) => format!("err: {e}"),
        }
    );

    if let Ok(series) = &out {
        if let Ok(payload) = serde_json::to_string(series) {
            // See the doc above for why `total` is the merged sum.
            let total = series.points.iter().map(|p| p.merged).sum();
            if let Err(e) = crate::store::stats::put(
                &conn,
                &key,
                &req.window.from,
                &req.window.to,
                total,
                series.is_complete(),
                &payload,
                now,
            ) {
                log::warn!("could not cache the stats series: {e}");
            }
        }
    }
    out
}

/// How many reviewer logins one call will accept.
///
/// A ceiling rather than a trusted input, for `clamp_days`' reason
/// (`commands.rs:37-46`): a Tauri command is a public surface, and the
/// argument here is a LIST, so an unbounded one builds an arbitrarily wide
/// fan-out -- the same blast radius `clamp_days` exists for, one axis over.
///
/// 100 because that is exactly what the sidebar can hand over: `tree::PAGE`
/// is 100, so an org's Members list arrives capped at 100 and a caller
/// sending more is sending something it did not read off a tree. Matching
/// the roster cap rather than undercutting it means this clamp never fires
/// on real input -- a clamp that silently truncated a 60-member org would be
/// a second, invisible truncation on top of the tree's own reported one.
///
/// Sized against the measurement as well: 36 reviewer aliases measured
/// 3.62-4.15s at cost 1 (`query::reviewer_query`), and 100 chunked at
/// `ALIAS_CHUNK` is ten requests, which fits inside two `READ_CONCURRENCY`
/// waves and so costs roughly two chunks of wall clock rather than ten --
/// comfortably inside `LOAD_TIMEOUT`.
///
/// The truncation this DOES impose is visible rather than silent: the caller
/// sees fewer rows than it sent logins, and `Reviewers::unmeasured` names
/// every login that did not come back for any other reason.
const MAX_REVIEWER_LOGINS: usize = 100;

/// The reviews-GIVEN leaderboard: who reviewed the most in a scope (#826).
///
/// # Why this is its own command rather than a field on `stats_board`
///
/// `stats_board` reads `reviews { totalCount }` off pull request nodes,
/// which counts reviews a PR RECEIVED and attributes them to its AUTHOR. A
/// node never says who wrote its reviews, so reviews given are not derivable
/// from that document at any price -- they need `reviewed-by:<login>`, one
/// search per person. #829 shipped the correctly-titled "most-reviewed"
/// board rather than print the received measure under "top reviewers", and
/// that call was right; this is the missing half rather than a correction to
/// it. Both boards ship, labelled for what each measures.
///
/// The two genuinely name DIFFERENT people on this account's own data
/// (MEASURED live 2026-09-11): a `reviewed-by:<viewer>` search over an org
/// window returned two pull requests, both AUTHORED BY SOMEONE ELSE and each
/// carrying `reviews { totalCount } == 1`. So the same two pull requests
/// credit the AUTHOR on the received board and the REVIEWER on the given one
/// -- different names, same rows of data.
///
/// # Why the LOGINS are an argument
///
/// The roster is already on screen. `stats_tree` enumerated it for the
/// sidebar at 2 points (#825), and the caller holds `org.members` for the
/// scope the user clicked -- so asking GitHub for it a second time here
/// would spend a request to re-derive a list the frontend already has, and
/// could disagree with the Members rows beside the board if the two reads
/// straddled a roster change.
///
/// The honest consequence, which the UI states rather than this command
/// hiding: this board covers the people in the list, NOT everyone who
/// reviewed. A reviewer from outside the org -- an outside collaborator, a
/// bot -- is absent, because nothing enumerated them. That is a bounded and
/// explainable gap; the alternative, enumerating reviewers from the PR nodes
/// themselves, cannot be done at all (see above) and enumerating them from
/// the org roster is exactly what this does.
///
/// # Cost
///
/// ONE request per `ALIAS_CHUNK` logins, each measured at 1 point: 4
/// reviewer aliases cost 1 at 0.84-1.04s, 36 cost 1 at 3.62-4.15s (live API,
/// 2026-09-11, `org:FNX-Labs`; the table is in `query::reviewer_query`). So
/// the real org size of 4 members is ONE request and one point -- the
/// cheapest load on this page, cheaper than the daily series. The budget
/// check is here anyway for `stats_series`' reason: it guards against
/// something else having already spent the budget to the reserve, which is
/// independent of how cheap this particular call is.
#[tauri::command]
pub async fn stats_reviewers(
    client: State<'_, GhClient>,
    scope_kind: String,
    scope_value: Option<String>,
    logins: Vec<String>,
    days: i64,
) -> Result<crate::github::stats::Reviewers, String> {
    use crate::github::stats::{Budget, Measure, StatsQuery};

    let client = client.0.clone().ok_or_else(|| AUTH_ERR.to_string())?;
    let now = chrono::Utc::now();
    let req = parse_scope_request(&scope_kind, scope_value, days, now)?;

    // Deduplicated and emptied-out before anything is spent. A repeated
    // login would build two aliases counting the same person and render them
    // as two rows on one leaderboard -- a ranking with a name in it twice,
    // which is worse than a missing row because it looks like data. An empty
    // or blank entry would build `reviewed-by: ` and match everything in
    // scope, attributing the whole window to a nameless row.
    let mut seen = std::collections::HashSet::new();
    let logins: Vec<String> = logins
        .into_iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && seen.insert(l.clone()))
        .take(MAX_REVIEWER_LOGINS)
        .collect();
    if logins.is_empty() {
        return Err("no reviewer logins to count".into());
    }

    let budget = Budget::new();
    // One request per `ALIAS_CHUNK` logins at 1 point each, plus slack.
    let projected =
        (logins.len() as u64).div_ceil(crate::github::stats::query::ALIAS_CHUNK as u64) + 1;
    if !budget.permits(projected) {
        return Err(format!(
            "GitHub budget too low for this leaderboard (needs about {projected} points, \
             keeping {} in reserve for background refresh)",
            crate::github::stats::budget::RESERVE
        ));
    }

    // No SUBJECT at all, which is the same rule `stats_board` follows and
    // for the same reason: a leaderboard asks "who, among everyone here",
    // and an author qualifier would narrow it to one person's pull requests
    // that each reviewer touched -- a third question. `StatsQuery::reviewed_by`
    // drops the author unconditionally, so this cannot be set wrong here, but
    // passing `None` keeps the intent visible at the call site.
    //
    // `Measure::Merged`, deliberately and not as a default: a review is work
    // done ON a pull request that landed, and counting reviews on PRs merely
    // OPENED in the window would count review of work still in flight --
    // which moves a reviewer up the board for a PR that may never merge.
    let q = StatsQuery::new(None, req.scope, Measure::Merged);

    crate::diag!(
        "[diag] cmd stats_reviewers start kind={scope_kind} people={} days={days}",
        logins.len()
    );
    let started = std::time::Instant::now();
    let out = crate::github::stats::load_reviewers(&client, &q, &logins, &req.window, &budget)
        .await
        .map_err(|e| e.to_string());
    crate::diag!(
        "[diag] cmd stats_reviewers end {}ms {}",
        started.elapsed().as_millis(),
        match &out {
            // Counts and flags, never a login: this is a public repo and the
            // privacy rule applies to the diagnostic log too, which is the
            // same rule `stats_board`'s line above follows. A reviewer board
            // is the one place this feature could leak a roster.
            Ok(r) => format!(
                "ok rows={} unmeasured={} refused={} complete={} points={}",
                r.rows.len(),
                r.unmeasured.len(),
                r.refused_fields,
                r.is_complete(),
                r.spend.points
            ),
            Err(e) => format!("err: {e}"),
        }
    );
    out
}

/// Whether we have a usable GitHub client. `state` is computed once at
/// startup from `auth::read_token` / `auth::build_client` and stored as
/// managed state; this command just hands it to the frontend.
#[tauri::command]
pub fn get_auth_state(state: State<'_, AuthState>) -> AuthState {
    state.inner().clone()
}

/// Import the Claude Code transcripts already on disk (#914, epic #910).
///
/// A FULL rescan of `~/.claude/projects`, every time. That is the whole
/// design and it is a measurement, not a shortcut: extracting all 1,430
/// real sessions on the development machine costs well under a second,
/// because each transcript is a bounded head read plus one 16 KB tail
/// seek rather than a whole-file read of an 881 MB corpus. At that price
/// there is no cache to invalidate, no stored offset to get wrong, and no
/// filesystem watcher to fail silently -- which on macOS a dead FSEvents
/// stream does, producing exactly the "watch that fails" the epic's own
/// constraints warn about.
///
/// `elapsed_ms` comes back in the result so that claim stays checkable on
/// a machine other than the one it was measured on.
///
/// # Why this is safe to call at startup
///
/// It is read-only against `~/.claude` -- those transcripts are Claude
/// Code's data and the file `claude --resume` depends on -- and one
/// transaction against our own cache. `spawn_blocking` keeps the disk
/// walk off the async runtime.
///
/// # Absent is not zero
///
/// [`Imported`] carries `unreadable_dirs`, `unreadable_files` and
/// `write_failures` as data. An empty session list with a non-empty
/// `unreadable_dirs` means "we could not read your history", which the UI
/// must not render as "you have no sessions": the two have opposite
/// remedies and the second is alarming when it is false.
///
/// [`Imported`]: crate::claude::store::Imported
#[tauri::command]
pub async fn claude_import_transcripts(
    app: tauri::AppHandle,
) -> Result<crate::claude::store::Imported, String> {
    let db = db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let scan = crate::claude::scan_default()?;
        let mut conn = open_db(&db).map_err(|e| e.to_string())?;
        crate::claude::store::import(&mut conn, scan).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What one pass over both live sources found (#913, epic #910).
///
/// The two halves are returned together because they are ONE answer to
/// "what are my Claude sessions doing", and because separating them would
/// let a UI render a sweep against a stale consumption or vice versa.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ClaudeLiveState {
    /// What the hook's handoff file contributed.
    pub handoff: crate::claude::handoff::Consumed,
    /// What the registry sweep contributed, including crashes found.
    pub sweep: crate::claude::crash::Recorded,
    /// The sessions the registry says are running right now, with the
    /// pid, `procStart`, `cwd` and `name` it carries. Returned as data so
    /// the list view (#917) can show live rows without asking a second
    /// time and getting a different moment's answer.
    pub running: Vec<crate::claude::registry::Live>,
    /// Registry records whose liveness could not be confirmed. NOT folded
    /// into `running`: migration 11's NULL `pid_start_time` means "cannot
    /// confirm", which must read as Unknown rather than Running.
    pub unconfirmed: Vec<crate::claude::registry::Live>,
}

/// Consume the hook's handoff file and sweep the live session registry
/// (#913, epic #910).
///
/// Two sources, one pass, in this order and for this reason: the sweep
/// resolves each pid's `procStart`, and the consumer needs those to fill
/// `pid_start_time`. The hook cannot supply it -- reading its own
/// parent's start time is `sysctl` work inside a 1.5 second budget -- and
/// this side gets it from a file read it is doing anyway.
///
/// # Polling, not a watcher
///
/// Called on a timer. `notify` is not a dependency and adding one would
/// buy the exact failure this epic forbids: a dead FSEvents stream
/// reports "no new sessions" indistinguishably from "the watch died", and
/// on macOS it dies silently. A poll that stops shows up as a log that
/// stops.
///
/// # Absent is not zero
///
/// Every count that could not be read reaches the caller as data --
/// `handoff.unparseable`, `handoff.unknown_version`, `sweep.unreadable`,
/// both `write_failures`. An `Err` means the source could not be read at
/// all, which the UI must render as a failure rather than as "no
/// sessions": the two have opposite remedies and the second is alarming
/// when it is false (#846, where a `= []` default made a REJECTED scan
/// read as "no CLAUDE.md files in this repository").
///
/// # What it writes
///
/// Its own database, plus a truncation of Headstate's OWN handoff file
/// after the records in it are committed. Nothing else under `~/.claude`
/// is written: the registry belongs to Claude Code and one of those files
/// is rewritten by its owner every few seconds.
#[tauri::command]
pub async fn claude_poll_live(app: tauri::AppHandle) -> Result<ClaudeLiveState, String> {
    let db = db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let home = crate::auth::home_dir()
            .ok_or_else(|| "no home directory, so ~/.claude cannot be read".to_string())?;

        // The sweep first: it is what resolves `procStart` into the
        // confirmed start times the consumer stores as `pid_start_time`.
        let swept = crate::claude::registry::sweep(&crate::claude::registry::dir_in(&home))?;
        let start_times = crate::claude::crash::start_times(&swept);

        let mut conn = open_db(&db).map_err(|e| e.to_string())?;
        let sweep = crate::claude::crash::record(&mut conn, &swept)?;

        // A stored offset that cannot be read falls back to zero, which
        // re-reads the file. That is the safe direction: re-reading is a
        // no-op by construction, whereas guessing a non-zero offset would
        // skip records permanently.
        let offset = crate::claude::handoff::Offset(
            crate::store::settings::get::<u64>(&conn, settings::keys::CLAUDE_HANDOFF_OFFSET)
                .unwrap_or(None)
                .unwrap_or(0),
        );
        let handoff = crate::claude::handoff::consume(
            &mut conn,
            &crate::claude::handoff::path_in(&home),
            offset,
            &start_times,
        )?;
        // Persisted AFTER the consume committed. The reverse order would
        // advance the offset past records that never reached the
        // database, and nothing would ever read them again.
        if let Err(e) = crate::store::settings::set(
            &conn,
            settings::keys::CLAUDE_HANDOFF_OFFSET,
            &handoff.offset,
        ) {
            // Not a failure of the pass: the records are stored. The cost
            // is that the next pass re-reads them, which the upserts make
            // a no-op. Said out loud rather than swallowed.
            log::warn!("claude: could not persist the handoff offset: {e}");
        }

        Ok(ClaudeLiveState {
            handoff,
            sweep,
            running: swept.running,
            unconfirmed: swept.unknown,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Aggregates for the Claude Code overview page (#921, epic #910).
///
/// Counts over the sessions already in the cache, plus the set of ids the
/// live registry says are running. It derives NO liveness of its own --
/// see `claude/overview.rs`, which argues why a second derivation on the
/// same page is a defect rather than a convenience.
///
/// # Two failures, kept separate on purpose
///
/// A database that could not be read is an `Err`: there is nothing honest
/// to draw, and a struct of zeros would render 30 chart columns and a
/// "0 resumable" tile that look exactly like a measured quiet month. That
/// is the worst version of the absent-is-not-zero bug because a flat line
/// does not look absent, and #846 is the same defect one view over.
///
/// A live registry that could not be read is NOT an error: every count
/// over stored history is still valid, so the report comes back with
/// `live_failure` set and the page draws the aggregates above a banner
/// saying the running figure cannot be trusted. Refusing the whole page
/// for a 3-file directory would hide 1,461 sessions of real data.
///
/// `spawn_blocking` because it stats one directory per session -- 1,461
/// of them on the development machine, measured at 22-31 ms warm, which
/// is small but is still disk work that does not belong on the async
/// runtime.
#[tauri::command]
pub async fn claude_overview(
    app: tauri::AppHandle,
) -> Result<crate::claude::overview::OverviewReport, String> {
    let db = db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let live = crate::claude::live::running_ids_default();
        let conn = open_db(&db).map_err(|e| e.to_string())?;
        let overview = crate::claude::overview::aggregate(&conn, &live.ids, chrono::Utc::now())
            .map_err(|e| e.to_string())?;
        Ok(crate::claude::overview::OverviewReport {
            overview,
            live_failure: live.failure,
            live_unreadable: live.unreadable,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------
// The Claude Code hook installer (#915). Rust side:
// `claude/install.rs`, which is where every rule below is argued.
// ---------------------------------------------------------------------

/// `~/.claude/settings.json` and this process's binary, or a reason not to.
///
/// The one place the real paths are resolved, so that every function in
/// `claude::install` keeps taking them as parameters -- which is what makes
/// the tests in that module incapable of reaching the developer's own
/// settings file.
fn claude_settings_target() -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let home = crate::auth::home_dir()
        .ok_or_else(|| "no home directory, so there is no ~/.claude to install into".to_string())?;
    let exe = crate::claude::install::current_exe().map_err(|e| e.to_string())?;
    Ok((crate::claude::install::settings_path_in(&home), exe))
}

/// Whether Headstate's hooks are in `~/.claude/settings.json` right now.
///
/// READ every time, never cached (epic #910 §5.5). A cached "installed" is
/// wrong the moment the user hand-edits the file -- and this is a file we
/// invite them to edit, because the malformed refusal tells them to.
///
/// Returns a [`Status`] rather than a bool, and that is the whole point:
/// the third state, `CannotTell`, must not render as "not installed". Claude
/// Code IGNORES a settings file it cannot parse, silently -- measured -- so a
/// user in that state has every hook in the file dead with no symptom, and
/// the remedy is an editor rather than the Install button.
///
/// A `Read` on the remote surface: it reads one file and has no side
/// effects, and "is that desktop recording?" is a reasonable thing to ask
/// from a phone.
///
/// [`Status`]: crate::claude::install::Status
#[tauri::command]
pub fn claude_hooks_status() -> Result<crate::claude::install::Status, String> {
    let (path, exe) = match claude_settings_target() {
        Ok(t) => t,
        // Even THIS is a status rather than an error, for the same reason:
        // a caller that got an `Err` would have to choose a state to show,
        // and the tempting choice is "not installed".
        Err(e) => {
            return Ok(crate::claude::install::Status::CannotTell(
                crate::claude::install::Refusal::Io {
                    path: "~/.claude/settings.json".to_string(),
                    detail: e,
                },
            ))
        }
    };
    Ok(crate::claude::install::status(&path, &exe))
}

/// Install the hooks, appending to whatever is already there.
///
/// `Class::Local` -- the phone cannot do this at all. It edits a config file
/// outside Headstate's ownership, shared with other tools, and the refusal
/// cases need a human reading an explanation at the machine with the broken
/// file. Nobody needs to install a hook from a phone: it only matters for
/// sessions started at that desktop's keyboard.
///
/// This is also the REINSTALL: it drops every matcher it recognises as ours
/// and appends one fresh matcher per event, in one atomic write. So it is
/// idempotent, and it is the repair for a stale path after the app moves.
#[tauri::command]
pub fn claude_install_hooks() -> Result<crate::claude::install::Installed, String> {
    let (path, exe) = claude_settings_target()?;
    crate::claude::install::install(&path, &exe).map_err(|e| e.to_string())
}

/// Reinstall the hooks. Identical to [`claude_install_hooks`] by design.
///
/// A separate command rather than a flag because it is a separate BUTTON
/// with a separate meaning to the user -- "repair this" rather than "set
/// this up" -- and the UI needs to be able to offer one without the other.
/// That they share an implementation is the point of §5.3: one code path
/// means the repair cannot drift from the install.
#[tauri::command]
pub fn claude_reinstall_hooks() -> Result<crate::claude::install::Installed, String> {
    claude_install_hooks()
}

/// Remove Headstate's hooks and nothing else.
///
/// `Class::Local`, like install: the risk is not deletion, it is corrupting
/// another tool's config remotely with no way to see the result. Refusing
/// outright beats gating it behind a biometric step-up.
///
/// Takes no binary path: ownership is decided by the marker and the
/// subcommand, neither of which depends on where the app currently lives.
/// An uninstall keyed on today's path would strand the matcher of an app
/// that has since moved -- which is precisely the hook that most needs
/// removing.
#[tauri::command]
pub fn claude_uninstall_hooks() -> Result<crate::claude::install::Uninstalled, String> {
    let home = crate::auth::home_dir().ok_or_else(|| {
        "no home directory, so there is no ~/.claude to uninstall from".to_string()
    })?;
    crate::claude::install::uninstall(&crate::claude::install::settings_path_in(&home))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// #336: `docker_builds` must actually ENRICH.
    ///
    /// `parse_history` hardcodes `context: None, revision: None`, and
    /// `buildForImage` filters on `b.revision &&`. So a `docker_builds`
    /// that forgets to call `enrich` compiles, passes every other test,
    /// and silently renders no build information at all -- which is
    /// exactly what shipped.
    ///
    /// Asserted on the SOURCE because the behaviour needs a Docker
    /// daemon with build records, which CI has neither of. A source
    /// check is weak, but the alternative here was no check, and this
    /// bug survived precisely because nothing looked.
    #[test]
    fn docker_builds_enriches_rather_than_returning_bare_history() {
        let src = include_str!("commands.rs");
        // The blocking half, not the async wrapper: the enrichment
        // lives there now (#496 moved it off the runtime).
        let start = src
            .find("fn docker_builds_blocking()")
            .expect("docker_builds_blocking not found");
        let body = &src[start..start + 1200];
        assert!(
            body.contains("enrich"),
            "docker_builds must enrich, or context and revision stay null \
             and the build fold never renders"
        );
    }

    /// No Docker command may be a plain `fn`.
    ///
    /// A synchronous `#[tauri::command]` runs on the async runtime's
    /// worker and BLOCKS it, so the whole UI freezes -- not just the
    /// view that asked. Clicking Docker in the menu hung the app for
    /// 5+ seconds because all four of these were sync, and the
    /// heaviest re-runs the full worktree expansion (#496).
    ///
    /// Asserted on the source for the same reason as the enrichment
    /// check above: the behaviour needs a Docker daemon, and this bug
    /// survived precisely because nothing looked.
    #[test]
    fn docker_commands_never_block_the_async_runtime() {
        let src = include_str!("commands.rs");
        for name in [
            "docker_builds",
            "docker_state",
            "docker_images",
            "docker_disk_usage",
        ] {
            assert!(
                src.contains(&format!("pub async fn {name}")),
                "{name} must be `pub async fn` and hand its work to \
                 spawn_blocking; a plain `fn` stalls the runtime and \
                 freezes the entire UI"
            );
            assert!(
                !src.contains(&format!("pub fn {name}(")),
                "{name} still has a blocking signature"
            );
        }
    }

    /// Every stats command must WRITE its answer to `stats_cache`.
    ///
    /// # Why this test exists at the command level
    ///
    /// `store::stats::the_table_is_actually_written_to` is named for the
    /// `merge_history` failure -- a table nothing writes to, implying a
    /// feature that does not exist -- but #840 showed it does not actually
    /// guard against it: it calls `store::stats::put` itself, so it passes
    /// with every production caller deleted. The guard was weaker than its
    /// name. This is the half that asserts the COMMAND path.
    ///
    /// # Why it reads the source rather than calling the commands
    ///
    /// Each of the three needs a `State<GhClient>` holding an
    /// authenticated client and makes live GitHub requests, which CI has
    /// neither a token nor a network budget for. Same trade-off, and the
    /// same precedent, as `docker_builds_enriches_rather_than_returning_
    /// bare_history` above: a source check is weak, but the alternative
    /// here was a guard that could not fail.
    ///
    /// It is still stronger than a whole-file grep in the two ways that
    /// matter: the `put` has to be inside THAT command's body, and the
    /// `Kind` has to be the right one -- so deleting one command's caching
    /// or copy-pasting another's discriminator both fail here.
    #[test]
    fn every_stats_command_writes_its_answer_to_the_cache() {
        let src = include_str!("commands.rs");
        for (name, kind) in [
            ("stats_count", "Kind::Count"),
            ("stats_board", "Kind::Board"),
            ("stats_series", "Kind::Series"),
        ] {
            let start = src
                .find(&format!("pub async fn {name}("))
                .unwrap_or_else(|| panic!("{name} not found"));
            // To the start of the NEXT command, so the window is this
            // function's body and not its neighbour's. Every stats
            // command is followed by another `#[tauri::command]` or by the
            // test module, so the fallback is the end of the file.
            let rest = &src[start + 1..];
            let end = rest
                .find("#[tauri::command]")
                .map(|i| start + 1 + i)
                .unwrap_or(src.len());
            let body = &src[start..end];
            assert!(
                body.contains("store::stats::put("),
                "{name} must cache its answer (#836); without it a closed \
                 window's result is recomputed on every cold start"
            );
            assert!(
                body.contains("store::stats::get("),
                "{name} must READ the cache too -- a write nothing reads \
                 is the `merge_history` shape with extra steps"
            );
            assert!(
                body.contains(kind),
                "{name} must key its rows with {kind}; sharing another \
                 command's discriminator means one answer overwrites the \
                 other, because `put` is INSERT OR REPLACE"
            );
            assert!(
                body.contains("note_stats_viewer("),
                "{name} must note the viewer (#840), or a token swap \
                 leaves the previous user's rows forever"
            );
        }
    }

    /// The identity check must run BEFORE the cache is read.
    ///
    /// Not a style preference -- it is what makes `stats_board`'s key safe.
    /// That key is `board|merged|*|org:X`: a board has no subject, so unlike
    /// `stats_count`'s key it carries NO viewer login. Two accounts on one
    /// machine share this database (`Subject::cache_key` records that as a
    /// real case), so one could otherwise read a board row the other wrote
    /// -- and a `StatsBoard` payload carries the viewer it was split into
    /// Mine and Others against, which would put the reader's own work under
    /// "Others" and show "no activity" for Mine.
    ///
    /// `note_stats_viewer` clears the whole table on an identity change, so
    /// running it first means the only rows the read can find were written
    /// by the account now asking. Reversed, the stale board is served and
    /// THEN the table is cleared -- the wrong answer already returned.
    ///
    /// Checked by source position for the reason the sibling guards above
    /// give: these commands need an authenticated client and live requests.
    /// A position check is weak, but it is the property itself, and the
    /// alternative was nothing.
    #[test]
    fn the_identity_check_precedes_every_cache_read() {
        let src = include_str!("commands.rs");
        for name in ["stats_count", "stats_board", "stats_series"] {
            let start = src.find(&format!("pub async fn {name}(")).unwrap();
            let rest = &src[start + 1..];
            let end = rest
                .find("#[tauri::command]")
                .map(|i| start + 1 + i)
                .unwrap_or(src.len());
            let body = &src[start..end];
            let note = body
                .find("note_stats_viewer(")
                .unwrap_or_else(|| panic!("{name} must call note_stats_viewer"));
            let read = body
                .find("store::stats::get(")
                .unwrap_or_else(|| panic!("{name} must read the cache"));
            assert!(
                note < read,
                "{name} reads the cache before checking whether the token \
                 still belongs to the same person; on a board -- whose key \
                 carries no login -- that serves another account's rows and \
                 splits Mine/Others against the wrong viewer"
            );
        }
    }

    /// A partial answer must reach `put` as partial, in every command.
    ///
    /// #836's acceptance item, asserted where the decision is made. The
    /// dangerous edit is passing a literal `true` -- it compiles, the
    /// numbers look right, and a capped roster or a series missing two
    /// days is cached forever as though it were whole. `store::stats`
    /// cannot catch that: by the time the flag reaches it, it is just a
    /// bool.
    #[test]
    fn no_stats_command_caches_a_partial_answer_as_complete() {
        let src = include_str!("commands.rs");
        for name in ["stats_count", "stats_board", "stats_series"] {
            let start = src.find(&format!("pub async fn {name}(")).unwrap();
            let rest = &src[start + 1..];
            let end = rest
                .find("#[tauri::command]")
                .map(|i| start + 1 + i)
                .unwrap_or(src.len());
            let body = &src[start..end];
            let put = body.find("store::stats::put(").unwrap();
            // The argument list, generously bounded -- the call spans
            // several lines and the flag is the sixth argument.
            let call = &body[put..(put + 600).min(body.len())];
            assert!(
                call.contains("is_complete()") || call.contains(".complete"),
                "{name} must pass the answer's own completeness flag to \
                 `put`, not a literal: a partial result cached as complete \
                 launders a sample into a fact"
            );
            assert!(
                !call.contains("\n                true,"),
                "{name} appears to hardcode `complete: true`"
            );
        }
    }

    /// Every verdict the frontend can name must map, and nothing else
    /// may. A typo in the UI must fail loudly here rather than silently
    /// submitting the wrong verdict on someone else's pull request.
    #[test]
    fn verdict_names_round_trip_and_reject_anything_else() {
        assert_eq!(parse_verdict("approve").unwrap(), ReviewVerdict::Approve);
        assert_eq!(
            parse_verdict("request_changes").unwrap(),
            ReviewVerdict::RequestChanges
        );
        assert_eq!(parse_verdict("comment").unwrap(), ReviewVerdict::Comment);
        assert!(
            parse_verdict("APPROVE").is_err(),
            "casing must not slip through"
        );
        assert!(
            parse_verdict("dismiss").is_err(),
            "dismiss is deliberately unreachable"
        );
        assert!(parse_verdict("").is_err());
    }

    use super::*;

    /// Both commands must accept exactly the same action names. If they
    /// drift, the batch rejects an action the kebab menu offers -- a
    /// failure that only shows up when a user selects rows and acts.
    #[test]
    fn every_offered_action_parses() {
        for name in [
            "merge", "close", "reopen", "draft", "ready", "enqueue", "dequeue",
        ] {
            assert!(parse_action(name).is_ok(), "{name} should parse");
        }
    }

    /// An unknown action names itself in the error, so a typo in the
    /// frontend is diagnosable from the message alone.
    #[test]
    fn an_unknown_action_is_named_in_the_error() {
        let err = parse_action("frobnicate").unwrap_err();
        assert!(err.contains("frobnicate"), "got: {err}");
    }

    /// A batch is issued in chunks, never all at once: GitHub applies
    /// secondary rate limits to concurrent mutations, and the premise of
    /// this feature is that AI-assisted work produces *many* pull
    /// requests, so a forty-PR batch is realistic rather than
    /// pathological. Asserting on the const alone would be vacuous
    /// (clippy says so), so this exercises the chunking the command
    /// actually performs.
    #[test]
    fn a_large_batch_is_issued_in_bounded_chunks() {
        let batch: Vec<u64> = (0..40).collect();
        let chunks: Vec<_> = batch.chunks(BATCH_CONCURRENCY).collect();

        assert!(
            chunks.iter().all(|c| c.len() <= BATCH_CONCURRENCY),
            "no chunk may exceed the concurrency bound"
        );
        assert!(
            chunks.len() > 1,
            "a 40-PR batch must be split, not fired at once"
        );
        // Every pull request is issued exactly once -- a chunking bug
        // that dropped or duplicated one would report the wrong outcomes.
        assert_eq!(chunks.concat(), batch);
    }

    /// The guard against an unbounded query. Its absence is invisible to
    /// every other test in the project.
    #[test]
    fn clamp_days_bounds_the_window() {
        assert_eq!(clamp_days(30), 30, "a normal request passes through");
        assert_eq!(clamp_days(7), 7);
        assert_eq!(clamp_days(90), 90, "the documented maximum is allowed");
        assert_eq!(clamp_days(10_000), 90, "an absurd request is capped");
        assert_eq!(clamp_days(0), 1, "zero would produce an empty query");
        assert_eq!(clamp_days(-5), 1, "negative would loop backwards");
    }

    /// At the cap, the chunked fetch stays to a sane number of concurrent
    /// requests -- the actual reason the clamp exists.
    #[test]
    fn the_cap_bounds_concurrent_chunks() {
        let chunks = clamp_days(10_000) / crate::github::query::HISTORY_CHUNK_DAYS;
        assert!(chunks <= 18, "at most 18 concurrent chunks, got {chunks}");
    }

    #[test]
    fn validate_dirs_accepts_real_directories() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().to_string_lossy().into_owned();
        assert_eq!(validate_dirs(vec![p.clone()]).unwrap(), vec![p]);
    }

    #[test]
    fn validate_dirs_trims_and_drops_blanks() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().to_string_lossy().into_owned();
        let out = validate_dirs(vec![format!("  {p}  "), "".into(), "   ".into()]).unwrap();
        assert_eq!(out, vec![p]);
    }

    /// The point of validating at all: a typo should fail loudly rather
    /// than being stored and later rendering as "no worktrees found".
    #[test]
    fn validate_dirs_rejects_a_path_that_is_not_a_directory() {
        let err = validate_dirs(vec!["/definitely/not/here".into()]).unwrap_err();
        assert!(err.contains("/definitely/not/here"), "{err}");
    }

    /// A file is not a directory, and the error should say so rather than
    /// accepting it and failing during the scan.
    #[test]
    fn validate_dirs_rejects_a_file() {
        let d = tempfile::TempDir::new().unwrap();
        let f = d.path().join("a-file");
        std::fs::write(&f, "x").unwrap();
        assert!(validate_dirs(vec![f.to_string_lossy().into_owned()]).is_err());
    }

    #[test]
    fn auth_error_names_the_command_that_fixes_it() {
        assert!(AUTH_ERR.contains("gh auth login"));
    }

    /// The board's budget projection is an UPPER bound at every offered
    /// window, recomputed here from the fan-out rather than compared against
    /// a hardcoded expectation.
    ///
    /// Under-projecting is the dangerous direction: the check exists to stop
    /// a load STARTING that then runs the budget below `RESERVE` and starves
    /// the poll loop, which is the only part of the app with a standing
    /// obligation. The first arithmetic here was `days / 5 + 8`, copied from
    /// `stats_count`, and it projected 14 for a 30-day window whose worst case
    /// is 18 requests and 26 for a 90-day window whose worst case is 54 --
    /// found by writing exactly this check.
    #[test]
    fn the_board_projection_bounds_its_own_worst_case() {
        let alias_chunk = crate::github::stats::query::ALIAS_CHUNK as u64;
        let detail_chunk = crate::github::stats::board::BOARD_ALIAS_CHUNK as u64;
        // Every window the UI offers, plus the clamp's own bounds -- a Tauri
        // command is a public surface, so the extremes are reachable.
        for days in [1_i64, 7, 14, 30, 90] {
            let slices = u64::try_from(days).unwrap().max(1);
            // The worst case the date grammar allows: one slice per day,
            // because GitHub has no sub-day range.
            let worst =
                slices.div_ceil(alias_chunk) * MAX_PROBE_ROUNDS + slices.div_ceil(detail_chunk);
            let projected = board_projection(days);
            assert!(
                projected >= worst,
                "{days} days: projected {projected} is below the worst case {worst}; \
                 a load could start and then starve the poll loop"
            );
        }
        // And it is not absurdly loose either: a projection so large that it
        // refuses ordinary loads is a feature nobody can use. The MEASURED
        // figure on a real 30-day org window was 9 points.
        assert!(
            board_projection(30) < 40,
            "a 30-day window measured 9 points; a projection this high would \
             refuse real loads"
        );
        // Clamped at both ends, so a hostile value cannot project to zero and
        // bypass the check.
        assert!(board_projection(0) > 0);
        assert!(board_projection(-5) > 0);
    }

    /// The window and the day list describe exactly the same period.
    ///
    /// `stats_board` measures the WINDOW and `stats_series` measures the DAYS,
    /// and the two render on one page. A board covering 30 days beside a chart
    /// covering 31 would be a page whose own numbers contradict each other
    /// with nothing on screen to explain it -- and the off-by-one that does it
    /// is invisible in review, because both halves look right alone.
    ///
    /// Asserted as three properties rather than against a fixed date, so the
    /// test does not rot and does not depend on when it runs.
    #[test]
    fn the_window_and_the_day_list_cover_the_same_period() {
        let now = chrono::Utc::now();
        for days in [1_i64, 7, 14, 30, 90] {
            let r = parse_scope_request("org", Some("acme".into()), days, now).expect("parses");
            assert_eq!(
                r.days.len(),
                usize::try_from(days).unwrap(),
                "{days} days: one entry per day"
            );
            assert_eq!(
                r.days.first().unwrap(),
                &r.window.from,
                "{days} days: starts together"
            );
            assert_eq!(
                r.days.last().unwrap(),
                &r.window.to,
                "{days} days: ends together"
            );
            // Contiguous with no gap and no duplicate: a gap would drop a
            // column from the chart and a duplicate would draw one twice.
            for pair in r.days.windows(2) {
                let a = chrono::NaiveDate::parse_from_str(&pair[0], "%Y-%m-%d").unwrap();
                let b = chrono::NaiveDate::parse_from_str(&pair[1], "%Y-%m-%d").unwrap();
                assert_eq!((b - a).num_days(), 1, "days must be consecutive: {pair:?}");
            }
        }
    }

    /// The window ends YESTERDAY, not today.
    ///
    /// `query::period_ranges` (`query.rs:247-252`) and `stats_count` both do
    /// this, and the reason is the same: today is still accumulating, so
    /// including it compares a partial day against complete ones. It is also
    /// what makes a closed window's answer cacheable at all.
    #[test]
    fn the_window_excludes_today() {
        let now = chrono::Utc::now();
        let r = parse_scope_request("all", None, 7, now).expect("parses");
        let today = now.format("%Y-%m-%d").to_string();
        assert!(
            r.window.to < today,
            "window ends {} but today is {today}; a partial day would drag \
             every figure down",
            r.window.to
        );
        assert!(!r.days.contains(&today));
    }

    /// An unbounded `days` is clamped, like `get_history`'s.
    ///
    /// A Tauri command is a public surface. Here the blast radius is worse
    /// than one long query: the planner probes, subdivides and probes again,
    /// so a 100-year window would build an enormous plan before anything
    /// refused it.
    #[test]
    fn the_window_is_clamped_like_every_other_public_surface() {
        let now = chrono::Utc::now();
        let huge = parse_scope_request("all", None, 100_000, now).expect("parses");
        assert_eq!(
            huge.days.len(),
            usize::try_from(clamp_days(100_000)).unwrap()
        );
        // And a zero or negative value does not produce an empty or
        // backwards window, which would make every search a no-op that
        // returned a confident zero.
        for bad in [0_i64, -1, i64::MIN] {
            let r = parse_scope_request("all", None, bad, now).expect("parses");
            assert!(!r.days.is_empty(), "{bad} produced an empty window");
            assert!(
                r.window.from <= r.window.to,
                "{bad} produced a backwards window"
            );
        }
    }

    /// A scope kind that needs a value and has none is an ERROR, not a
    /// silently widened question.
    #[test]
    fn a_scope_without_its_value_is_refused() {
        let now = chrono::Utc::now();
        for kind in ["repo", "org", "user"] {
            assert!(
                parse_scope_request(kind, None, 30, now).is_err(),
                "{kind} with no value must be refused, not widened"
            );
        }
        // `all` is the one kind whose value is genuinely absent.
        assert!(parse_scope_request("all", None, 30, now).is_ok());
        assert!(parse_scope_request("nonsense", Some("x".into()), 30, now).is_err());
    }

    /// The probe-round bound is a DECISION, not `MAX_DEPTH`.
    ///
    /// `slice::MAX_DEPTH` is 24 and is the recursion guard; projecting against
    /// it would multiply the bound sixfold and refuse almost every load. The
    /// measured figure on a real 30-day window was 3 rounds.
    #[test]
    fn the_probe_round_bound_is_not_the_recursion_guard() {
        const {
            assert!(
                MAX_PROBE_ROUNDS >= 3,
                "a real 30-day window measured 3 rounds"
            );
            assert!(
                MAX_PROBE_ROUNDS < crate::github::stats::slice::MAX_DEPTH as u64,
                "MAX_DEPTH is the recursion guard, not a realistic round count"
            );
        }
    }
}

/// The machine's current state.
///
/// Cheap -- single-digit milliseconds -- but it reads the kernel, so it
/// goes to a blocking worker like every other read here.
///
/// The `Collector` is managed state rather than built per call: sysinfo
/// reports CPU use SINCE THE LAST REFRESH, so a fresh instance every
/// time would report an idle machine forever (`health::collect`).
#[tauri::command]
pub async fn system_health(
    collector: State<'_, std::sync::Arc<crate::health::collect::Collector>>,
) -> Result<crate::health::Sample, String> {
    let collector = collector.inner().clone();
    tauri::async_runtime::spawn_blocking(move || collector.sample(&chrono::Utc::now().to_rfc3339()))
        .await
        .map_err(|e| e.to_string())
}

/// The last 24 hours, downsampled.
///
/// Bounded by `store::health::MAX_POINTS` inside the query, so a caller
/// cannot ask for the raw series however long the app has been running.
/// The phone reads this over the LAN, where an unbounded payload is the
/// mistake that made `size_worktrees` time out (#661).
#[tauri::command]
pub async fn system_health_history(app: AppHandle) -> Result<Vec<crate::health::Sample>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
        crate::store::health::history(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every health condition currently true of this machine (#789).
///
/// The rules run HERE and nowhere else. A caller that wanted to notify
/// about this machine's health -- the paired phone does -- would
/// otherwise have to reimplement every threshold in `health::alerts` and
/// `health::runaway`, in a separate crate, and the two copies would
/// drift silently. A drifted copy of an interrupt-the-user rule is worse
/// than no rule: it keeps passing its own tests while describing
/// behaviour the app no longer has.
///
/// So this returns verdicts, not data: `health::AlertReport` carries the
/// condition key and the desktop's own wording. The caller adds only
/// what the desktop cannot know -- whose machine it is -- and
/// deduplicates on the key.
///
/// # Why there is no `Fired` here
///
/// This reports what IS true, not what is NEW, exactly as
/// `health::alerts::evaluate` does and for the same reason: dedup state
/// belongs to whoever is doing the notifying. The desktop's sampler has
/// its own `Fired`; the phone has its own; and a command that returned
/// only transitions would make the answer depend on who asked last,
/// which would mean two clients each seeing half the alerts.
///
/// Reads the stored series, like `system_health_history`, so both the
/// charts and the rules see the same gap-preserving, downsampled data.
/// The process table is read for the aggregate CPU rule's "no single
/// process explains it" clause -- see `health::runaway`.
#[tauri::command]
pub async fn health_alerts(app: AppHandle) -> Result<Vec<crate::health::AlertReport>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db_path(&app)).map_err(|e| e.to_string())?;
        let history = crate::store::health::history(&conn).map_err(|e| e.to_string())?;
        let threshold = crate::health::alerts::low_percent(read_ui_prefs(&app).battery_low_percent);

        let mut out: Vec<crate::health::AlertReport> =
            crate::health::alerts::evaluate(&history, threshold)
                .into_iter()
                .map(|a| crate::health::AlertReport {
                    key: a.key().to_string(),
                    title: a.title(),
                    body: a.body(),
                })
                .collect();

        // A fresh `Table` per call rather than managed state, unlike
        // `Collector` and `Footprints`. Those are live readings on a
        // timer, where a held instance is what makes a CPU delta mean
        // anything; this is an occasional question from a phone, so the
        // two refreshes it needs are done here and the instance is
        // dropped.
        //
        // `read_twice`, never two `read` calls: `sysinfo` will not
        // recompute a CPU delta inside `MINIMUM_CPU_UPDATE_INTERVAL`, so
        // back-to-back reads report every process at 0% -- and a zero
        // `top_cpu_percent` makes the aggregate rule conclude that
        // nothing explains the load, firing on a legitimate build. That
        // wait is why `read_twice` exists and why it lives in
        // `health::runaway` rather than here.
        let table = crate::health::runaway::Table::new();
        let (_, aggregate) = table.read_twice();
        out.extend(
            crate::health::runaway::evaluate(&history, Some(&aggregate))
                .into_iter()
                .map(|a| crate::health::AlertReport {
                    key: a.key().to_string(),
                    title: a.title(),
                    body: a.body(),
                }),
        );

        // #872's oversubscription notice. Computed HERE rather than in
        // the poll loop, unlike the watch notices below: this rule is
        // pure over the stored series -- `runaway::oversubscribed` reads
        // `Sample::load`, which every row already carries -- so it needs
        // no accumulator and nothing to remember between calls. The
        // series is the one the charts draw, which is the same property
        // `runaway::evaluate` above relies on.
        //
        // `aggregate.cores` is 0 when `available_parallelism` failed, and
        // `oversubscribed` reads that as "cannot say" and returns `None`
        // rather than dividing -- a division would be an infinity that
        // cleared the threshold on every machine whose core count could
        // not be read. Absent is not zero.
        out.extend(
            crate::health::runaway::oversubscribed(&history, aggregate.cores)
                .into_iter()
                .map(|n| crate::health::AlertReport {
                    key: n.key(),
                    title: n.title(),
                    body: n.body(),
                }),
        );

        // #865's watch notices: processes holding a moderate amount of
        // CPU for long enough to be worth a human glance. Read from the
        // poll loop's shared result rather than recomputed, because the
        // duration half of the rule lives in its long-running `Watcher`
        // -- a fresh `Table` here would see every process at 0.0 minutes
        // and surface nothing. See `runaway::Watched`.
        //
        // Appended after the alerts, so anything that would interrupt
        // the user sorts above anything that merely wants a look.
        // `get` returns None both before the first pass and once the newest
        // pass is older than `NOTICE_MAX_AGE` (#908), so a dead poll loop
        // serves nothing rather than its last answer. The age is folded into
        // each body, because a claim about the present should carry the age
        // of its evidence -- the same honesty #788 put on the worktree row's
        // "up to date with upstream".
        if let Some((age, notices)) = app
            .state::<std::sync::Arc<crate::health::runaway::Watched>>()
            .get()
        {
            out.extend(notices.into_iter().map(|n| crate::health::AlertReport {
                key: n.key(),
                title: n.title(),
                body: crate::health::runaway::with_age(&n.body(), age),
            }));
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What Headstate itself is costing, right now (#665).
/// What is using this machine, right now (#687, #721).
///
/// Feeds the System Health CPU and Memory detail pages, and nothing
/// else: the machine's top processes by CPU and by resident set, the
/// same two summed by process name, and how many processes were running
/// in total so each page can say what it is NOT showing.
///
/// A kernel read of the already-open process table -- no subprocess, no
/// directory walk -- so it is safe to call as often as those pages
/// refresh, and `health::footprint` carries a test asserting it stays
/// that way.
///
/// # The name is historical, and deliberately not fixed
///
/// This was #665's "What Headstate is costing" panel: our own process,
/// the `git`/`gh`/Docker subprocesses we spawn, and the Docker daemon.
/// #795 removed that panel and those three fields -- a once-a-second
/// sample could not catch the bursty `git` fan-out that is our real
/// cost, so it reported us as cheap, confidently and wrongly.
///
/// The command kept the name. Renaming it would mean changing a literal
/// string in two remote-surface allowlists (`remote::surface` here and
/// `src-mobile/src/surface.rs`), which a phone build pinned to an older
/// desktop cannot follow -- a wire break for a word. So the misnomer
/// stays and this paragraph is the fix.
///
/// # No disk sizing here, still
///
/// Worktree, artifact, venv and Docker sizes come from `size_worktrees`,
/// `size_artifacts`, `size_venvs` and `docker_disk_usage`, which the
/// Worktrees, Artifacts and Docker views own. There is no combined
/// command on purpose: those four take seconds to tens of seconds
/// (`size_worktrees` was the #661 timeout) and must never share a call
/// site with something this cheap. #796 removed the one view that
/// summed them into a "what of this is ours" figure, so nothing calls
/// them together any more -- which makes the rule easier to keep, not
/// less necessary.
#[tauri::command]
pub async fn system_footprint(
    footprints: State<'_, std::sync::Arc<crate::health::footprint::Footprints>>,
) -> Result<crate::health::Footprint, String> {
    let footprints = footprints.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        footprints.sample(&chrono::Utc::now().to_rfc3339())
    })
    .await
    .map_err(|e| e.to_string())
}

/// Which processes are using the network, right now (#718).
///
/// # This blocks for about FIVE SECONDS on macOS, by construction
///
/// `nettop` samples for a full interval before it prints, and `-L 1`
/// waits that interval out -- measured at 5.06-5.25 s across every
/// flag combination that might have shortened it, against 0.08 s of
/// CPU. It is a sleep, not work, and there is no faster route to
/// per-process attribution without elevation. `health::netproc` carries
/// the full table and the reasoning.
///
/// # So this is NOT `system_health`, and must never be called beside it
///
/// The live view polls `system_health` every five seconds
/// (`HEALTH_POLL_MS`). A 5.1-second subprocess on a 5-second timer
/// means each call outlives the interval that spawned it: `nettop`
/// processes would overlap continuously for as long as the app was
/// open. That is #661's failure -- a slow command on a shared timer --
/// so this is a SEPARATE command driven by the Network detail page's own
/// slower cadence, and it exists separately from `system_health`
/// precisely so it cannot be folded into that sample by accident.
///
/// Returns an empty list on every platform but macOS, which is the
/// honest answer rather than a zero: no unprivileged per-process
/// attribution exists on Linux, and Windows' is real unwritten work.
#[tauri::command]
pub async fn system_network_processes() -> Result<Vec<crate::health::NetProcess>, String> {
    tauri::async_runtime::spawn_blocking(crate::health::netproc::read)
        .await
        .map_err(|e| e.to_string())
}

/// The event name a branch scan reports its progress under.
///
/// One name, two frame shapes, because it is one stream: a `listed`
/// frame then `classified` frames, and a consumer that saw only the
/// second kind could not know how many to expect. On the allowlists in
/// `remote/events.rs` and `src-mobile/src/events.rs`, so the phone
/// receives it too.
pub const BRANCH_SCAN_PROGRESS: &str = "branch-scan-progress";

/// One frame of a branch scan.
///
/// `repo` is on EVERY frame, and load-bearing rather than
/// informational: the events are app-global while the scan is
/// per-repository, so a page that changed repository mid-scan would
/// otherwise fold the old repository's verdicts into the new
/// repository's rows.
///
/// The paths are NOT in the payload beyond the repository the caller
/// already named, matching the rule `worktree-removal-progress`
/// follows -- a progress event is not a place to leak what the user is
/// working on. Branch names are here because they are the join key,
/// and the page is already showing them.
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BranchScanFrame {
    /// Every branch, metadata only, all verdicts `Pending`. Sent once,
    /// before any classification, and it carries the total: that is
    /// what lets the page say "47 of 512" and so makes a stream that
    /// died at 47 visibly incomplete rather than merely finished-looking.
    #[serde(rename_all = "camelCase")]
    Listed {
        repo: String,
        total: usize,
        branches: Vec<crate::branches::Branch>,
    },
    /// A batch of settled verdicts, by branch name.
    #[serde(rename_all = "camelCase")]
    Classified {
        repo: String,
        verdicts: Vec<(String, crate::branches::Deletable)>,
    },
}

/// Emits [`BranchScanFrame`]s as the scan produces them.
struct BranchScanEmitter {
    app: AppHandle,
    repo: String,
}

impl crate::branches::Progress for BranchScanEmitter {
    fn listed(&self, branches: &[crate::branches::Branch]) {
        let _ = self.app.emit(
            BRANCH_SCAN_PROGRESS,
            BranchScanFrame::Listed {
                repo: self.repo.clone(),
                total: branches.len(),
                branches: branches.to_vec(),
            },
        );
    }

    fn classified(&self, verdicts: &[(String, crate::branches::Deletable)]) {
        // Called from all eight classification threads. `emit` takes
        // `&self` and Tauri's handle is `Sync`, so no lock is needed
        // here -- and adding one would serialise the workers behind
        // the reporting, which is the opposite of the point.
        let _ = self.app.emit(
            BRANCH_SCAN_PROGRESS,
            BranchScanFrame::Classified {
                repo: self.repo.clone(),
                verdicts: verdicts.to_vec(),
            },
        );
    }
}

/// Every branch in a repository, classified.
///
/// Blocking git work -- measured at ~9s on a 675-branch repository --
/// so it goes to a blocking thread rather than an async worker.
///
/// `scan_cached`, not `scan`: this is the read-only listing, and the
/// page refetches on a deliberately short `staleTime`, so an unchanged
/// repository was paying the full scan every ten seconds. The cache is
/// keyed on the ref state, so it returns only when nothing that could
/// change an answer has moved (#657). Deletion still calls `scan`
/// directly and is unaffected.
///
/// # Why it also streams
///
/// The cache fixed the REPEAT visit and structurally cannot fix the
/// cold one -- there is nothing to serve. So this reports what it
/// finds as it finds it: one `listed` frame with every row, then
/// verdicts as the threads settle them (#657). The return value is
/// unchanged and remains the authority; the frames are an early view
/// of the same work, not a second source of truth.
#[tauri::command]
pub async fn list_branches(
    app: AppHandle,
    repo_path: String,
) -> Result<Vec<crate::branches::Branch>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = BranchScanEmitter {
            app,
            repo: repo_path.clone(),
        };
        crate::branches::scan_cached_with_progress(std::path::Path::new(&repo_path), &emitter)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The event name a branch DELETION reports its progress under.
///
/// Separate from [`BRANCH_SCAN_PROGRESS`] even though the deletion's
/// first phase is a scan. They describe different operations to the
/// user -- one fills a list in, the other is destroying refs -- and a
/// page that folded them together would show a deletion's re-check as
/// the listing reclassifying itself. On the allowlists in
/// `remote/events.rs` and `src-mobile/src/events.rs`, so the phone
/// receives it too.
pub const BRANCH_DELETE_PROGRESS: &str = "branch-delete-progress";

/// One frame of a running branch deletion.
///
/// Two shapes because a deletion has two phases with genuinely
/// different meanings, and reporting them as one counter is the bug
/// (#724): the safety re-check is a full uncached scan at ~64ms per
/// branch, so on the reported 562-branch batch a single counter sat at
/// 0/562 for minutes before the first ref came off. `Checking` names
/// that wait; `Deleting` counts what is actually being destroyed.
///
/// `repo` is on every frame for the reason the scan's frames carry it:
/// the events are app-global while the work is per-repository.
///
/// No paths beyond the repository the caller itself named -- the rule
/// `worktree-removal-progress` follows. Branch names are absent from
/// the payload entirely: unlike the scan, which is filling a list of
/// them in, nothing here needs a join key. Counts are enough.
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BranchDeleteFrame {
    /// The safety re-check has classified `done` of `total` branches.
    ///
    /// `total` is every branch in the repository, not the batch: the
    /// gate scans the whole repository once. Nothing has been deleted
    /// while these arrive, which is precisely what the phase label has
    /// to convey.
    #[serde(rename_all = "camelCase")]
    Checking {
        repo: String,
        done: usize,
        total: usize,
    },
    /// `done` of `total` selected branches attempted, `failed` refused.
    ///
    /// `failed` rides along on every frame rather than waiting for the
    /// summary: a batch losing thirty branches to refusals is
    /// something the user wants while the run is still going.
    #[serde(rename_all = "camelCase")]
    Deleting {
        repo: String,
        done: usize,
        total: usize,
        failed: usize,
    },
}

/// Emits [`BranchDeleteFrame`]s as a deletion proceeds.
struct BranchDeleteEmitter {
    app: AppHandle,
    repo: String,
}

impl crate::branches::DeleteProgress for BranchDeleteEmitter {
    fn checking(&self, done: usize, total: usize) {
        // Called from the scan's eight classification threads. `emit`
        // takes `&self` and Tauri's handle is `Sync`, so no lock --
        // and one here would serialise the workers behind the
        // reporting.
        let _ = self.app.emit(
            BRANCH_DELETE_PROGRESS,
            BranchDeleteFrame::Checking {
                repo: self.repo.clone(),
                done,
                total,
            },
        );
    }

    fn deleted(&self, done: usize, total: usize, failed: usize) {
        let _ = self.app.emit(
            BRANCH_DELETE_PROGRESS,
            BranchDeleteFrame::Deleting {
                repo: self.repo.clone(),
                done,
                total,
                failed,
            },
        );
    }
}

/// Delete local branches, re-checking each one at delete time.
///
/// # Why it reports progress
///
/// It ran for over ten minutes on a 562-branch selection with nothing
/// on screen, and the user could not tell a slow batch from a hung one
/// (#724). The re-check is the slow half and it happens BEFORE any
/// deletion, so the two are reported as separate phases: a counter
/// that sits at 0 through the longest part of the wait is the failure
/// being fixed, not the fix.
///
/// The gate itself does not move. This still re-checks against a fresh
/// uncached scan; the frames observe that scan, they do not replace or
/// shorten it.
#[tauri::command]
pub async fn delete_branches(
    app: AppHandle,
    repo_path: String,
    names: Vec<String>,
) -> Result<Vec<crate::branches::DeleteOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = BranchDeleteEmitter {
            app,
            repo: repo_path.clone(),
        };
        crate::branches::delete_local_with_progress(&repo_path, &names, &emitter)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Delete branches ON THE REMOTE.
///
/// A separate command from `delete_branches` on purpose: this pushes to
/// shared state, and there is no reflog on the other side to recover a
/// mistake from. Keeping it distinct means the UI cannot reach it by
/// the same control.
///
/// Reports the same two phases (#724), and the second phase matters
/// more here: every deletion is a network round trip.
#[tauri::command]
pub async fn delete_remote_branches(
    app: AppHandle,
    repo_path: String,
    names: Vec<String>,
) -> Result<Vec<crate::branches::DeleteOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = BranchDeleteEmitter {
            app,
            repo: repo_path.clone(),
        };
        crate::branches::delete_remote_with_progress(&repo_path, &names, &emitter)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Apply updates, open a pull request, and report when it is up.
///
/// Returns IMMEDIATELY. The wizard used to await the whole run with the
/// modal open on one unchanging "Applying…" label -- and the run is a
/// package-manager command per package, so on a repository with 122
/// selected it sat there for minutes with the app unusable (#495).
///
/// Progress and completion arrive as events, the same shape the
/// worktree removal uses: `update-run-progress` with (done, total)
/// after each package, then `update-run-done`. The work runs to
/// completion regardless of what is on screen, so navigating away does
/// not cancel it.
///
/// The progress half was missing until #626 -- this comment described
/// it, `run_on_branch` took no callback, and only the terminal event
/// ever fired. A phone feels that hardest: it has no window to leave
/// open and watch.
#[tauri::command]
pub async fn apply_updates_in_background(
    app: AppHandle,
    client: State<'_, GhClient>,
    repo_path: String,
    requests: Vec<crate::packages::apply::UpdateRequest>,
    branch: Option<String>,
) -> Result<(), String> {
    // Checked HERE as well as inside the run: this command returns
    // immediately, so a bad name would otherwise be reported only by a
    // notification minutes later, long after the moment the user could
    // connect it to what they typed.
    if let Some(b) = branch.as_deref() {
        crate::packages::apply::valid_branch_name(b)?;
    }
    // Claimed BEFORE anything is spawned, and the claim is what refuses
    // a second run: two package managers in one worktree is not a thing
    // to discover afterwards. It also returns the flag the run reads to
    // stop, so the registry owns both halves.
    let stop = app
        .state::<crate::packages::runs::UpdateRuns>()
        .start(&repo_path, requests.len())?;

    // Cloned OUT of `State` before spawning: the guard borrows the
    // app handle and cannot outlive this function, but the task must.
    let gh = GhClient(client.0.clone());
    tauri::async_runtime::spawn(async move {
        let repo = repo_path.clone();
        let reqs = requests.clone();
        let progress_app = app.clone();
        let progress_repo = repo_path.clone();
        let stop_flag = stop.clone();
        let applied = tauri::async_runtime::spawn_blocking(move || {
            crate::packages::apply::run_on_branch_cancellable(
                std::path::Path::new(&repo),
                &reqs,
                branch.as_deref(),
                |done, total| {
                    // Counts only -- never package names. Same rule as
                    // the worktree removal's progress beside it.
                    let _ = progress_app.emit("update-run-progress", (done, total));
                    // And into the registry, so a client that was
                    // asleep for the whole run can still ask.
                    progress_app
                        .state::<crate::packages::runs::UpdateRuns>()
                        .progress(&progress_repo, done, total);
                },
                move || stop_flag.load(std::sync::atomic::Ordering::SeqCst),
            )
        })
        .await;
        let was_cancelled = stop.load(std::sync::atomic::Ordering::SeqCst);

        let report = match applied {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                finish(&app, UpdateRunDone::failed(&repo_path, e));
                return;
            }
            Err(e) => {
                finish(&app, UpdateRunDone::failed(&repo_path, e.to_string()));
                return;
            }
        };

        // Stopped by the user. Not a failure: the packages that landed
        // before the stop really did land, and the worktree holding
        // them still exists -- so this reports what happened rather
        // than opening a pull request nobody asked to finish.
        if was_cancelled {
            let mut done = UpdateRunDone::worktree_only(&repo_path, &report, "cancelled");
            done.cancelled = true;
            finish(&app, done);
            return;
        }

        // Every package failed: there is nothing to open a pull request
        // about, and saying one is coming would be a lie.
        if report.results.iter().all(|r| r.error.is_some()) {
            finish(
                &app,
                UpdateRunDone::worktree_only(&repo_path, &report, "no update applied"),
            );
            return;
        }

        match crate::commands::open_update_pr_inner(&gh, &repo_path, report.clone()).await {
            Ok(url) => finish(&app, UpdateRunDone::opened(&repo_path, &report, url)),
            // The worktree still exists and the updates are still in it;
            // only the pull request did not happen. Say exactly that.
            Err(e) => finish(&app, UpdateRunDone::worktree_only(&repo_path, &report, &e)),
        }
    });
    Ok(())
}

/// Ask a background update run to stop.
///
/// It stops after the package it is on, never during one: a package
/// manager killed halfway leaves a worktree in a state nobody asked
/// for. So this returns immediately and the run ends a moment later,
/// reporting what it managed to apply.
///
/// Errors when nothing is running in that repository, rather than
/// succeeding quietly -- a Cancel that appears to work on a run that
/// already finished is its own small lie.
#[tauri::command]
pub fn cancel_update_run(
    runs: State<'_, crate::packages::runs::UpdateRuns>,
    repo_path: String,
) -> Result<(), String> {
    runs.cancel(&repo_path)
}

/// How a repository's background update run is going, or how it ended.
///
/// The read a client uses when it was not listening. Progress and
/// completion are events, and a suspended phone holds no event stream
/// (`src-mobile/src/background.rs`), so one that started a run and went
/// to sleep missed every frame including the terminal one. Without this
/// it could start a run and then genuinely never learn how it ended.
///
/// `None` when this process has never run one for that repository.
#[tauri::command]
pub fn update_run_state(
    runs: State<'_, crate::packages::runs::UpdateRuns>,
    repo_path: String,
) -> Option<crate::packages::runs::RunState> {
    runs.state(&repo_path)
}

/// What a background update run produced.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRunDone {
    pub repo_path: String,
    /// The pull request, when one was opened.
    pub url: Option<String>,
    pub branch: Option<String>,
    pub applied: usize,
    pub failed: usize,
    /// Whether the user stopped it. Distinct from `error`: a cancelled
    /// run did not fail, it was asked to stop, and the packages that
    /// landed before it did really did land.
    pub cancelled: bool,
    /// Why no pull request, when there is none. Never a claim that one
    /// exists.
    pub error: Option<String>,
}

impl UpdateRunDone {
    fn opened(repo: &str, r: &crate::packages::apply::RunReport, url: String) -> Self {
        Self {
            repo_path: repo.to_string(),
            url: Some(url),
            branch: Some(r.branch.clone()),
            applied: r.results.iter().filter(|x| x.error.is_none()).count(),
            failed: r.results.iter().filter(|x| x.error.is_some()).count(),
            cancelled: false,
            error: None,
        }
    }
    fn worktree_only(repo: &str, r: &crate::packages::apply::RunReport, why: &str) -> Self {
        Self {
            repo_path: repo.to_string(),
            url: None,
            branch: Some(r.branch.clone()),
            applied: r.results.iter().filter(|x| x.error.is_none()).count(),
            failed: r.results.iter().filter(|x| x.error.is_some()).count(),
            cancelled: false,
            error: Some(why.to_string()),
        }
    }
    fn failed(repo: &str, why: String) -> Self {
        Self {
            repo_path: repo.to_string(),
            url: None,
            branch: None,
            applied: 0,
            failed: 0,
            cancelled: false,
            error: Some(why),
        }
    }
}

/// Emit the outcome and, when a pull request went up, notify.
fn finish(app: &AppHandle, done: UpdateRunDone) {
    use tauri_plugin_notification::NotificationExt;

    // Recorded BEFORE the event. Every exit from the run goes through
    // here -- the three error arms, the all-failed case, and success --
    // which makes this the one place that cannot be forgotten, and the
    // reason the registry entry is always consistent with what was
    // emitted.
    //
    // It is also what a client that missed the event reads later:
    // `update_run_state` returns exactly this.
    app.state::<crate::packages::runs::UpdateRuns>()
        .finished(&done.repo_path, done.clone());

    if let Err(e) = app.emit("update-run-done", &done) {
        log::warn!("could not emit update-run-done: {e}");
    }

    // ONLY for a pull request that actually exists. A run that stopped
    // at the worktree still did useful work, but interrupting the user
    // to say "ready" about something that is not there is worse than
    // staying quiet -- the toast carries that case.
    let Some(url) = done.url.as_deref() else {
        return;
    };
    if !crate::poll::notification_allowed(app) {
        return;
    }
    let body = match done.failed {
        0 => format!("{} package(s) updated", done.applied),
        n => format!("{} updated, {n} could not be", done.applied),
    };
    if let Err(e) = app
        .notification()
        .builder()
        .title("Package update pull request is ready")
        .body(body)
        .show()
    {
        log::warn!("failed to show notification: {e}");
    }
    log::info!("update run opened {url}");
}
