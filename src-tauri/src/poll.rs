//! Background polling.
//!
//! Polling lives in Rust rather than React so it continues while the window
//! is hidden to the tray -- which is what makes the tray badge meaningful.
//! React never talks to GitHub directly: it renders whatever snapshot is on
//! disk and listens for the `prs-updated` event.

use crate::github::client::{ClientError, GitHubClient};
use crate::github::model::{needs_attention_count, CiState, MergeState, PullRequest, ReviewState};
use crate::store::{open_db, save_snapshot, CachedList};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

/// Default focused cadence, in seconds.
///
/// Two minutes rather than one: PR state rarely changes minute-to-minute,
/// and a tick costs 4 rate-limit points, so halving the rate halves the
/// spend for no practical loss of freshness.
///
/// At 120s that is 30 ticks/hour x 4 = **120 points/hour** of 5,000.
///
/// The cost figure is per TICK, which is two searches -- see
/// `MIN_FOCUSED_SECS` for the measurement and for what the "6 points"
/// these comments used to quote actually described.
pub const DEFAULT_FOCUSED_SECS: u64 = 120;

/// Floor on the configured interval.
///
/// # What a tick costs, measured
///
/// 4 rate-limit points, not the 6 these comments claimed (#842, #844).
/// Both numbers were wrong in both directions and for the same reason: 6
/// was measured on a document carrying TWO search aliases, which
/// `PRS_QUERY`'s own doc records was split into one search per request --
/// and nothing re-measured afterwards. Meanwhile the tick grew a SECOND
/// request, so reasoning from one fetch understated it by half.
///
/// MEASURED live 2026-09-11, `gh api graphql -F first=25`, the document
/// extracted verbatim from `PRS_QUERY` with its `#` comment lines
/// stripped, 3 runs per row:
///
/// | Search (`poll.rs` line)                     | Cost | Wall clock  |
/// |---------------------------------------------|------|-------------|
/// | `is:pr is:open author:@me` (`:835`)         | 2    | 2.31-2.56s  |
/// | `is:pr is:open review-requested:@me` (`:851`)| 2   | 0.84-0.99s  |
///
/// = **4 points per tick** when the ready-to-review notification is on,
/// which is the default. The second search is skipped when it is off, so
/// 2 is the floor and 4 is what to budget for.
///
/// # Why 60s and not 30
///
/// At 60s that is 60 ticks/hour x 4 = **240 points/hour**; at 30s it
/// would be 480. Both survive a 5,000/hour budget, but the app should not
/// be able to consume a tenth of the user's own `gh` allowance on a
/// setting they picked without knowing the cost.
///
/// `both_cadences_stay_well_inside_the_rate_limit` asserts against THIS
/// value rather than the default, so a user choosing the fastest allowed
/// setting still cannot blow through the guard -- and it now reasons from
/// the measured 4 rather than from a count of connection names.
pub const MIN_FOCUSED_SECS: u64 = 60;
pub const MAX_FOCUSED_SECS: u64 = 3600;

/// Backgrounded polling is 5x the focused interval: the window is hidden,
/// so freshness matters less, but the tray badge must not go stale for
/// long. Proportional rather than separately configurable -- one knob is
/// enough, and two invites inconsistent pairs.
pub const BACKGROUND_MULTIPLIER: u64 = 5;

pub const FOCUSED: Duration = Duration::from_secs(DEFAULT_FOCUSED_SECS);
pub const BACKGROUND: Duration = Duration::from_secs(DEFAULT_FOCUSED_SECS * BACKGROUND_MULTIPLIER);

/// Clamp a user-supplied interval into the allowed range.
///
/// Extracted so it is testable: a Tauri command is a public surface, and
/// an unbounded value here would either hammer GitHub or effectively stop
/// polling.
pub fn clamp_interval(secs: u64) -> u64 {
    secs.clamp(MIN_FOCUSED_SECS, MAX_FOCUSED_SECS)
}

/// #22: how long after a poll to fire the one-shot targeted re-poll for
/// PRs still stuck on `MergeState::Checking`. GitHub computes mergeability
/// lazily and often hasn't finished 5s after a push; this is far shorter
/// than either regular cadence so a fresh push resolves quickly without
/// waiting a full tick.
pub const RECHECK_DELAY: Duration = Duration::from_secs(5);

/// The cadence for the current window state, given a configured interval.
///
/// A tick costs 4 rate-limit points -- TWO sequential searches at a
/// measured 2 each, see `MIN_FOCUSED_SECS` for the table -- so the default
/// 120s focused cadence spends **120 points/hour** against a 5,000/hour
/// budget, and the 60s floor spends 240.
///
/// `both_cadences_stay_well_inside_the_rate_limit` asserts the FLOOR, not
/// the default, so no reachable setting can blow the budget.
pub fn interval_for_secs(focused: bool, configured_secs: u64) -> Duration {
    let secs = clamp_interval(configured_secs);
    Duration::from_secs(if focused {
        secs
    } else {
        secs * BACKGROUND_MULTIPLIER
    })
}

/// The default cadence, for callers with no configured value.
pub fn interval_for(focused: bool) -> Duration {
    interval_for_secs(focused, DEFAULT_FOCUSED_SECS)
}

/// True if any PR is still waiting on GitHub's lazy mergeability
/// computation. Drives whether a one-shot recheck (#22) is worth
/// scheduling at all -- no `Checking` PRs means nothing to gain from an
/// extra request.
fn has_checking(prs: &[PullRequest]) -> bool {
    prs.iter().any(|pr| pr.merge == MergeState::Checking)
}

/// Overlays freshly-fetched PRs onto a base snapshot by `(repo, number)`
/// identity, leaving every other PR in `base` untouched. Used to fold the
/// #22 targeted recheck's results back into the last known snapshot without
/// discarding PRs the recheck didn't (need to) touch.
///
/// Pure and side-effect free so the merge semantics -- "only the polled
/// identities move, everything else is preserved verbatim" -- are testable
/// without a mock server or a running event loop.
/// Send one desktop notification for a newly-broken PR.
///
/// Failure is logged and swallowed: a notification is an affordance, and
/// losing one must never take down polling. Clicking is wired through the
/// plugin's default behaviour rather than a custom handler, so there is no
/// state to leak if the window is closed.
fn notify_breakage(app: &AppHandle, b: &Breakage) {
    use tauri_plugin_notification::NotificationExt;

    // Ask ONCE, before the first notification rather than at whatever
    // arbitrary moment a PR happens to break. Left implicit, the OS
    // prompt appeared hours in and possibly while the window was hidden;
    // if it was missed or dismissed, `show()` failed forever after and
    // the failure was swallowed by design ("a notification is an
    // affordance"). So a headline feature could be permanently dead with
    // no user-visible signal at all.
    if !notification_allowed(app) {
        return;
    }

    let body = format!("{}#{} {}", b.repo, b.number, b.kind.reason());
    if let Err(e) = app
        .notification()
        .builder()
        .title(b.title.clone())
        .body(body)
        .show()
    {
        log::warn!("failed to show notification: {e}");
    }
}

/// Whether a desktop notification can be shown, asking once if needed.
///
/// Shared with the package-update run, which notifies when a pull
/// request is ready. Extracted rather than duplicated: the ASK-ONCE
/// behaviour below is the load-bearing part, and a second copy would
/// drift from it.
pub(crate) fn notification_allowed(app: &AppHandle) -> bool {
    use tauri_plugin_notification::NotificationExt;

    match app.notification().permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => true,
        Ok(tauri_plugin_notification::PermissionState::Prompt)
        | Ok(tauri_plugin_notification::PermissionState::PromptWithRationale) => {
            match app.notification().request_permission() {
                Ok(tauri_plugin_notification::PermissionState::Granted) => true,
                Ok(_) => false,
                Err(e) => {
                    log::warn!("could not request notification permission: {e}");
                    false
                }
            }
        }
        Ok(tauri_plugin_notification::PermissionState::Denied) => {
            // Logged at INFO, not warn: the user said no, which is a
            // choice rather than a fault. Silence made "why do I get no
            // notifications?" unanswerable from the log.
            log::info!("notifications are denied; not notifying");
            false
        }
        Err(e) => {
            log::warn!("could not read notification permission: {e}");
            false
        }
    }
}

/// A newly-broken PR worth interrupting the user for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakage {
    pub title: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub kind: BreakageKind,
}

/// What broke.
///
/// A type rather than the prose string it used to be, so the settings
/// filter matches on the KIND and cannot drift from display wording. A
/// user who turns off conflict notifications must keep getting CI ones,
/// and comparing on "has merge conflicts" would break the moment that
/// sentence is reworded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakageKind {
    CiFailed,
    Conflicted,
    /// A pull request awaiting YOUR review became ready to pick up.
    ///
    /// The odd one out: this is good news, where the other two are
    /// breakage. The type keeps its name because renaming it touches
    /// every call site for no behavioural gain -- but a third variant
    /// that is not a breakage is exactly the sort of thing that makes a
    /// name wrong, so it is called out here rather than left to be
    /// discovered.
    ReadyToReview,
    /// A pull request that was not in the previous tick's list (#789).
    ///
    /// The FOURTH variant in a type called `BreakageKind`, which by now
    /// makes the name plainly wrong -- two of the four are not breakage.
    /// Still not renamed, for the reason `ReadyToReview` gives: the name
    /// appears at every call site and on the wire in `NotifyPrefs`, and
    /// a rename buys nothing behavioural while touching the settings
    /// struct users have stored values for. If a fifth arrives, rename
    /// it then and migrate the preference keys in the same change.
    Appeared,
}

impl BreakageKind {
    /// The notification body. Reads after "owner/repo#123 ...".
    pub fn reason(self) -> &'static str {
        match self {
            BreakageKind::CiFailed => "CI is failing",
            BreakageKind::Conflicted => "has merge conflicts",
            BreakageKind::ReadyToReview => "is ready for your review",
            BreakageKind::Appeared => "just appeared",
        }
    }

    /// Whether the user wants to hear about this one.
    pub fn enabled_by(self, prefs: &NotifyPrefs) -> bool {
        match self {
            BreakageKind::CiFailed => prefs.ci_failed,
            BreakageKind::Conflicted => prefs.conflicted,
            BreakageKind::ReadyToReview => prefs.ready_to_review,
            BreakageKind::Appeared => prefs.new_pr,
        }
    }
}

/// Interface preferences that Rust needs to know about.
///
/// Lives here beside `NotifyPrefs` rather than in a UI module because
/// `close_hides_to_tray` is read by the window event handler, which is
/// Rust-side and cannot see anything the webview stores.
///
/// `hidden_views` is a plain list of view ids rather than a bool per
/// view, so adding a view later needs no migration and hiding an id
/// this build does not know about is harmless.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UiPrefs {
    /// View ids the switcher should not offer.
    pub hidden_views: Vec<String>,
    /// Whether the close button hides to the tray instead of quitting.
    pub close_hides_to_tray: bool,
    /// Whether a new release announces itself with a dialog.
    ///
    /// `serde(default)` so a settings row written before this field
    /// existed still deserialises -- without it, adding a field would
    /// make every stored value unreadable and silently reset every
    /// other preference alongside it.
    #[serde(default = "default_true")]
    pub announce_updates: bool,
    /// Whether the Claude Code integrations are switched on (#916, epic
    /// #910).
    ///
    /// A CAPABILITY flag, deliberately not an entry in `hidden_views`, and
    /// the distinction is load-bearing rather than pedantic:
    ///
    /// `hidden_views` means "I do not want to see this". It is a user
    /// preference, reversible with no consequences, and `ViewSwitcher`
    /// honours it loosely on purpose -- `ALWAYS_OFFERED` and its
    /// current-view hatch both override it, so a user sitting on a view
    /// keeps it. That is right for a preference and WRONG for a
    /// capability: a switched-off integration must not stay on screen
    /// because someone happened to be looking at it.
    ///
    /// Routing it through `hidden_views` would also clobber a real
    /// preference. Someone who had hidden the Claude Code view for their
    /// own reasons would lose that the first time the capability toggled,
    /// because the two meanings would share one list.
    ///
    /// Defaults OFF. The integrations install hooks into another tool's
    /// configuration, and a feature that edits `~/.claude/settings.json`
    /// should be asked for rather than assumed.
    #[serde(default)]
    pub claude_integrations_enabled: bool,
    /// Whether to write the verbose `[diag]` timing log.
    ///
    /// Added in v3.5.3 to diagnose a slow review query on one machine,
    /// and kept as a switch rather than removed: the next report of
    /// "it is slow on my machine" wants exactly this log, and asking a
    /// user to install a special build to produce it is a much worse
    /// experience than a checkbox.
    ///
    /// Defaults OFF. The logging is per-request and noisy, and a log
    /// nobody asked for is a cost every user pays for a diagnosis
    /// almost none of them need.
    #[serde(default)]
    pub diagnostic_logging: bool,
    /// How many days idle before a virtualenv counts as stale.
    ///
    /// Adjustable because 90 is a default, not a fact: someone with
    /// seasonal projects should be able to move it rather than work
    /// around it. Zero means "use the default" rather than "everything
    /// is stale" -- a stored 0 from a bad write must not reclassify the
    /// whole cache.
    #[serde(default)]
    pub stale_venv_days: u32,
    /// Charge below which the battery alert fires, in percent (#720).
    ///
    /// Zero means "never set" and resolves to
    /// `health::alerts::DEFAULT_LOW_PERCENT`, exactly like
    /// `stale_venv_days` above -- a stored 0 from an upgrade must not
    /// silently disable the alert, and `health::alerts::low_percent`
    /// is where that is decided.
    ///
    /// This is CHARGE, not capacity. The two are different numbers and
    /// the UI keeps them in separate panels; see `health::Battery`.
    #[serde(default)]
    pub battery_low_percent: u32,
}

/// Days idle before a virtualenv is called stale, honouring the setting.
///
/// Clamped rather than trusted. A very small value would call an active
/// project stale, and this number gates a delete once the opt-in above
/// is on -- so the floor is what stops a typo in Settings from making
/// live work selectable.
pub fn stale_venv_days(prefs: &UiPrefs) -> u32 {
    match prefs.stale_venv_days {
        0 => 90,
        d => d.clamp(30, 3650),
    }
}

/// Serde needs a function, not a literal, for a defaulted bool.
fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            // Nothing hidden, and close hides -- exactly what the app
            // did before this setting existed. An upgrade must not
            // change behaviour for someone who never opens Settings.
            hidden_views: Vec::new(),
            close_hides_to_tray: true,
            // OFF by default, for the reason the field's own doc gives:
            // this installs hooks into another tool's config file, which
            // is a thing to be asked for rather than assumed on upgrade.
            claude_integrations_enabled: false,
            // Announcing is the point of checking. The status bar has
            // always shown the hint and it was easy to miss; a user who
            // finds the dialog intrusive can turn it off, which is what
            // the setting is for.
            announce_updates: true,
            // OFF: an upgrade must never widen what a click can delete.
            // 0 means "use the default", resolved by `stale_venv_days`.
            stale_venv_days: 0,
            // 0 is "use the default", not "alert at 0%". See
            // `health::alerts::low_percent`.
            battery_low_percent: 0,
            // OFF. Verbose per-request logging is a cost every user
            // pays for a diagnosis almost none of them need -- it is
            // turned on when someone is chasing a problem.
            diagnostic_logging: false,
        }
    }
}

/// Which notifications the user wants.
///
/// Defaults to everything ON, matching the behaviour before this existed
/// -- an upgrade must not silently turn off a feature someone relies on.
/// `enabled` is a master switch rather than a third kind, so turning
/// notifications off does not lose the per-kind choices underneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotifyPrefs {
    pub enabled: bool,
    pub ci_failed: bool,
    pub conflicted: bool,
    /// Notify when a pull request enters the "Ready for review" set.
    ///
    /// `#[serde(default)]` so an existing stored preference, written
    /// before this field existed, still deserialises -- without it a
    /// missing key would fail the whole struct and silently reset every
    /// other notification setting to its default.
    #[serde(default = "default_true")]
    pub ready_to_review: bool,
    /// Notify when a pull request APPEARS that was not there before
    /// (#789).
    ///
    /// `serde(default)` like the field above, and for the same reason: a
    /// stored preference written before this existed must still
    /// deserialise, or a missing key would fail the whole struct and
    /// silently reset every other notification setting.
    #[serde(default = "default_true")]
    pub new_pr: bool,
    /// Notify about the battery: low charge, fast discharge, or
    /// discharging on AC (#720, gated here for the first time by #789).
    ///
    /// **This is the first time the battery alerts are gateable at all.**
    /// Until now they notified unconditionally, with only
    /// `battery_low_percent` to adjust WHEN -- so a user who wanted
    /// pull-request notifications and not machine ones had no way to say
    /// so. The master switch turned off both or neither.
    ///
    /// One category for all three conditions rather than three, because
    /// they are one subject to a person: "something is wrong with this
    /// machine's power". The desktop already treats them as one -- only
    /// the low threshold is configurable, because the other two have no
    /// threshold worth setting.
    ///
    /// Defaults ON, so an upgrade does not silence an alert someone has
    /// been relying on since #720.
    #[serde(default = "default_true")]
    pub health_battery: bool,
    /// Notify when the CPU is busy with nothing in particular (#791).
    ///
    /// Its own category rather than folded into `health_battery`: the
    /// two answer different questions -- "is this machine about to die"
    /// and "is it burning cores for no reason" -- and someone who wants
    /// one and not the other is making a reasonable choice.
    #[serde(default = "default_true")]
    pub health_cpu: bool,
    /// Notify when a watched Claude Code session dies (#979).
    ///
    /// `#[serde(default)]` like every field above, and for the reason
    /// `ready_to_review` states: without it a stored preference written
    /// before this field existed would fail the WHOLE struct and silently
    /// reset every other notification setting to its default.
    ///
    /// # Why this defaults ON, when "new field, default OFF" is the
    /// # cautious reading
    ///
    /// Every field above defaults ON because those alerts predate their
    /// own gate and an upgrade must not silence something a user relies
    /// on. That argument does not apply here -- this alert is new and
    /// nobody relies on it yet -- so the precedent is not a reason on its
    /// own and #979 is right to ask for it to be argued.
    ///
    /// It is ON because it CANNOT fire unless the user has already opted
    /// in to the thing it is about. `claude_integrations_enabled` defaults
    /// `false` (`UiPrefs::default`) and the sweep that produces this
    /// signal does not run without it, so nobody is interrupted about
    /// something they never asked for -- which is the harm a default-OFF
    /// would be protecting against. A user who turns the Claude Code
    /// integration on is asking the app to watch their sessions, and
    /// "the one you were relying on just died" is the whole of what
    /// watching buys while you are away from the desk.
    ///
    /// Two gates rather than one, so turning the notification off does
    /// not turn the feature off and vice versa: the master switch,
    /// `enabled`, still covers it like everything else.
    #[serde(default = "default_true")]
    pub claude_crashed: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            ci_failed: true,
            conflicted: true,
            ready_to_review: true,
            new_pr: true,
            // ON, matching what #720 and #791 do without a setting. An
            // upgrade must not silently mute an alert someone relies on.
            health_battery: true,
            health_cpu: true,
            // ON, and NOT by copying the rows above -- see the field's
            // own doc. It cannot fire unless the user has already turned
            // the Claude Code integration on, which defaults off, so
            // nobody is interrupted about something they never asked for.
            claude_crashed: true,
        }
    }
}

impl NotifyPrefs {
    /// Whether this breakage should interrupt the user.
    pub fn wants(&self, kind: BreakageKind) -> bool {
        self.enabled && kind.enabled_by(self)
    }

    /// Whether a health condition with this key should interrupt the
    /// user (#789).
    ///
    /// Matches on `health::alerts::Alert::key` and
    /// `health::runaway::Alert::key` -- the stable condition identities,
    /// not the wording, so a reworded alert does not silently change
    /// which category it belongs to.
    ///
    /// An UNKNOWN key is allowed through under `enabled`, deliberately.
    /// A condition a future release adds would otherwise be silent until
    /// someone remembered to add a match arm -- and "a category you
    /// cannot yet switch off" is a much smaller problem than "a warning
    /// you never received". This mirrors `PhoneNotifyPrefs::wants_health`
    /// on the companion, which makes the same call for the same reason.
    pub fn wants_health(&self, key: &str) -> bool {
        if !self.enabled {
            return false;
        }
        match key {
            "low" | "fast_discharge" | "draining_on_ac" => self.health_battery,
            "diffuse_cpu" => self.health_cpu,
            _ => true,
        }
    }
}

/// PRs that just BROKE, by comparing a tick against the one before it.
///
/// Deliberately failures only. A 60s loop across many repos is a firehose
/// if it reports every state change, and "your PR was approved" is good
/// news the badge and list already carry passively. An interruption should
/// mean something needs your hands.
///
/// Only TRANSITIONS fire: a PR that was already red on the previous tick
/// is not re-reported, or every tick would re-notify the same 13 PRs
/// forever. A PR absent from `previous` -- first run, or newly opened --
/// never fires, because its "before" state is unknown and assuming green
/// would notify the whole list on first launch.
/// Whether a pull request is ready for someone to review right now.
///
/// MUST mirror `readyForReview` in `src/lib/derive.ts`, which decides
/// what the green "Ready for review" panel shows. A notification that
/// used its own rule would announce pull requests the panel does not
/// list, and the two would drift apart silently.
///
/// `ci == None` counts as ready: a repository with no checks configured
/// has nothing to wait for. `Pending` does NOT -- ready means the checks
/// passed, not that they have not failed yet.
fn ready_for_review(pr: &PullRequest) -> bool {
    !pr.is_draft
        && (pr.ci == CiState::Success || pr.ci == CiState::None)
        && pr.merge != MergeState::Conflicted
        && pr.review != ReviewState::Approved
        && pr.review != ReviewState::ChangesRequested
        && !pr.in_merge_queue
}

/// Pull requests that have just become ready for the user to review.
///
/// The transition rule is DELIBERATELY different from `newly_broken`.
///
/// That function never fires for a pull request absent from `previous`,
/// because its "before" state is unknown and assuming green would
/// notify the whole list on the first tick. Correct for breakage.
///
/// Here it is backwards: a brand-new pull request that arrives already
/// green, with the user as a reviewer, is EXACTLY the case worth
/// announcing -- and it is always absent from `previous`. So an absent
/// prior state counts as "was not ready".
///
/// The first-tick burst is prevented by the caller instead, which skips
/// this entirely until it has one tick of history. Opening the app must
/// not announce every pull request already waiting.
pub fn newly_ready(previous: &[PullRequest], current: &[PullRequest]) -> Vec<Breakage> {
    current
        .iter()
        .filter(|pr| ready_for_review(pr))
        .filter(|pr| {
            // Absent from `previous` means "was not ready", not "skip".
            previous
                .iter()
                .find(|p| p.repo == pr.repo && p.number == pr.number)
                .is_none_or(|was| !ready_for_review(was))
        })
        .map(|pr| Breakage {
            title: pr.title.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            url: pr.url.clone(),
            kind: BreakageKind::ReadyToReview,
        })
        .collect()
}

/// Pull requests that have just APPEARED (#789).
///
/// Same shape as [`newly_ready`] and [`newly_broken`]: a pure comparison
/// of the previous list against the current one, returning what changed
/// rather than what is true.
///
/// # First-tick suppression is the caller's job, and it is not optional
///
/// Every pull request is absent from `previous` on the first tick, so
/// this would announce the whole list -- thirteen notifications at once
/// on launch. The guard lives in the caller, which skips this entirely
/// until it has one tick of history, exactly as it does for
/// [`newly_ready`]. That is why an absent prior state counts as "did not
/// exist" here rather than "skip": the two rules each need the other
/// half to be correct.
///
/// Note this is the OPPOSITE treatment from the battery alerts, which
/// deliberately re-arm on relaunch (`lib.rs`). A battery alert is a
/// standing condition and restating it once is a service; a pull request
/// appearing is an EVENT, and re-announcing it after a restart would
/// claim an event that did not happen.
///
/// # Drafts are excluded
///
/// A draft is work its author has explicitly marked as not ready to be
/// looked at, so its creation is not news -- the same judgement
/// [`ready_for_review`] makes. It leaving draft would be news, and that
/// is a transition this release does not detect.
pub fn newly_appeared(previous: &[PullRequest], current: &[PullRequest]) -> Vec<Breakage> {
    current
        .iter()
        .filter(|pr| !pr.is_draft)
        .filter(|pr| {
            !previous
                .iter()
                .any(|p| p.repo == pr.repo && p.number == pr.number)
        })
        .map(|pr| Breakage {
            title: pr.title.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            url: pr.url.clone(),
            kind: BreakageKind::Appeared,
        })
        .collect()
}

pub fn newly_broken(previous: &[PullRequest], current: &[PullRequest]) -> Vec<Breakage> {
    current
        .iter()
        .filter_map(|pr| {
            let was = previous
                .iter()
                .find(|p| p.repo == pr.repo && p.number == pr.number)?;
            let kind = if pr.ci == CiState::Failure && was.ci != CiState::Failure {
                BreakageKind::CiFailed
            } else if pr.merge == MergeState::Conflicted && was.merge != MergeState::Conflicted {
                BreakageKind::Conflicted
            } else {
                return None;
            };
            Some(Breakage {
                title: pr.title.clone(),
                repo: pr.repo.clone(),
                number: pr.number,
                url: pr.url.clone(),
                kind,
            })
        })
        .collect()
}

fn merge_by_identity(base: &[PullRequest], updates: &[PullRequest]) -> Vec<PullRequest> {
    base.iter()
        .map(|pr| {
            updates
                .iter()
                .find(|u| u.repo == pr.repo && u.number == pr.number)
                .cloned()
                .unwrap_or_else(|| pr.clone())
        })
        .collect()
}

/// Persists a snapshot and emits `prs-updated`, matching every fallible
/// step rather than unwrapping -- shared by both the regular poll tick and
/// the #22 one-shot recheck so the "never panic, never blank the UI on
/// failure" discipline lives in exactly one place.
/// Tell the UI a local write failed.
///
/// Reuses the existing `poll-error` banner rather than adding a channel:
/// the snapshot failing means offline readability and cold-start speed are
/// gone, which the user should know about even though the live list is
/// unaffected.
fn emit_store_error(app: &AppHandle, msg: String) {
    // Its OWN channel, not `poll-error`. Sharing it meant this banner was
    // destroyed microseconds after it appeared: persist_and_emit emits
    // the error and then UNCONDITIONALLY emits `prs-updated`, which the
    // frontend uses to clear poll errors. A full disk was invisible.
    //
    // A store failure also describes a condition the successful poll did
    // NOT fix, so a later success must not clear it.
    if let Err(e) = app.emit("store-error", msg) {
        log::warn!("failed to emit store error: {e}");
    }
}

/// The user's notification choices, or the default if unreadable.
///
/// Every failure path here -- no data dir, no database, a corrupt value
/// -- falls back to `NotifyPrefs::default()`, which is everything ON.
/// That direction is deliberate: the alternative is that a transient
/// database problem silently mutes an interruption channel the user is
/// relying on, and they would have no way to tell that from "nothing
/// broke". A notification too many is recoverable; a missed one is not.
///
/// # Why `spawn_blocking` (#1090)
///
/// This runs inside `tauri::async_runtime::spawn`, twice per tick, and it
/// opens SQLite -- a file open, a schema check and a query. That is disk
/// work on an async runtime worker, and this loop is the one thing in the
/// app that does it forever on a timer. `poll.rs`'s own diagnostic
/// comment one screen up already worries that "a foreground query can be
/// what makes the foreground query look slow"; this is the half of that
/// which was measurable and unnecessary.
async fn read_notify_prefs(app: &AppHandle) -> NotifyPrefs {
    let Ok(dir) = app.path().app_data_dir() else {
        return NotifyPrefs::default();
    };
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(conn) = open_db(&dir.join("headstate.db")) else {
            return NotifyPrefs::default();
        };
        crate::store::settings::get(&conn, crate::store::settings::keys::NOTIFY_PREFS)
            .ok()
            .flatten()
            .unwrap_or_default()
    })
    .await
    // Every other failure path here falls back to the default -- see the
    // doc above on why that direction is deliberate -- and a panicked
    // join is not the one case to start muting notifications on.
    .unwrap_or_default()
}

/// Write the snapshot and tell the UI.
///
/// # Why the WRITE is `spawn_blocking` and the emits are not (#1090)
///
/// `save_snapshot` serialises the list and COMMITS, which is an `fsync` --
/// the unbounded-latency case, on a runtime worker, on a timer, forever.
/// `app.emit` is a channel send and `set_badge` is a platform call; both
/// are cheap and neither touches the disk, so only the database half moves
/// off the runtime. Splitting it this way also keeps the ordering the UI
/// depends on: the snapshot is on disk before `prs-updated` says so.
async fn persist_and_emit(app: &AppHandle, prs: &[PullRequest]) {
    match app.path().app_data_dir() {
        Ok(dir) => {
            let owned: Vec<PullRequest> = prs.to_vec();
            let written = tauri::async_runtime::spawn_blocking(move || {
                let conn =
                    open_db(&dir.join("headstate.db")).map_err(|e| (true, format!("{e}")))?;
                save_snapshot(&conn, CachedList::Authored, &owned)
                    .map_err(|e| (false, format!("{e}")))
            })
            .await;
            match written {
                Ok(Ok(())) => {}
                Ok(Err((opening, e))) if opening => {
                    log::error!("failed to open db: {e}");
                    emit_store_error(app, format!("could not open the local database: {e}"));
                }
                Ok(Err((_, e))) => {
                    log::error!("failed to save snapshot: {e}");
                    emit_store_error(app, format!("could not save local snapshot: {e}"));
                }
                // A panic in the closure. Reported rather than swallowed:
                // the snapshot not landing is exactly what `store-error`
                // exists to say, and silence here would be the "offline
                // readability is gone and nothing told you" failure the
                // banner was added for.
                Err(e) => {
                    log::error!("the snapshot write task failed: {e}");
                    emit_store_error(app, format!("could not save local snapshot: {e}"));
                }
            }
        }
        Err(e) => {
            log::error!("failed to resolve app data dir: {e}");
            emit_store_error(app, format!("could not find the app data directory: {e}"));
        }
    }
    if let Err(e) = app.emit("prs-updated", prs) {
        log::warn!("failed to emit prs-updated: {e}");
    }
    // The badge is why polling lives in Rust at all: it has to stay correct
    // while the window is hidden, when no React component is mounted to
    // compute it. Counted here from the same list just persisted, using the
    // model's single owner of the rule.
    crate::tray::set_badge(app, needs_attention_count(prs));
}

/// #22: schedules exactly one targeted re-poll ~`RECHECK_DELAY` after a
/// poll that left PRs in `MergeState::Checking`, so a mergeability check
/// that GitHub hadn't finished computing yet gets a chance to resolve
/// before the next regular tick (60s/300s) instead of always waiting for
/// it.
///
/// "Exactly one" is enforced structurally, not by a retry counter: this
/// function calls `client.fetch_prs()` a single time and then returns --
/// there is no loop, no re-scheduling of itself, and no path back into this
/// function from within it. Whatever happens (success, network error, or
/// nothing left `Checking` by the time it fires), the task ends and the
/// regular poll loop's own next tick is what runs after that. A failed
/// recheck logs and returns without touching the snapshot, so the last good
/// snapshot on disk is left exactly as the regular tick left it -- the UI
/// is never blanked.
fn spawn_recheck(app: AppHandle, client: Arc<GitHubClient>, last_known: Vec<PullRequest>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RECHECK_DELAY).await;

        match client.fetch_prs().await {
            Ok(fresh) => {
                let merged = merge_by_identity(&last_known, &fresh);
                persist_and_emit(&app, &merged).await;
            }
            Err(e) => {
                // No retry: the regular 60s/300s cadence picks this back up
                // on its own next tick. Logging only, snapshot untouched.
                log::warn!("targeted recheck failed: {e}");
            }
        }
    });
}

/// Spawn the poll loop. Each tick fetches, writes the snapshot, and emits
/// `prs-updated`; the frontend invalidates its query on that event. If the
/// fetch left any PR in `MergeState::Checking`, it also schedules the #22
/// one-shot recheck described on `spawn_recheck` above.
///
/// A failed poll leaves the last snapshot on disk in place rather than
/// blanking the UI: on error we emit `poll-error` and let the next tick
/// retry, we never clear the cache. Nothing in this loop panics -- a panic
/// in a spawned task would silently kill polling for the rest of the
/// session, so every fallible step here is matched or logged, never
/// unwrapped.
/// Wall-clock ceiling on one poll's fetch.
///
/// The transport timeouts in `auth::build_client` bound individual socket
/// operations; this bounds the whole request. A server that trickles bytes
/// can keep a read alive indefinitely without ever tripping a read timeout,
/// and the loop must reach its sleep either way.
///
/// Bounded BELOW `MIN_FOCUSED_SECS`, which is the property that matters
/// and which 90s did not have. At the fastest interval the user can
/// choose, a 90s ceiling means a hung poll is still in flight when the
/// next tick fires: two fetches overlap, each spending the rate-limit
/// budget, and the banner blames whichever loses. A ceiling under the
/// floor makes a poll's failure land strictly before its successor
/// starts.
///
/// 30s, not lower: measured per-POST latency on a reported six-day
/// session is p50 6,655ms, p90 8,814ms, p99 30,126ms (n=3,806). A 30s
/// ceiling therefore abandons roughly the slowest 1% and leaves p90 with
/// better than 3x headroom. The three polls in that log which ran to the
/// old 90s ceiling had been useless for 90 seconds each while the next
/// tick was only 120s away -- giving up at 30s and retrying is strictly
/// better than waiting out a request that has already missed its window.
///
/// The measured floor is `mergeStateStatus`, which GitHub computes per
/// pull request synchronously (see `client.rs`). That is why this is a
/// ceiling rather than a target: the query cannot be made reliably fast,
/// so the loop's job is to fail fast and retry rather than to hang.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The ceiling on a whole TICK, which is what must clear the next one.
///
/// # The gap this closes (#844)
///
/// `FETCH_TIMEOUT` bounds ONE fetch, and
/// `the_fetch_ceiling_is_under_the_shortest_poll_interval` asserted
/// `30s < 60s` on the premise that a tick IS one fetch. It is not: a tick
/// awaits two sequential 30-second timeouts (`:868` and `:884`), so the
/// worst case is 60s -- **exactly equal** to `MIN_FOCUSED_SECS`, which
/// reintroduces the overlapping-fetch regression that test was written to
/// prevent. Two hung fetches, both spending budget, for requests that had
/// already been useless for a full interval.
///
/// # Why a shared deadline rather than halving `FETCH_TIMEOUT`
///
/// #844 offers both. Halving does not survive the OTHER guard:
/// `the_fetch_ceiling_still_clears_the_measured_p90` requires
/// `FETCH_TIMEOUT >= 3 x p90`, and p90 is 8,814ms per POST over 3,806
/// requests in a reported six-day session -- so 15s would leave 1.7x
/// headroom and cut off polls on accounts that are merely large, which is
/// the failure that guard exists to prevent. The two requirements are only
/// jointly satisfiable by bounding the TICK rather than shrinking each
/// fetch, so the deadline is shared: the second fetch gets whatever the
/// first left of it.
///
/// # And NOT by parallelising the two fetches
///
/// The obvious alternative is `tokio::join!`, and it is measurably worse.
/// MEASURED (#844, and reproduced by this issue's own figures): sequential
/// 3.5s against concurrent 5.6s -- GitHub contends on simultaneous
/// node-heavy queries, so issuing both at once makes the slow one slower
/// by more than the overlap saves. `fetch_prs_and_reviewing`
/// (`client.rs:564`) does run two searches concurrently, and its own
/// diagnostic comment is about exactly this: a total far above the slower
/// of the two means they are not actually overlapping.
///
/// # Why 45s
///
/// It must be under `MIN_FOCUSED_SECS` (60) with enough margin that a tick
/// finishing at the ceiling still lands before its successor starts, and
/// it must leave the FIRST fetch its full `FETCH_TIMEOUT` -- the authored
/// list is what the UI renders, and the review queue is a notification
/// whose loss costs nothing (`:880`). 45s gives the first fetch all 30s
/// and the second up to 15s, and 45 < 60 holds with 15s of slack.
///
/// The budget is generous on purpose for `fetch::LOAD_TIMEOUT`'s reason:
/// "the budget exists to convert an unbounded hang into an actionable
/// error, not to tighten a latency target". Measured tick latency is
/// 3.37s + 0.78s = ~4.2s, so this ceiling is an order of magnitude above
/// what a real tick needs.
pub const TICK_TIMEOUT: Duration = Duration::from_secs(45);

/// What is left of [`TICK_TIMEOUT`] after `spent`, capped at
/// [`FETCH_TIMEOUT`].
///
/// Extracted rather than inlined in the loop for the reason
/// `clamp_interval` is: the loop needs a live `AppHandle` and a network, so
/// the arithmetic is only testable if it lives apart from them. The
/// property that matters -- the two fetches together cannot outlast a tick
/// -- is then asserted directly instead of inferred from two constants.
///
/// Capped at `FETCH_TIMEOUT` so the second fetch never gets a LONGER
/// ceiling than the first: a fast authored fetch leaves 40+ seconds, and
/// handing all of it to the review queue would make a hung notification
/// fetch the thing that overruns the tick.
///
/// Saturating, so a first fetch that somehow outran the whole budget
/// yields zero rather than wrapping into a near-infinite ceiling. The
/// caller treats zero as "skip": a request with no time to answer still
/// spends a rate-limit point.
pub fn remaining_tick_budget(spent: Duration) -> Duration {
    TICK_TIMEOUT.saturating_sub(spent).min(FETCH_TIMEOUT)
}

/// Wakes the poll loop out of its sleep.
///
/// Managed in Tauri state so the tray and the window-focus handler can
/// reach it. One mechanism covers two problems:
///
/// 1. Tray "Refresh now" only emitted an event whose sole listener is a
///    React effect inside `App` -- so while the window was hidden the
///    click did a real fetch whose result landed in a cache nobody was
///    looking at, never persisted, and never touched the badge.
/// 2. `tokio::time::sleep` does not fire during macOS system sleep and
///    does not compensate on wake, so after a closed lid the first tick
///    was delayed up to a full interval (300s when backgrounded).
pub struct Waker(pub Arc<Notify>);

/// The configured focused interval, in seconds.
///
/// Shared rather than passed once, so changing the setting takes effect on
/// the NEXT tick instead of requiring a relaunch. Paired with the `Waker`:
/// after a change the loop is woken so a shortened interval applies
/// immediately rather than after the old, longer sleep expires.
pub struct PollInterval(pub Arc<AtomicU64>);

/// Whether the active view needs live GitHub data.
///
/// `false` while the user is looking at local worktrees, which need no
/// PR data at all. The loop drops to the BACKGROUND cadence rather than
/// stopping: the tray badge must not go stale while the window sits open
/// on another view, and the badge staying honest is the stated reason
/// polling lives in Rust at all.
pub struct ViewNeedsGithub(pub Arc<AtomicBool>);

/// How many consecutive transient failures before the banner appears.
///
/// Measured on a real log: 5 of 164 polls failed with a transport error
/// and every one recovered on the very next tick. One blip is weather;
/// two in a row is a problem worth naming.
///
/// Deliberately small. This delays a real outage's banner by one poll,
/// which is a fair price for not alarming a user about something that
/// fixed itself before they finished reading it.
const FAILURES_BEFORE_BANNER: u32 = 2;

/// Whether a failure is worth interrupting the user for.
///
/// Actionable failures -- a dead token, an exhausted rate limit, a
/// malformed query -- surface immediately, because the next tick will
/// fail identically and waiting cannot help. Transient ones wait for a
/// second opinion.
fn should_surface(e: &ClientError, consecutive: u32) -> bool {
    !e.is_transient() || consecutive >= FAILURES_BEFORE_BANNER
}

/// The `poll-state` a finished tick leaves behind.
///
/// The loop emits this ONCE, at the end of every tick, after the request is
/// no longer in flight. It carries the tick's outcome rather than a fixed
/// `"idle"` because a suppressed transient failure has nothing else to
/// announce it: no `poll-error`, no `prs-updated`. Emitting `"idle"`
/// unconditionally told the bar the data was current when the fetch had just
/// failed, so it showed a green "Up to date" over stale rows for up to ten
/// minutes (#1104).
///
/// `None` is a success. `Some(surfaced)` is a failure, where `surfaced` says
/// whether a `poll-error` banner was already emitted for it -- in that case
/// the banner is the signal and the bar has no further work to do.
fn tick_state(failure: Option<bool>) -> &'static str {
    match failure {
        Some(false) => "retrying",
        Some(true) | None => "idle",
    }
}

pub fn spawn(
    app: AppHandle,
    client: Arc<GitHubClient>,
    focused: Arc<AtomicBool>,
    waker: Arc<Notify>,
    interval_secs: Arc<AtomicU64>,
    view_needs_github: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        let mut previous: Vec<PullRequest> = Vec::new();
        // Whether a tick has ever completed, so `newly_appeared` has
        // something real to compare against (#789).
        //
        // Not `!previous.is_empty()`: a user whose last pull request
        // merged has a genuinely empty list, and the next one they open
        // is news. "Empty" and "never looked" are different answers, and
        // only this flag distinguishes them.
        let mut had_a_tick = false;
        // The review queue as of the last tick, and whether there HAS
        // been one.
        //
        // `None` rather than an empty Vec: for the ready-to-review
        // notification an absent prior entry means "was not ready", so
        // an empty history and a genuinely empty queue would be
        // indistinguishable -- and the first tick would announce every
        // pull request already waiting.
        let mut previous_reviewing: Option<Vec<PullRequest>> = None;
        // Consecutive failures, reset by any success. Transient failures
        // are not surfaced until this crosses the threshold -- see
        // `should_surface`.
        let mut consecutive_failures: u32 = 0;
        loop {
            // `timeout` collapses a hang into the Err arm the loop already
            // handles, so a wedged request costs one tick instead of the
            // rest of the session.
            let _ = app.emit("poll-state", "fetching");
            // What this tick leaves behind, emitted once at the end. A
            // suppressed failure sets "retrying"; everything else is
            // genuinely idle. Reset per tick so a recovery clears it.
            let mut ending_state = tick_state(None);
            // DIAGNOSTIC LOGGING (Settings > diagnostic log). The background loop
            // shares one client -- and one connection pool -- with
            // whatever the user just clicked, so a tick that overlaps a
            // foreground query can be what makes the foreground query
            // look slow. Logging the tick boundaries makes that overlap
            // visible against the `cmd get_reviewing` bracket.
            crate::diag!("[diag] poll tick start");
            let tick_started = std::time::Instant::now();
            // ONE deadline across BOTH fetches, not one per fetch (#844).
            // Two independent 30s ceilings make the worst-case tick 60s,
            // exactly `MIN_FOCUSED_SECS` -- so a hung tick overlaps its own
            // successor, which is the regression
            // `the_fetch_ceiling_is_under_the_shortest_poll_interval` exists
            // to prevent. See `TICK_TIMEOUT` for why the fix is a shared
            // deadline rather than a smaller `FETCH_TIMEOUT` or a
            // `tokio::join!`.
            let fetched =
                match tokio::time::timeout(FETCH_TIMEOUT, client.fetch_prs_with_total()).await {
                    Ok(res) => res,
                    Err(_) => Err(ClientError::Timeout(FETCH_TIMEOUT.as_secs())),
                };

            // The review queue, for the ready-to-review notification.
            //
            // A SEPARATE request rather than `fetch_prs_and_reviewing`,
            // which returns both but drops the total this loop needs.
            // Fetched only when the notification is wanted, so a user
            // who turns it off pays nothing.
            //
            // A failure here is NOT a tick failure: the authored list
            // above is what the UI renders, and losing one notification
            // must not cost the poll. That is also what makes it the right
            // half to squeeze when the shared deadline is nearly spent.
            let remaining = remaining_tick_budget(tick_started.elapsed());
            let reviewing_now = if read_notify_prefs(&app).await.ready_to_review {
                if remaining.is_zero() {
                    // The authored fetch used the whole tick. Skipped rather
                    // than issued with no time to answer: a request that
                    // cannot finish still SPENDS a rate-limit point, and the
                    // next tick is about to ask the same question with a
                    // full budget.
                    crate::diag!("[diag] poll reviewing skipped: tick budget spent");
                    None
                } else {
                    match tokio::time::timeout(remaining, client.fetch_reviewing()).await {
                        Ok(Ok(list)) => Some(list),
                        Ok(Err(e)) => {
                            crate::diag!("[diag] poll reviewing failed: {e}");
                            None
                        }
                        Err(_) => {
                            crate::diag!(
                                "[diag] poll reviewing timed out after {}ms of tick budget",
                                remaining.as_millis()
                            );
                            None
                        }
                    }
                }
            } else {
                None
            };
            crate::diag!(
                "[diag] poll tick fetch done {}ms {}",
                tick_started.elapsed().as_millis(),
                match &fetched {
                    Ok((prs, total)) => format!("ok n={} total={total}", prs.len()),
                    Err(e) => format!("err: {e}"),
                }
            );
            match fetched {
                Ok((prs, total)) => {
                    // Compare against the tick before this one. `previous`
                    // starts empty, so the first tick never notifies --
                    // otherwise launching with 13 broken PRs would fire 13
                    // notifications at once.
                    // Read per tick rather than cached at startup, so a
                    // setting change takes effect on the next poll
                    // instead of at the next relaunch. A failed read
                    // falls back to the default (everything on), which
                    // is what the app did before the setting existed.
                    let prefs = read_notify_prefs(&app).await;
                    for b in newly_broken(&previous, &prs) {
                        if prefs.wants(b.kind) {
                            notify_breakage(&app, &b);
                        }
                    }
                    // Newly APPEARED pull requests (#789).
                    //
                    // Gated on `had_a_tick` rather than on `previous`
                    // being non-empty, and that distinction is the whole
                    // of the first-tick suppression. `previous` starts
                    // EMPTY, so `!previous.is_empty()` would be false on
                    // the first tick and also false on the first tick of
                    // a user whose last pull request merged -- and the
                    // second of those is a real empty list whose next
                    // arrival IS news. A separate flag says "we have
                    // compared at least once", which is the actual
                    // question.
                    //
                    // `newly_broken` above needs no such guard: it never
                    // fires for a pull request absent from `previous`, so
                    // an empty previous list announces nothing by
                    // construction. This rule is the opposite shape --
                    // absent means new -- so it needs the flag.
                    if had_a_tick {
                        for b in newly_appeared(&previous, &prs) {
                            if prefs.wants(b.kind) {
                                notify_breakage(&app, &b);
                            }
                        }
                    }
                    had_a_tick = true;
                    // Ready-to-review, from the queue fetched above.
                    //
                    // Skipped entirely until there is one tick of
                    // history: without this, opening the app announces
                    // every pull request already waiting.
                    if let Some(now) = reviewing_now {
                        if let Some(before) = &previous_reviewing {
                            for b in newly_ready(before, &now) {
                                if prefs.wants(b.kind) {
                                    notify_breakage(&app, &b);
                                }
                            }
                        }
                        previous_reviewing = Some(now);
                    }

                    previous = prs.clone();
                    // Emitted on EVERY tick, including the complete one
                    // -- see `truncation_payload` for why the zero
                    // matters as much as the count.
                    let truncated = truncation_payload(prs.len() as u64, total);
                    if truncated > 0 {
                        log::warn!("truncated: showing {} of {total} open PRs", prs.len());
                    }
                    if let Err(e) = app.emit("prs-truncated", truncated) {
                        log::warn!("failed to emit prs-truncated: {e}");
                    }
                    // The heartbeat that makes "it stopped updating"
                    // answerable: if the log ends here, the loop died or
                    // the machine slept; if it keeps ticking, the problem
                    // is downstream. Counts only -- never titles, never
                    // repository names.
                    log::info!(
                        "poll ok: {} open, {} need attention (of {total} matching)",
                        prs.len(),
                        needs_attention_count(&prs)
                    );
                    // Fields GitHub refused on this fetch, then cleared:
                    // a later complete response must stop reporting a
                    // shortfall that no longer exists. Emitted even when
                    // zero, so the banner disappears on recovery rather
                    // than sticking until relaunch.
                    let refused = crate::github::client::REFUSED_FIELDS.swap(0, Ordering::Relaxed);
                    if let Err(e) = app.emit("prs-incomplete", refused) {
                        log::warn!("failed to emit prs-incomplete: {e}");
                    }

                    consecutive_failures = 0;
                    persist_and_emit(&app, &prs).await;
                    if has_checking(&prs) {
                        spawn_recheck(app.clone(), client.clone(), prs);
                    }
                }
                // A failed poll leaves the last snapshot in place rather
                // than blanking the UI; the next tick retries.
                Err(e) => {
                    log::warn!("poll failed: {e}");
                    consecutive_failures += 1;
                    if should_surface(&e, consecutive_failures) {
                        if let Err(emit_err) = app.emit("poll-error", e.to_string()) {
                            log::warn!("failed to emit poll-error: {emit_err}");
                        }
                    } else {
                        log::info!(
                            "not surfacing a transient failure ({consecutive_failures} in a row); \
                             the next tick should recover"
                        );
                        // The bar has nothing else to go on: no
                        // poll-error and no prs-updated on a suppressed
                        // failure, so it would otherwise show a green
                        // "Up to date" while the data is stale.
                        //
                        // Recorded rather than emitted here: the terminal
                        // emit below runs on EVERY tick, so emitting
                        // "retrying" at this point would be overwritten by
                        // it microseconds later and the bar would settle on
                        // green anyway -- the exact bug this branch exists
                        // to prevent (#1104).
                        ending_state = tick_state(Some(false));
                    }
                }
            }
            // The bar shows FETCHING only while a request is genuinely in
            // flight. Inferring it from `isFetching` would miss the tray
            // path, which bypasses the queryFn -- so the loop that knows
            // says so directly.
            //
            // Carries the tick's OUTCOME, not a fixed "idle": a suppressed
            // failure leaves the data stale, and the bar has no other way to
            // learn that. See `tick_state`.
            let _ = app.emit("poll-state", ending_state);

            // Whichever comes first: the cadence elapsing, or someone
            // asking for a refresh. `Notify` stores one permit, so a
            // request that arrives mid-fetch is not lost -- the next
            // `notified()` returns immediately rather than waiting out a
            // full interval.
            let sleep_for = interval_for_secs(
                focused.load(Ordering::Relaxed) && view_needs_github.load(Ordering::Relaxed),
                interval_secs.load(Ordering::Relaxed),
            );
            crate::diag!("[diag] poll tick sleeping {}s", sleep_for.as_secs());
            tokio::select! {
                _ = tokio::time::sleep(interval_for_secs(
                    // A view that does not show PR data polls at the
                    // background rate even when the window is focused.
                    focused.load(Ordering::Relaxed)
                        && view_needs_github.load(Ordering::Relaxed),
                    interval_secs.load(Ordering::Relaxed),
                )) => {}
                _ = waker.notified() => {}
            }
        }
    });
}

/// What to tell the frontend about truncation on this tick.
///
/// GitHub's true count while the list is short, and `0` once it is
/// complete.
///
/// The zero is the point. This used to be emitted ONLY while the list
/// was short, and `useTruncation` holds the last value it received --
/// so a poll that recovered and returned everything left "showing 8 of
/// 29" sitting over a complete list until the app was relaunched, which
/// is a false warning where there had been a true one (#745).
/// `prs-incomplete` already emits its zero for exactly this reason;
/// truncation was the one advisory that could not take itself back.
///
/// One event carrying zero rather than a second "cleared" event, so the
/// two states cannot arrive out of order and disagree.
///
/// `saturating_sub` because the two numbers come from different parts of
/// the same response: `issueCount` is GitHub's, the length is what
/// survived mapping, and a list LONGER than the count is not a negative
/// truncation.
fn truncation_payload(fetched: u64, total: u64) -> u64 {
    if total.saturating_sub(fetched) > 0 {
        total
    } else {
        0
    }
}

/// Spawn the PR Stats backfill worker (#1092, #1093).
///
/// # Why this is a SIBLING of [`spawn`] and not part of its tick
///
/// Two reasons, and `stats::backfill`'s module docs carry them in full.
/// [`TICK_TIMEOUT`]'s margin under [`MIN_FOCUSED_SECS`] is load-bearing --
/// a tick that overruns overlaps its successor and two hung fetches both
/// spend budget -- so adding a consumer inside the tick spends exactly the
/// margin that margin exists to protect. And `budget::RESERVE` exists to
/// protect THIS loop; putting the backfill inside the loop `RESERVE`
/// protects would make the protection self-referential.
///
/// The worker therefore has its own cadence, its own floor above
/// `RESERVE`, and no ability to delay a poll tick.
///
/// # What one tick does
///
/// Picks the least recently advanced scope a user has opened, reads the
/// ledger for the days it has never retrieved, and asks for up to
/// [`stats::backfill::GROUP_SLICES`] of them -- one document, MEASURED at
/// 1 point. The rows and the ledger entry claiming them land in ONE
/// transaction, so a crash can lose the work but can never leave the
/// ledger claiming work that is not there.
///
/// Nothing is written before the fetch, which is what makes a crash
/// mid-tick leave no stuck row: there is no `pending` state to be stuck
/// in.
pub fn spawn_backfill(app: AppHandle, client: Arc<GitHubClient>) {
    tauri::async_runtime::spawn(async move {
        // The first tick waits a full interval rather than firing at
        // launch. Startup is the busiest moment the app has -- the poll
        // loop's first tick, the window's first render and whatever the
        // user clicks are all competing -- and a backfill is the one
        // consumer that can always wait.
        loop {
            tokio::time::sleep(crate::github::stats::backfill::BACKFILL_INTERVAL).await;
            let outcome = backfill_tick(&app, &client).await;
            match &outcome {
                crate::github::stats::backfill::TickOutcome::Advanced { days, prs } => {
                    crate::diag!("[diag] stats backfill advanced {days} days, {prs} pull requests");
                }
                crate::github::stats::backfill::TickOutcome::Failed(e) => {
                    // `warn`, never a user-facing error. Nobody asked for
                    // this work, so a failure is not an event the user
                    // must act on -- and the days stay uncovered, so the
                    // next tick simply tries them again.
                    log::warn!("stats backfill tick failed: {e}");
                }
                other => crate::diag!("[diag] stats backfill {other:?}"),
            }
        }
    });
}

/// One backfill tick: pick a scope, fetch a group of uncovered days, write
/// them down.
///
/// Separated from the loop so the sequencing is readable and so the loop
/// itself holds no state that a failure could corrupt.
async fn backfill_tick(
    app: &AppHandle,
    client: &Arc<GitHubClient>,
) -> crate::github::stats::backfill::TickOutcome {
    use crate::github::stats::backfill::{self as bf, TickOutcome};

    // The gate, BEFORE any request. `None` means skip -- deliberately the
    // opposite of `Budget::permits`' arm, because nobody is waiting for
    // this and an unknown budget costs one minute rather than a user's
    // first click.
    let observed = crate::github::stats::budget::observed_remaining();
    if !bf::affordable(observed, bf::TICK_PROJECTION) {
        return TickOutcome::Skipped {
            remaining: observed,
        };
    }

    let db = crate::commands::db_path(app);
    let now = chrono::Utc::now();

    // Which scope, and what it still owes -- off the runtime, because this
    // is SQLite (`persist_and_emit` states the rule).
    let picked = {
        let db = db.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<_>, String> {
            let conn = open_db(&db).map_err(|e| e.to_string())?;
            let Some(scope) =
                crate::store::pr_backfill_scope::next_to_work(&conn).map_err(|e| e.to_string())?
            else {
                return Ok(None);
            };
            let Some((from, to)) = bf::horizon_window(now, scope.horizon_days) else {
                return Ok(None);
            };
            let uncovered =
                crate::store::pr_slice::uncovered_days(&conn, &scope.scope_key, &from, &to)
                    .map_err(|e| e.to_string())?;
            Ok(Some((scope, from, to, uncovered)))
        })
        .await
    };
    let picked = match picked {
        Ok(Ok(Some(v))) => v,
        Ok(Ok(None)) => return TickOutcome::NoScope,
        Ok(Err(e)) => return TickOutcome::Failed(e),
        Err(e) => return TickOutcome::Failed(e.to_string()),
    };
    let (scope, from, to, uncovered) = picked;

    if uncovered.is_empty() {
        // Every day in the horizon is covered. The scope is still marked
        // worked, so the rotation moves on rather than re-deciding this
        // same scope every minute.
        mark_worked(&db, &scope.scope_key, now).await;
        emit_backfill(app, &db, &scope.scope_key, &from, &to, false).await;
        return TickOutcome::Complete;
    }

    // A measure whose days could never settle is not walked at all --
    // see `backfill::walkable`. Marked worked so it cannot hold the
    // rotation, and left alone otherwise.
    if !bf::walkable(&scope.measure) {
        mark_worked(&db, &scope.scope_key, now).await;
        return TickOutcome::Complete;
    }
    let Some(q) = bf::query_for(&scope.scope_kind, &scope.scope_value, &scope.measure) else {
        // A row this build cannot interpret. Marked worked so it cannot
        // hold the rotation, and left alone otherwise.
        mark_worked(&db, &scope.scope_key, now).await;
        return TickOutcome::Failed(format!("unreadable scope kind {}", scope.scope_kind));
    };

    let group: Vec<String> = uncovered.iter().take(bf::GROUP_SLICES).cloned().collect();
    let slices = bf::day_slices(&group);
    let budget = bf::tick_budget();

    // One document for the whole group, MEASURED at 1 point.
    let fetched = crate::github::stats::fetch::load_detail_chunked(
        client,
        &q,
        &slices,
        &budget,
        bf::GROUP_SLICES,
    )
    .await;
    let map = match fetched {
        Ok(v) => v,
        Err(e) => {
            // The days stay uncovered, so the next tick retries them.
            // Nothing was written, so there is no row to get stuck.
            mark_worked(&db, &scope.scope_key, now).await;
            return TickOutcome::Failed(e.to_string());
        }
    };

    // Map the response the SAME way a foreground board does -- one mapper,
    // so a backfilled row and a clicked one cannot differ.
    let prs = crate::github::stats::Board::retrieved_prs(&map, &slices);
    let refused = map["__refused"].as_u64().unwrap_or(0);
    let counts: Vec<u64> = slices
        .iter()
        .enumerate()
        .map(|(i, _)| {
            map[crate::github::stats::query::slice_alias(i)]["issueCount"]
                .as_u64()
                .unwrap_or(0)
        })
        .collect();

    let scope_key = scope.scope_key.clone();
    let written = {
        let db = db.clone();
        let slices = slices.clone();
        let key = scope_key.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<(usize, usize), String> {
            let mut conn = open_db(&db).map_err(|e| e.to_string())?;
            let mut days = 0;
            let mut rows = 0;
            for (i, slice) in slices.iter().enumerate() {
                // Rows belonging to THIS slice only. A day's rows and the
                // claim about that day land together, so a partially
                // written group leaves whole days rather than partial ones.
                let mine: Vec<_> = prs
                    .iter()
                    .filter(|p| p.merged_at == slice.from)
                    .cloned()
                    .collect();
                let count = counts.get(i).copied().unwrap_or(0);
                // Refusals are attributed to the whole document rather
                // than to one alias -- `Outcome::refused_fields` records
                // that the sliced path cannot attribute them -- so a
                // refusal marks every slice in the group as refused. That
                // is the conservative direction: the days are re-asked.
                let row = crate::github::stats::backfill::classify(
                    slice,
                    count,
                    mine.len() as u64,
                    refused,
                );
                if row.state.settled() {
                    days += 1;
                }
                rows += crate::store::pr_slice::record_with_rows(&mut conn, &key, &row, &mine, now)
                    .map_err(|e| e.to_string())?;
            }
            let _ = crate::store::pr_history::prune(&conn);
            Ok((days, rows))
        })
        .await
    };
    mark_worked(&db, &scope_key, now).await;
    let (days, rows) = match written {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return TickOutcome::Failed(e),
        Err(e) => return TickOutcome::Failed(e.to_string()),
    };

    // Running while there is still work left in the horizon, so the page
    // can distinguish "still walking" from "stopped" -- a caveat identical
    // in both cases is #1042's indefinite skeleton at page level.
    let more = uncovered.len() > group.len();
    emit_backfill(app, &db, &scope_key, &from, &to, more).await;
    TickOutcome::Advanced { days, prs: rows }
}

/// Note that a scope was advanced, so the rotation moves on.
///
/// Called on EVERY outcome including failure: a scope whose tick failed
/// must still yield, or one persistently failing scope holds the worker
/// forever and starves every other one.
async fn mark_worked(db: &std::path::Path, scope_key: &str, now: chrono::DateTime<chrono::Utc>) {
    let db = db.to_path_buf();
    let key = scope_key.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Ok(conn) = open_db(&db) {
            if let Err(e) = crate::store::pr_backfill_scope::note_worked(&conn, &key, now) {
                log::warn!("could not record backfill progress: {e}");
            }
        }
    })
    .await;
}

/// Read the ledger and emit what the page should show.
///
/// The coverage is re-read from the database rather than accumulated in
/// memory, so the figure the user sees is the figure that is stored -- a
/// counter kept alongside could drift from it, and the drift would be
/// invisible.
async fn emit_backfill(
    app: &AppHandle,
    db: &std::path::Path,
    scope_key: &str,
    from: &str,
    to: &str,
    running: bool,
) {
    let db = db.to_path_buf();
    let key = scope_key.to_string();
    let (from, to) = (from.to_string(), to.to_string());
    let report = tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db).ok()?;
        let cov = crate::store::pr_slice::coverage(&conn, &key, &from, &to).ok()?;
        let held = crate::store::pr_history::count(&conn, &key, &from, &to).unwrap_or(0);
        let days_total = crate::github::stats::backfill::days_between(&from, &to);
        Some(crate::github::stats::backfill::Report {
            scope_key: key,
            days_covered: cov.days_covered(),
            days_total,
            collected: held,
            // Straight through, `None` and all. Defaulting it here would
            // be the "400 of 0" the design forbids, introduced at the one
            // layer nobody would look at.
            total: cov.total,
            running,
        })
    })
    .await;
    if let Ok(Some(report)) = report {
        crate::commands::emit_stats_backfill(app, &report);
    }
}

#[cfg(test)]
mod tests {
    /// The worst case in the six-day log: 8 fetched of 29 open (#745).
    #[test]
    fn a_short_list_reports_githubs_own_count() {
        assert_eq!(truncation_payload(8, 29), 29);
    }

    /// The regression this function exists for: a recovered poll has to
    /// TAKE BACK the notice, or the frontend keeps rendering the last
    /// count it was given over a list that is now complete.
    #[test]
    fn a_complete_list_clears_the_notice() {
        assert_eq!(truncation_payload(29, 29), 0);
    }

    /// A list longer than the count is still complete, not a negative
    /// shortfall.
    #[test]
    fn more_fetched_than_counted_is_not_a_truncation() {
        assert_eq!(truncation_payload(30, 29), 0);
    }

    /// The consequence of classifying a parse failure as transient: one
    /// stays quiet, two in a row is still named.
    ///
    /// A `Timeout` stands in for the parse failure because octocrab's
    /// `Serde` variant has no public constructor -- both are transient,
    /// and `should_surface` reads only that property, so this asserts
    /// the exact rule that governs the reported case. The
    /// classification itself is tested against a real truncated
    /// response in `github::client`.
    #[test]
    fn a_transient_failure_is_named_only_when_it_repeats() {
        let e = ClientError::Timeout(90);
        assert!(e.is_transient());
        assert!(!should_surface(&e, 1), "one blip is weather");
        assert!(
            should_surface(&e, FAILURES_BEFORE_BANNER),
            "two in a row is a problem"
        );
    }

    /// Defaults must reproduce the behaviour from before this setting
    /// existed: nothing hidden, and close hides to the tray. An upgrade
    /// must not change what the app does for someone who never opens
    /// Settings -- and a close button that suddenly QUITS loses more
    /// than one that hides.
    #[test]
    fn ui_prefs_default_to_the_previous_behaviour() {
        let d = UiPrefs::default();
        assert!(d.hidden_views.is_empty());
        assert!(d.close_hides_to_tray);
    }

    /// A settings row written BEFORE `announce_updates` existed must
    /// still deserialise, and must not silently reset the preferences
    /// stored alongside it.
    ///
    /// Without `serde(default)` the whole struct fails to parse, the
    /// read falls back to `Default`, and a user who had hidden Docker
    /// and turned off close-to-tray quietly gets both back.
    #[test]
    fn prefs_stored_before_this_field_existed_still_load() {
        let old = r#"{"hidden_views":["docker"],"close_hides_to_tray":false}"#;
        let p: UiPrefs = serde_json::from_str(old).expect("old rows must still parse");
        assert_eq!(p.hidden_views, vec!["docker"]);
        assert!(!p.close_hides_to_tray, "the stored choice must survive");
        assert!(p.announce_updates, "a missing field takes the default");
    }

    /// Hidden views are a list of ids, not a bool per view, so a build
    /// that does not know an id simply carries it -- no migration, and
    /// no crash on a value written by a newer version.
    #[test]
    fn an_unknown_hidden_view_id_is_carried_not_rejected() {
        let json =
            r#"{"hidden_views":["docker","a-view-from-the-future"],"close_hides_to_tray":false}"#;
        let p: UiPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(p.hidden_views.len(), 2);
        assert!(!p.close_hides_to_tray);
    }

    /// The default must be everything ON. This is the upgrade path: a
    /// user who has never opened Settings had notifications before this
    /// key existed, and must still have them after.
    #[test]
    fn notifications_default_to_on() {
        let d = NotifyPrefs::default();
        assert!(d.enabled && d.ci_failed && d.conflicted);
        assert!(d.wants(BreakageKind::CiFailed));
        assert!(d.wants(BreakageKind::Conflicted));
    }

    /// The master switch silences everything without discarding the
    /// per-kind choices underneath it, so turning notifications back on
    /// restores what the user picked rather than a reset.
    #[test]
    fn the_master_switch_silences_every_kind() {
        let p = NotifyPrefs {
            enabled: false,
            ..Default::default()
        };
        assert!(!p.wants(BreakageKind::CiFailed));
        assert!(!p.wants(BreakageKind::Conflicted));
        // Good news is silenced by the master switch too.
        assert!(!p.wants(BreakageKind::ReadyToReview));
        // The choices survive: flipping `enabled` back is enough.
        assert!(p.ci_failed && p.conflicted && p.ready_to_review);
    }

    /// Turning one kind off must not touch the other. This is the whole
    /// point of the per-kind split -- "stop telling me about conflicts"
    /// is a different request from "stop telling me anything".
    #[test]
    fn kinds_are_silenced_independently() {
        let no_conflicts = NotifyPrefs {
            conflicted: false,
            ..Default::default()
        };
        assert!(no_conflicts.wants(BreakageKind::CiFailed));
        assert!(!no_conflicts.wants(BreakageKind::Conflicted));
        assert!(no_conflicts.wants(BreakageKind::ReadyToReview));

        let no_ci = NotifyPrefs {
            ci_failed: false,
            ..Default::default()
        };
        assert!(!no_ci.wants(BreakageKind::CiFailed));
        assert!(no_ci.wants(BreakageKind::Conflicted));

        // And the new kind is independent of both.
        let no_ready = NotifyPrefs {
            ready_to_review: false,
            ..Default::default()
        };
        assert!(!no_ready.wants(BreakageKind::ReadyToReview));
        assert!(no_ready.wants(BreakageKind::CiFailed));
    }

    /// The kind drives the filter; the prose is only display. Asserting
    /// both here is what stops a reworded sentence from silently
    /// changing which notifications a user receives.
    #[test]
    fn kind_carries_the_wording_but_the_filter_matches_the_kind() {
        assert_eq!(BreakageKind::CiFailed.reason(), "CI is failing");
        assert_eq!(BreakageKind::Conflicted.reason(), "has merge conflicts");
        assert_ne!(BreakageKind::CiFailed, BreakageKind::Conflicted);
    }

    use super::*;

    /// The reported bug. A single transport blip painted a red banner
    /// that stayed for a full poll interval -- for something that fixed
    /// itself on the next tick.
    ///
    /// Real numbers from the log that prompted this: 5 failures in 164
    /// polls, every one followed immediately by a success.
    #[test]
    fn one_transient_failure_does_not_surface() {
        let e = ClientError::Timeout(90);
        assert!(e.is_transient());
        assert!(!should_surface(&e, 1), "one blip must stay quiet");
    }

    /// A suppressed transient failure must NOT end the tick as "idle".
    ///
    /// The bar reads `poll-state` as the whole truth when no `poll-error` and
    /// no `prs-updated` accompany it, so an "idle" here renders a green
    /// "Up to date" dot over rows that failed to refresh (#1104).
    ///
    /// Asserted against the tick's own decision function, not a mocked hook:
    /// `StatusBar.test.tsx` sets `state.current = "retrying"` directly and so
    /// passed against the broken code for the whole time it was broken.
    #[test]
    fn a_suppressed_failure_does_not_end_the_tick_idle() {
        assert_eq!(tick_state(Some(false)), "retrying");
    }

    /// A surfaced failure DOES end the tick idle: `poll-error` is already on
    /// screen, and a second simultaneous signal would have the bar competing
    /// with its own banner.
    #[test]
    fn a_surfaced_failure_leaves_the_banner_to_speak() {
        assert_eq!(tick_state(Some(true)), "idle");
    }

    /// A successful tick is genuinely idle -- the guard above must not be so
    /// broad that the bar can never return to green.
    #[test]
    fn a_successful_tick_is_idle() {
        assert_eq!(tick_state(None), "idle");
    }

    /// But a real outage must not be hidden. Two in a row stops being
    /// weather.
    #[test]
    fn repeated_transient_failures_do_surface() {
        let e = ClientError::Timeout(90);
        assert!(should_surface(&e, FAILURES_BEFORE_BANNER));
        assert!(should_surface(&e, FAILURES_BEFORE_BANNER + 5));
    }

    /// Waiting cannot fix a rate limit or a malformed query, so those
    /// surface on the first failure -- the next tick fails identically.
    #[test]
    fn actionable_failures_surface_immediately() {
        for e in [
            ClientError::RateLimited("resets in 12m".into()),
            ClientError::Graphql("field 'nope' does not exist".into()),
            ClientError::Join("task panicked".into()),
        ] {
            assert!(!e.is_transient(), "{e} should not be transient");
            assert!(
                should_surface(&e, 1),
                "{e} must surface on the first failure"
            );
        }
    }

    /// The counter must reset on success, or a machine that blips once
    /// an hour would eventually cross the threshold and show a banner
    /// for a network that is fine. Models the loop's own sequence, since
    /// the counter lives in a spawned task the tests cannot reach.
    #[test]
    fn a_success_resets_the_failure_count() {
        let e = ClientError::Timeout(90);
        let mut consecutive: u32 = 0;

        // blip, recover, blip, recover -- the pattern from the real log.
        for _ in 0..5 {
            consecutive += 1;
            assert!(
                !should_surface(&e, consecutive),
                "an isolated blip must never surface"
            );
            consecutive = 0; // the success arm
        }

        // Two back to back, with no success between them, does surface.
        consecutive += 1;
        assert!(!should_surface(&e, consecutive));
        consecutive += 1;
        assert!(should_surface(&e, consecutive));
    }

    /// A timeout is transient by definition: the request was still in
    /// flight when the ceiling hit, which says nothing about whether the
    /// next one will succeed.
    #[test]
    fn a_timeout_is_transient() {
        assert!(ClientError::Timeout(90).is_transient());
    }

    /// A poll must give up before its successor starts -- the whole TICK,
    /// not one fetch of it.
    ///
    /// The regression this guards: `FETCH_TIMEOUT` was 90s while
    /// `MIN_FOCUSED_SECS` is 60, so a user on the fastest allowed
    /// interval could have a hung fetch still in flight when the next
    /// tick fired -- two concurrent fetches, both spending rate-limit
    /// budget, for a request that had already been useless for a full
    /// interval.
    ///
    /// # What this test MISSED, and why it is rewritten (#844)
    ///
    /// It asserted `FETCH_TIMEOUT < MIN_FOCUSED_SECS` -- `30s < 60s`, true
    /// -- on the premise that a tick is one fetch. A tick awaits TWO
    /// sequential 30s timeouts (`:868` and `:884`), so the worst case was
    /// 60s: exactly equal to the floor, which reintroduces the very overlap
    /// the test was written to prevent. The assertion passed while the
    /// property failed, which is the shape of every defect in #842 and #847
    /// as well.
    ///
    /// Asserted against `TICK_TIMEOUT` and against the arithmetic that
    /// enforces it, rather than against a pair of constants plus a belief
    /// about how many fetches there are. `remaining_tick_budget` is the
    /// thing the loop actually calls, so this measures the shipped
    /// behaviour -- the property `a_tick_cannot_outlast_its_own_cadence`
    /// below pins end to end.
    #[test]
    fn the_fetch_ceiling_is_under_the_shortest_poll_interval() {
        // The TICK is what must clear the next tick.
        assert!(
            TICK_TIMEOUT < Duration::from_secs(MIN_FOCUSED_SECS),
            "a tick may not outlive the gap to the next poll: \
             TICK_TIMEOUT={TICK_TIMEOUT:?} MIN_FOCUSED_SECS={MIN_FOCUSED_SECS}"
        );
        // And one fetch must still fit inside a tick, or the first fetch
        // alone could exhaust the budget and the second would always skip.
        assert!(
            FETCH_TIMEOUT < TICK_TIMEOUT,
            "the first fetch must leave the second some budget: \
             FETCH_TIMEOUT={FETCH_TIMEOUT:?} TICK_TIMEOUT={TICK_TIMEOUT:?}"
        );
    }

    /// The two sequential fetches, together, cannot outlast a tick.
    ///
    /// This is the property the old single-constant assertion could not
    /// state. Driven through `remaining_tick_budget`, which is what the loop
    /// calls, so it holds for the code that ships rather than for a
    /// restatement of it.
    ///
    /// Includes the pathological case the loop has to handle: a first fetch
    /// that ran to its own ceiling leaves 15s, not 30, so the pair is 45s
    /// and not the 60s two independent ceilings allowed.
    #[test]
    fn a_tick_cannot_outlast_its_own_cadence() {
        let floor = Duration::from_secs(MIN_FOCUSED_SECS);
        // Every point at which the first fetch could finish, including
        // running to its own ceiling and past the whole tick budget.
        for spent_secs in [0, 1, 10, 29, 30, 44, 45, 60, 600] {
            let spent = Duration::from_secs(spent_secs);
            let first = spent.min(FETCH_TIMEOUT);
            let worst = first + remaining_tick_budget(spent);
            assert!(
                worst <= TICK_TIMEOUT,
                "a tick whose first fetch took {spent:?} could run {worst:?}, \
                 past TICK_TIMEOUT={TICK_TIMEOUT:?}"
            );
            assert!(
                worst < floor,
                "a tick whose first fetch took {spent:?} could run {worst:?} and \
                 overlap the next tick at {floor:?}"
            );
        }
        // A first fetch at its full ceiling does NOT hand the second
        // another full one. Pinned as a number because it is the case the
        // old guard got wrong: 30 + 30 = 60, not 30 + 15 = 45.
        assert_eq!(
            remaining_tick_budget(FETCH_TIMEOUT),
            Duration::from_secs(15),
            "a hung first fetch must shrink the second's ceiling, not reset it"
        );
        // Overrunning the budget yields zero rather than wrapping, which the
        // loop reads as "skip": a request with no time to answer still
        // spends a rate-limit point.
        assert!(remaining_tick_budget(Duration::from_secs(600)).is_zero());
        // A fast first fetch does not hand the second a LONGER ceiling than
        // the first had, or a hung notification fetch becomes the thing that
        // overruns the tick.
        assert_eq!(remaining_tick_budget(Duration::ZERO), FETCH_TIMEOUT);
    }

    /// The ceiling must still clear the measured p90, or ordinary slow
    /// polls would be cut off and the list would stop updating for
    /// users whose accounts are merely large.
    ///
    /// p90 is 8,814ms per POST across 3,806 requests in a reported
    /// six-day session; p99 is 30,126ms. 30s keeps better than 3x
    /// headroom over p90 while abandoning roughly the slowest 1%.
    #[test]
    fn the_fetch_ceiling_still_clears_the_measured_p90() {
        const MEASURED_P90: Duration = Duration::from_millis(8_814);
        assert!(
            FETCH_TIMEOUT >= MEASURED_P90 * 3,
            "the ceiling must not cut off ordinary slow polls: \
             FETCH_TIMEOUT={FETCH_TIMEOUT:?}"
        );
    }
    use crate::github::model::MergeStateStatus;

    /// `notify_one` stores a permit when nobody is waiting, so a refresh
    /// requested WHILE a fetch is in flight is not lost -- the next
    /// `notified()` returns immediately instead of waiting out a full
    /// interval. Without that property, a tray click landing mid-tick
    /// would appear to do nothing for up to 300s.
    #[tokio::test]
    async fn a_wake_requested_before_the_wait_is_not_lost() {
        let n = Notify::new();
        n.notify_one(); // arrives while the loop is busy fetching
        let waited = tokio::time::timeout(Duration::from_millis(50), n.notified()).await;
        assert!(waited.is_ok(), "a stored permit must satisfy the next wait");
    }

    /// And the select really does prefer whichever fires first, so a wake
    /// short-circuits the cadence rather than being queued behind it.
    #[tokio::test]
    async fn a_wake_short_circuits_the_sleep() {
        let n = Arc::new(Notify::new());
        let n2 = n.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            n2.notify_one();
        });
        let start = std::time::Instant::now();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => panic!("slept instead of waking"),
            _ = n.notified() => {}
        }
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    /// A hung request must collapse into the error arm rather than
    /// awaiting forever. Without the `timeout` wrapper the poll loop had
    /// no ceiling at all: a blackholed TCP connection meant no error, no
    /// next tick, and silent stale data for the rest of the session.
    #[tokio::test]
    async fn a_hung_fetch_times_out_instead_of_awaiting_forever() {
        // A future that never resolves, standing in for a wedged socket.
        // Uses a short ceiling so the test is instant; the mechanism is
        // identical to the one FETCH_TIMEOUT drives in `spawn`.
        let hung = std::future::pending::<Result<Vec<PullRequest>, ClientError>>();
        let out = tokio::time::timeout(Duration::from_millis(20), hung).await;
        assert!(out.is_err(), "a never-resolving fetch must time out");

        // And the timeout maps into the error arm the loop already handles.
        let mapped: Result<Vec<PullRequest>, ClientError> = match out {
            Ok(res) => res,
            Err(_) => Err(ClientError::Timeout(FETCH_TIMEOUT.as_secs())),
        };
        // Against the CONSTANT, not a literal: this test is about the
        // mapping into the error arm, not about what the ceiling happens
        // to be, and hardcoding the number made #744's change to it fail
        // here for no reason.
        let secs = FETCH_TIMEOUT.as_secs();
        assert!(matches!(mapped, Err(ClientError::Timeout(s)) if s == secs));
    }

    /// The ceiling has to clear a normal fetch by a wide margin, or a
    /// merely slow response would be reported as a failure.
    ///
    /// The no-overlap half of this used to read `FETCH_TIMEOUT < FOCUSED
    /// + FOCUSED` -- 240s, which the old 90s ceiling satisfied
    /// comfortably while still being able to outlive a 60s interval.
    /// That is why the overlap went unnoticed; the real bound is
    /// `MIN_FOCUSED_SECS`, and it is asserted in
    /// `the_fetch_ceiling_is_under_the_shortest_poll_interval` above.
    #[test]
    fn fetch_timeout_leaves_headroom_over_a_normal_fetch() {
        // PRS_QUERY's own doc records ~2.9s for 27 PRs.
        assert!(FETCH_TIMEOUT >= Duration::from_secs(30));
    }
    use crate::github::model::{CiState, Label, ReviewState};
    use chrono::Utc;

    fn pr(repo: &str, number: u64, merge: MergeState) -> PullRequest {
        PullRequest {
            id: "PR_test".into(),
            number,
            title: "Add retry to the fetch client".into(),
            url: format!("https://github.com/{repo}/pull/{number}"),
            repo: repo.into(),
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            base_ref: "main".into(),
            author: "octocat".into(),
            is_draft: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ci: CiState::Success,
            merge,
            merge_status: MergeStateStatus::Clean,
            review: ReviewState::Approved,
            in_merge_queue: false,
            labels: Vec::<Label>::new(),
            comment_count: 0,
            unresolved_threads: 0,
            requested_reviewers: Vec::new(),
            assignees: Vec::new(),
            latest_reviews: Vec::new(),
            // Zero totals against empty lists: nothing was cut.
            requested_reviewers_total: 0,
            assignees_total: 0,
            latest_reviews_total: 0,
            labels_total: 0,
        }
    }

    #[test]
    fn polls_faster_when_focused() {
        assert_eq!(
            interval_for(true),
            Duration::from_secs(DEFAULT_FOCUSED_SECS)
        );
        assert_eq!(
            interval_for(false),
            Duration::from_secs(DEFAULT_FOCUSED_SECS * BACKGROUND_MULTIPLIER)
        );
    }

    #[test]
    fn the_default_cadence_is_two_minutes() {
        assert_eq!(DEFAULT_FOCUSED_SECS, 120);
        assert_eq!(interval_for(true), Duration::from_secs(120));
    }

    #[test]
    fn a_configured_interval_is_honoured() {
        assert_eq!(interval_for_secs(true, 300), Duration::from_secs(300));
        // Background stays proportional.
        assert_eq!(
            interval_for_secs(false, 300),
            Duration::from_secs(300 * BACKGROUND_MULTIPLIER)
        );
    }

    /// A Tauri command is a public surface. Without a floor, `0` would
    /// busy-loop against GitHub; without a ceiling, a huge value would
    /// effectively stop polling while the UI still claimed to be live.
    #[test]
    fn a_configured_interval_is_clamped() {
        assert_eq!(clamp_interval(0), MIN_FOCUSED_SECS);
        assert_eq!(clamp_interval(1), MIN_FOCUSED_SECS);
        assert_eq!(clamp_interval(u64::MAX), MAX_FOCUSED_SECS);
        assert_eq!(clamp_interval(120), 120, "a normal value passes through");
        assert_eq!(
            interval_for_secs(true, 0),
            Duration::from_secs(MIN_FOCUSED_SECS)
        );
    }

    /// A view that shows no PR data polls at the BACKGROUND rate even
    /// when the window is focused -- but still polls, so the tray badge
    /// does not go stale while the user cleans up worktrees.
    #[test]
    fn a_non_github_view_uses_the_background_cadence() {
        let secs = 120;
        // Read through atomics so the `focused && needs_github` the loop
        // actually evaluates cannot be constant-folded away.
        let focused = AtomicBool::new(true);
        let needs_github = AtomicBool::new(true);
        let effective = || {
            interval_for_secs(
                focused.load(Ordering::Relaxed) && needs_github.load(Ordering::Relaxed),
                secs,
            )
        };

        let fast = effective();
        assert_eq!(fast, Duration::from_secs(secs));

        // Focused, but on a view that shows no PR data.
        needs_github.store(false, Ordering::Relaxed);
        let on_worktrees = effective();

        // Same as being unfocused...
        focused.store(false, Ordering::Relaxed);
        needs_github.store(true, Ordering::Relaxed);
        assert_eq!(on_worktrees, effective());

        // ...slower than focused, and never stopped: the tray badge must
        // not go stale while the window sits on another view.
        assert!(on_worktrees > fast);
        assert!(on_worktrees.as_secs() > 0, "must not stop");
    }

    /// Budget guard for BOTH cadences, against the MEASURED cost of the
    /// shipped query and a deny-list that refuses the connections nobody
    /// has priced.
    ///
    /// # Why a deny-list replaced the count (#842)
    ///
    /// The previous version summed occurrences of eight connection names and
    /// asserted the total was 7. Its own comment recorded the failure mode
    /// twice -- "a NESTED connection is invisible to a substring count of its
    /// parent, so a new connection has to be listed here BY NAME" -- and then
    /// shipped a third instance of it: `commits(` was never on the list, and
    /// `commits(last: 1)` is the connection that WRAPS the entire
    /// check-status subtree in `query.rs`.
    ///
    /// So the logic is inverted, copying `stats/query.rs:604-620`: split at
    /// the node selection and DENY every paged connection that has not been
    /// explicitly approved. "Count the ones I remembered" cannot catch the
    /// next addition; "refuse the ones I have not approved" can, because the
    /// next addition is by definition not on the approved list.
    ///
    /// # The numbers, re-measured
    ///
    /// Both old numbers were stale: the assertion said `cost == 7` and the
    /// failure message said "7 connections = 4 points", against a shipped
    /// query that costs **2**.
    ///
    /// #842 records a disagreement about whether `commits(` is one of those
    /// two points -- the auditor measured yes, the issue's own author could
    /// not reproduce it from hand-built approximations. I settled it with the
    /// auditor's method: the document extracted VERBATIM from `PRS_QUERY`
    /// with its `#` comment lines stripped, `gh api graphql -F first=25`,
    /// three runs per row, 2026-09-11, against `is:pr is:open author:@me`:
    ///
    /// | Document                            | Cost | Wall clock  |
    /// |-------------------------------------|------|-------------|
    /// | shipped, verbatim                   | **2**| 2.31-2.56s  |
    /// | same, `commits(last: 1)` block gone | **1**| 2.04-2.26s  |
    /// | same, `reviewThreads(` block gone   | 2    | 2.18-2.56s  |
    /// | same, inner `contexts(` block gone  | 2    | 2.68-3.34s  |
    ///
    /// The auditor was right and the reproduction attempt was not: removing
    /// `commits(` halves the cost, while removing either of the two sibling
    /// paged connections leaves it at 2. So the marginal point is `commits(`
    /// specifically, not "whichever connection is dropped last" -- which is
    /// the hypothesis the hand-built approximations could not distinguish.
    /// `commits(` is **1 of 2 points, 50% of per-poll spend**, and the old
    /// guard could not see it.
    ///
    /// # Per TICK, not per query
    ///
    /// A tick issues TWO searches (`poll.rs:835` and `:851`), and each is its
    /// own request at its own cost. Measured the same day, same method:
    /// `is:pr is:open author:@me` cost 2 in 2.31-2.56s, `is:pr is:open
    /// review-requested:@me` cost 2 in 0.84-0.99s. So a tick costs **4**
    /// points with ready-to-review on, which is the default.
    ///
    /// The budget arithmetic below is therefore against 4, not 2. At the
    /// FLOOR cadence that is 240/hr of 5,000 -- which is why this is an
    /// accuracy defect rather than a budget risk, and why the old 7 was
    /// never unsafe, only wrong.
    #[test]
    fn both_cadences_stay_well_inside_the_rate_limit() {
        let q = crate::github::query::PRS_QUERY;
        assert!(
            q.contains("search("),
            "PRS_QUERY must contain at least one search"
        );
        // ONE search per document. The query carried two aliases and cost 6
        // until they were split; a second alias here would double every
        // caller's spend, which is the change that split them.
        assert_eq!(
            q.matches("search(").count(),
            1,
            "PRS_QUERY is ONE search per request; a second alias makes every \
             caller pay for both"
        );

        // The NODE SELECTION, which is where a per-PR field is added. The
        // outer `search(` and the document's own `first:` are not nested
        // connections and are excluded by splitting here --
        // `stats/query.rs:604-620`'s rule, and the reason that guard works
        // where a whole-document substring count does not.
        let nodes = q
            .split_once("... on PullRequest {")
            .expect("the node selection")
            .1;

        // APPROVED paged connections: each one is in the shipped document,
        // has been measured, and its cost is accounted for in MEASURED_COST
        // below. Anything else nested here is REFUSED until somebody
        // measures it -- which is the whole inversion #842 asks for.
        const APPROVED: [&str; 7] = [
            "assignees(",
            "reviewRequests(",
            "latestReviews(",
            "labels(",
            "reviewThreads(",
            // The connection the old count-based guard could not see, and
            // the one that costs a point: 2 -> 1 when it is removed.
            "commits(",
            // Nested INSIDE `commits(`, which is exactly why a substring
            // count of the parent was blind to it (#312). Free on top of
            // its parent: removing it alone leaves the cost at 2.
            "contexts(",
        ];
        // Every `name(` in the node selection: a paged connection is one
        // taking an argument, which is the shape GitHub prices
        // (`stats/query.rs:575-600` measured `reviews { totalCount }` at 1
        // point and `reviews(first: 1) { totalCount }` at 2). Scanning for
        // the SHAPE rather than for known names is what makes this a
        // deny-list instead of another allow-list with a blind spot.
        let mut rest = nodes;
        while let Some(i) = rest.find('(') {
            // Walk back over the identifier immediately before the paren.
            let head = &rest[..i];
            let name_start = head
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |p| p + 1);
            let name = &head[name_start..];
            rest = &rest[i + 1..];
            if name.is_empty() {
                continue;
            }
            let paged = format!("{name}(");
            // `... on User {` style fragments and GraphQL directives have no
            // identifier before the paren and are skipped above. A bare
            // `repository(` WOULD be caught, correctly: it is free as an
            // object and priced as a page.
            assert!(
                APPROVED.contains(&paged.as_str()),
                "`{paged}` is a paged nested connection in PRS_QUERY's node \
                 selection and is NOT on the approved list. GitHub prices the \
                 `first:`/`last:` ARGUMENT, so this may have changed what every \
                 poll costs. Measure the LIVE cost -- extract the document, \
                 strip the `#` comment lines, run it with `rateLimit {{ cost }}` \
                 -- then update MEASURED_COST and add the name here. \
                 (`commits(` was missing from the previous guard and is 1 of the \
                 2 points: measured 2 -> 1 with it removed, 2026-09-11.)"
            );
        }
        // And the approved connections must still BE there: a deny-list
        // refuses additions but cannot notice a removal, and removing one
        // would make MEASURED_COST an overstatement.
        for c in APPROVED {
            assert!(
                nodes.contains(c),
                "`{c}` is approved and measured but no longer in the document; \
                 re-measure before lowering MEASURED_COST"
            );
        }

        /// MEASURED live 2026-09-11, `gh api graphql -F first=25`, the
        /// document extracted verbatim from `PRS_QUERY` with `#` comment
        /// lines stripped: **cost 2**, 3 runs (2.31s, 2.32s, 2.56s).
        ///
        /// RE-MEASURED 2026-09-16 for #1089, same method, after adding
        /// `totalCount` to `assignees`, `reviewRequests`, `latestReviews`
        /// and `labels` and raising `latestReviews` from 5 to 20: still
        /// **cost 2**, 3 runs (2.47s, 2.63s, 2.68s), and 2 again against
        /// `repo:kubernetes/kubernetes` before and after. So this figure
        /// stands rather than being carried forward on faith.
        ///
        /// Why it did not move, which is the reusable part: GitHub prices
        /// the `first:` ARGUMENT, not the field or the page size
        /// (`stats/query.rs:694-720` measured `reviews { totalCount }` at
        /// 1 point and `reviews(first: 1) { totalCount }` at 2). All four
        /// connections were paged already, so `totalCount` rides along on
        /// something being paid for either way, and widening a window
        /// that already exists changes nothing.
        ///
        /// Not a count of anything. The previous guard's number was a count
        /// of connection appearances asserted to be 7, which had drifted
        /// three separate times from a live cost that was 2 -- so this is
        /// the measurement itself, with the method recorded beside it so the
        /// next reader can repeat it rather than trust it.
        const MEASURED_COST: u64 = 2;
        /// Searches per TICK. `poll.rs:835` fetches the authored list and
        /// `:851` the review queue, sequentially, each its own request.
        ///
        /// Measured the same day and the same way: authored cost 2 in
        /// 2.31-2.56s, review-requested cost 2 in 0.84-0.99s. The second is
        /// issued only when the ready-to-review notification is on, which is
        /// the DEFAULT -- so 2 is the cadence docs' real shape, and reasoning
        /// from one fetch understated every figure by half.
        const SEARCHES_PER_TICK: u64 = 2;
        let cost = MEASURED_COST * SEARCHES_PER_TICK;

        // The FLOOR, not the default: a user picking the fastest allowed
        // setting must still be inside budget, or this guard only protects
        // people who never touch the setting.
        //
        // At the floor that is 60 ticks/hr x 4 = 240 points of 5,000. The
        // old assertion reasoned from 7 and passed for the wrong reason;
        // this one reasons from 4 and passes for the right one.
        for focused in [true, false] {
            let per_hour = 3600 / interval_for_secs(focused, MIN_FOCUSED_SECS).as_secs();
            let points = per_hour * cost;
            assert!(
                points < 500,
                "{} polling would spend {points}/hr of a 5000 budget",
                if focused { "focused" } else { "background" }
            );
        }
    }

    /// #22's recheck delay is a single one-shot query, not a recurring
    /// cadence -- guard it the same way the regular cadence test guards
    /// FOCUSED/BACKGROUND, so a future change (e.g. a mistaken retry loop)
    /// that shrank this toward "every few seconds" fails CI instead of
    /// silently turning into a retry storm against the rate limit.
    #[test]
    fn recheck_delay_is_a_single_short_one_shot_not_a_tight_polling_cadence() {
        assert!(
            RECHECK_DELAY < FOCUSED,
            "recheck should fire before the next regular tick"
        );
        assert!(
            RECHECK_DELAY >= Duration::from_secs(1),
            "recheck delay too aggressive for a one-shot"
        );
    }

    #[test]
    fn has_checking_detects_a_pr_still_being_computed() {
        let prs = vec![
            pr("octocat/hello-world", 1, MergeState::Mergeable),
            pr("octocat/hello-world", 2, MergeState::Checking),
        ];
        assert!(has_checking(&prs));
    }

    #[test]
    fn has_checking_is_false_once_everything_resolved() {
        let prs = vec![
            pr("octocat/hello-world", 1, MergeState::Mergeable),
            pr("octocat/spoon-knife", 7, MergeState::Conflicted),
        ];
        assert!(!has_checking(&prs));
    }

    /// #436: a notification when a pull request enters the green
    /// "Ready for review" panel -- so it can be picked up immediately.
    mod ready {
        use super::*;

        fn ready(number: u64) -> PullRequest {
            pr_full("o/r", number, CiState::Success, MergeState::Mergeable)
        }

        /// The case that makes this DIFFERENT from `newly_broken`.
        ///
        /// That function never fires for a pull request absent from
        /// `previous`. Here, a brand-new PR arriving already green with
        /// the user as reviewer is exactly what is worth announcing --
        /// and it is always absent from the previous tick.
        #[test]
        fn a_brand_new_ready_pull_request_notifies() {
            let out = newly_ready(&[], &[ready(1)]);
            assert_eq!(out.len(), 1, "an unseen ready PR must notify");
            assert_eq!(out[0].kind, BreakageKind::ReadyToReview);
        }

        /// And it must not re-announce on every tick afterwards.
        #[test]
        fn a_pull_request_already_ready_does_not_notify_again() {
            let before = vec![ready(1)];
            assert!(newly_ready(&before, &[ready(1)]).is_empty());
        }

        /// Going green is the transition, not merely being green.
        #[test]
        fn turning_green_notifies() {
            let before = vec![pr_full("o/r", 1, CiState::Failure, MergeState::Mergeable)];
            assert_eq!(newly_ready(&before, &[ready(1)]).len(), 1);
        }

        #[test]
        fn a_draft_is_not_ready() {
            let mut d = ready(1);
            d.is_draft = true;
            assert!(newly_ready(&[], &[d]).is_empty());
        }

        /// Pending is not ready: the checks have not passed, they merely
        /// have not failed yet.
        #[test]
        fn pending_checks_are_not_ready() {
            let p = pr_full("o/r", 1, CiState::Pending, MergeState::Mergeable);
            assert!(newly_ready(&[], &[p]).is_empty());
        }

        /// A repository with no checks configured has nothing to wait
        /// for -- excluding it would empty the panel for anyone not
        /// running CI.
        #[test]
        fn no_checks_configured_is_ready() {
            let p = pr_full("o/r", 1, CiState::None, MergeState::Mergeable);
            assert_eq!(newly_ready(&[], &[p]).len(), 1);
        }

        #[test]
        fn conflicts_are_not_ready() {
            let p = pr_full("o/r", 1, CiState::Success, MergeState::Conflicted);
            assert!(newly_ready(&[], &[p]).is_empty());
        }

        /// An existing verdict means it is no longer WAITING.
        #[test]
        fn an_already_reviewed_pull_request_is_not_ready() {
            for verdict in [ReviewState::Approved, ReviewState::ChangesRequested] {
                let mut p = ready(1);
                p.review = verdict;
                assert!(newly_ready(&[], &[p]).is_empty(), "{verdict:?}");
            }
        }

        #[test]
        fn a_queued_pull_request_is_not_ready() {
            let mut p = ready(1);
            p.in_merge_queue = true;
            assert!(newly_ready(&[], &[p]).is_empty());
        }
    }

    /// #789: a notification when a pull request APPEARS that was not
    /// there before.
    mod appeared {
        use super::*;

        fn open(number: u64) -> PullRequest {
            pr_full("o/r", number, CiState::Pending, MergeState::Mergeable)
        }

        /// The rule, stated: absent before, present now.
        ///
        /// Unlike `newly_ready` this says nothing about the pull
        /// request's STATE -- a red, conflicted, unreviewable pull
        /// request appearing is still news. That is the whole difference
        /// between "something new exists" and "something became
        /// actionable".
        #[test]
        fn a_pull_request_absent_from_the_previous_tick_has_appeared() {
            let out = newly_appeared(&[open(1)], &[open(1), open(2)]);
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].number, 2);
            assert_eq!(out[0].kind, BreakageKind::Appeared);
            assert_eq!(out[0].kind.reason(), "just appeared");
        }

        /// A red, conflicted pull request appearing is still an
        /// appearance. If this test fails, the rule has quietly acquired
        /// a readiness clause it should not have.
        #[test]
        fn a_broken_pull_request_appearing_is_still_news() {
            let broken = pr_full("o/r", 9, CiState::Failure, MergeState::Conflicted);
            let out = newly_appeared(&[], &[broken]);
            assert_eq!(out.len(), 1, "{out:?}");
        }

        /// An unchanged list announces nothing, however many ticks pass.
        #[test]
        fn an_unchanged_list_announces_nothing() {
            let list = vec![open(1), open(2)];
            assert!(newly_appeared(&list, &list).is_empty());
        }

        /// A pull request that DISAPPEARED -- merged, closed -- is not an
        /// appearance. Only one direction is in scope.
        #[test]
        fn a_closed_pull_request_announces_nothing() {
            assert!(newly_appeared(&[open(1), open(2)], &[open(1)]).is_empty());
        }

        /// The identity is `(repo, number)`, so the same number in two
        /// repositories is two pull requests.
        #[test]
        fn the_identity_is_the_repo_and_the_number() {
            let a = pr_full(
                "octocat/hello-world",
                7,
                CiState::Success,
                MergeState::Mergeable,
            );
            let b = pr_full(
                "octocat/spoon-knife",
                7,
                CiState::Success,
                MergeState::Mergeable,
            );
            let out = newly_appeared(std::slice::from_ref(&a), &[a.clone(), b]);
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].repo, "octocat/spoon-knife");
        }

        /// A draft appearing is not news: its author has explicitly said
        /// it is not ready to be looked at.
        #[test]
        fn a_new_draft_is_not_announced() {
            let mut d = open(2);
            d.is_draft = true;
            assert!(newly_appeared(&[open(1)], &[open(1), d]).is_empty());
        }

        /// **The first-tick trap, stated as a test of the FUNCTION's
        /// contract rather than the loop's.**
        ///
        /// With an empty `previous` this function announces the whole
        /// list, by design -- and that is exactly why the caller must
        /// gate it on having had a tick. A future refactor that removed
        /// the caller's guard would make this the launch burst, so the
        /// behaviour is pinned here with the reason attached.
        #[test]
        fn an_empty_previous_list_announces_everything_which_is_why_the_caller_gates_it() {
            let out = newly_appeared(&[], &[open(1), open(2), open(3)]);
            assert_eq!(
                out.len(),
                3,
                "the suppression lives in the poll loop's `had_a_tick`, not here"
            );
        }

        /// And the `NotifyPrefs` category gates it.
        #[test]
        fn the_new_pr_category_gates_the_notification() {
            let off = NotifyPrefs {
                new_pr: false,
                ..Default::default()
            };
            assert!(!off.wants(BreakageKind::Appeared));
            assert!(
                off.wants(BreakageKind::CiFailed),
                "turning one category off must not touch another"
            );
            assert!(NotifyPrefs::default().wants(BreakageKind::Appeared));
        }
    }

    /// #789: the health categories, which until now had none -- the
    /// battery alerts notified unconditionally.
    mod health_categories {
        use super::*;

        /// The keys are the ones `health::alerts` and `health::runaway`
        /// actually produce. Asserted against their own `key()` rather
        /// than transcribed, so a rename there fails here instead of
        /// silently moving a condition into the "unknown" bucket.
        #[test]
        fn the_keys_match_the_modules_that_produce_them() {
            use crate::health::alerts::Alert as Battery;
            use crate::health::runaway::Alert as Cpu;
            let d = NotifyPrefs::default();
            for key in [
                Battery::Low {
                    percent: 10.0,
                    threshold: 25,
                }
                .key(),
                Battery::FastDischarge {
                    percent_per_min: 1.0,
                }
                .key(),
                Battery::DrainingOnAc {
                    percent_per_min: 1.0,
                }
                .key(),
                Cpu::DiffuseCpu {
                    percent: 70.0,
                    minutes: 20.0,
                    process_count: 900,
                }
                .key(),
            ] {
                assert!(d.wants_health(key), "{key} is not wanted by default");
            }

            let no_battery = NotifyPrefs {
                health_battery: false,
                ..Default::default()
            };
            assert!(!no_battery.wants_health(
                Battery::Low {
                    percent: 10.0,
                    threshold: 25
                }
                .key()
            ));
            assert!(no_battery.wants_health(
                Cpu::DiffuseCpu {
                    percent: 70.0,
                    minutes: 20.0,
                    process_count: 900
                }
                .key()
            ));
        }

        /// All three battery conditions are ONE category: they are one
        /// subject to a person.
        #[test]
        fn the_three_battery_conditions_are_one_category() {
            let off = NotifyPrefs {
                health_battery: false,
                ..Default::default()
            };
            for key in ["low", "fast_discharge", "draining_on_ac"] {
                assert!(!off.wants_health(key), "{key}");
            }
        }

        #[test]
        fn the_master_switch_silences_health_too() {
            let off = NotifyPrefs {
                enabled: false,
                ..Default::default()
            };
            assert!(!off.wants_health("low"));
            assert!(!off.wants_health("diffuse_cpu"));
            assert!(!off.wants_health("something_new"));
            assert!(
                off.health_battery && off.health_cpu,
                "the choices survive the master switch"
            );
        }

        /// A condition a future release adds is allowed through rather
        /// than silently dropped: a category you cannot yet switch off is
        /// a smaller problem than a warning you never received.
        #[test]
        fn an_unknown_condition_is_not_silently_dropped() {
            let off_everything_known = NotifyPrefs {
                health_battery: false,
                health_cpu: false,
                ..Default::default()
            };
            assert!(off_everything_known.wants_health("thermal_critical"));
        }

        /// The upgrade path: a stored preference written before these
        /// fields existed still decodes, and decodes to ON.
        ///
        /// Without `serde(default)` the missing keys would fail the whole
        /// struct, and `unwrap_or_default()` in `read_notify_prefs` would
        /// silently reset every OTHER notification setting the user had
        /// chosen.
        #[test]
        fn a_stored_preference_from_before_these_fields_still_decodes() {
            let stored = r#"{"enabled":true,"ci_failed":false,"conflicted":true}"#;
            let p: NotifyPrefs = serde_json::from_str(stored).unwrap();
            assert!(!p.ci_failed, "the stored choice survives");
            assert!(p.new_pr, "and the new fields default to ON");
            assert!(p.health_battery);
            assert!(p.health_cpu);
            assert!(p.ready_to_review);
        }

        /// #979's field on the same upgrade path, asserted separately
        /// because its default was ARGUED rather than copied.
        ///
        /// Every field above defaults ON because those alerts predate
        /// their own gate and an upgrade must not silence something a
        /// user relies on. That reasoning does not apply to a new alert
        /// nobody relies on yet -- so this one is ON for a different
        /// reason: it cannot fire unless `claude_integrations_enabled` is
        /// on, and that defaults OFF, so nobody can be interrupted about
        /// something they never asked for.
        ///
        /// Sabotage: drop `#[serde(default = "default_true")]` from
        /// `claude_crashed` and this fails on the `from_str`, which is
        /// the failure mode the attribute exists for -- a missing key
        /// failing the WHOLE struct and silently resetting every other
        /// setting.
        #[test]
        fn a_stored_preference_from_before_the_claude_field_still_decodes() {
            let stored = r#"{"enabled":true,"ci_failed":false,"conflicted":true,
                             "ready_to_review":false,"new_pr":true,
                             "health_battery":false,"health_cpu":true}"#;
            let p: NotifyPrefs = serde_json::from_str(stored).unwrap();
            assert!(!p.ci_failed, "every stored choice survives");
            assert!(!p.ready_to_review);
            assert!(!p.health_battery);
            assert!(
                p.claude_crashed,
                "and the new field defaults ON, because the feature it \
                 reports on is itself opt-in and defaults off"
            );
        }

        /// The gate is a real gate: turning it off is expressible and
        /// leaves everything else alone. The pair to the test above --
        /// a default that cannot be turned off is not a preference.
        #[test]
        fn the_claude_alert_can_be_turned_off_on_its_own() {
            let off = NotifyPrefs {
                claude_crashed: false,
                ..Default::default()
            };
            assert!(!off.claude_crashed);
            assert!(
                off.health_battery && off.health_cpu && off.ci_failed,
                "turning one category off must not touch the others"
            );
            let round: NotifyPrefs =
                serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
            assert!(
                !round.claude_crashed,
                "and the choice survives a round trip"
            );
        }
    }

    fn pr_full(repo: &str, number: u64, ci: CiState, merge: MergeState) -> PullRequest {
        let t = chrono::DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        PullRequest {
            id: "PR_test".into(),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            repo: repo.to_string(),
            author: "someone".into(),
            is_draft: false,
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            base_ref: "main".into(),
            created_at: t,
            updated_at: t,
            ci,
            merge,
            merge_status: MergeStateStatus::Clean,
            review: crate::github::model::ReviewState::None,
            in_merge_queue: false,
            labels: vec![],
            comment_count: 0,
            unresolved_threads: 0,
            requested_reviewers: Vec::new(),
            assignees: Vec::new(),
            latest_reviews: Vec::new(),
            // Zero totals against empty lists: nothing was cut.
            requested_reviewers_total: 0,
            assignees_total: 0,
            latest_reviews_total: 0,
            labels_total: 0,
        }
    }

    #[test]
    fn notifies_when_ci_turns_red() {
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full(
            "acme/a",
            1,
            CiState::Failure,
            MergeState::Mergeable,
        )];
        let b = newly_broken(&before, &after);
        assert_eq!(b.len(), 1);
        // The KIND, not the prose: the kind is what the notification
        // filter matches on, and the wording is asserted separately.
        assert_eq!(b[0].kind, BreakageKind::CiFailed);
        assert!(b[0].url.ends_with("/pull/1"), "must be clickable");
    }

    #[test]
    fn notifies_when_a_conflict_appears() {
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Conflicted,
        )];
        assert_eq!(
            newly_broken(&before, &after)[0].kind,
            BreakageKind::Conflicted
        );
    }

    /// The rule that stops it being a firehose: an ALREADY-red PR must not
    /// re-notify every 60 seconds forever.
    #[test]
    fn does_not_renotify_a_pr_that_was_already_broken() {
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Failure,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full(
            "acme/a",
            1,
            CiState::Failure,
            MergeState::Mergeable,
        )];
        assert!(newly_broken(&before, &after).is_empty());
    }

    /// First run has no "before", and assuming green would notify the
    /// entire backlog at launch -- 13 notifications on this account today.
    #[test]
    fn never_notifies_for_a_pr_it_has_not_seen_before() {
        let after = vec![pr_full(
            "acme/a",
            1,
            CiState::Failure,
            MergeState::Conflicted,
        )];
        assert!(newly_broken(&[], &after).is_empty());
    }

    #[test]
    fn recovering_does_not_notify() {
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Failure,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Mergeable,
        )];
        assert!(newly_broken(&before, &after).is_empty());
    }

    /// `Checking` is GitHub still computing mergeability, which happens on
    /// every push -- treating it as a conflict would notify constantly.
    #[test]
    fn checking_mergeability_is_not_a_breakage() {
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full("acme/a", 1, CiState::Success, MergeState::Checking)];
        assert!(newly_broken(&before, &after).is_empty());
        // ...and pending CI likewise.
        let after2 = vec![pr_full(
            "acme/a",
            1,
            CiState::Pending,
            MergeState::Mergeable,
        )];
        assert!(newly_broken(&before, &after2).is_empty());
    }

    #[test]
    fn matches_prs_by_repo_and_number_together() {
        // Numbers repeat across repos; a number-only join would compare
        // unrelated PRs and report phantom breakages.
        let before = vec![pr_full(
            "acme/a",
            1,
            CiState::Success,
            MergeState::Mergeable,
        )];
        let after = vec![pr_full(
            "acme/b",
            1,
            CiState::Failure,
            MergeState::Mergeable,
        )];
        assert!(newly_broken(&before, &after).is_empty());
    }

    /// The core #22 invariant: a targeted recheck's results replace only
    /// the PRs it actually re-fetched, identified by (repo, number) --
    /// everything else in the last known snapshot survives untouched. This
    /// is what keeps a partial recheck from silently dropping PRs that
    /// weren't part of it.
    #[test]
    fn merge_by_identity_replaces_only_matching_prs() {
        let base = vec![
            pr("octocat/hello-world", 1, MergeState::Checking),
            pr("octocat/hello-world", 2, MergeState::Mergeable),
            pr("octocat/spoon-knife", 7, MergeState::Checking),
        ];
        let updates = vec![
            pr("octocat/hello-world", 1, MergeState::Mergeable),
            pr("octocat/spoon-knife", 7, MergeState::Conflicted),
        ];

        let merged = merge_by_identity(&base, &updates);

        assert_eq!(merged[0].merge, MergeState::Mergeable); // resolved
        assert_eq!(merged[1].merge, MergeState::Mergeable); // untouched, unchanged
        assert_eq!(merged[2].merge, MergeState::Conflicted); // resolved
    }

    /// A PR present in the base snapshot but absent from the recheck's
    /// results (e.g. it was closed between the two fetches) must be kept,
    /// not dropped -- the recheck only ever narrows toward "resolved,"
    /// never toward "gone," since that would blank part of the UI on a
    /// mismatch that isn't even an error.
    #[test]
    fn merge_by_identity_keeps_prs_absent_from_the_update_set() {
        let base = vec![pr("octocat/hello-world", 1, MergeState::Checking)];
        let updates: Vec<PullRequest> = vec![];

        let merged = merge_by_identity(&base, &updates);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].merge, MergeState::Checking);
    }

    /// Distinct repos can legally share a PR number -- identity must be the
    /// (repo, number) pair, not the number alone, or a recheck could
    /// overwrite the wrong repo's PR.
    #[test]
    fn merge_by_identity_disambiguates_same_number_in_different_repos() {
        let base = vec![
            pr("octocat/hello-world", 7, MergeState::Checking),
            pr("octocat/spoon-knife", 7, MergeState::Checking),
        ];
        let updates = vec![pr("octocat/hello-world", 7, MergeState::Mergeable)];

        let merged = merge_by_identity(&base, &updates);

        assert_eq!(merged[0].merge, MergeState::Mergeable);
        assert_eq!(merged[1].merge, MergeState::Checking); // different repo, untouched
    }
}
