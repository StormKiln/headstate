//! Background polling.
//!
//! Polling lives in Rust rather than React so it continues while the window
//! is hidden to the tray -- which is what makes the tray badge meaningful.
//! React never talks to GitHub directly: it renders whatever snapshot is on
//! disk and listens for the `prs-updated` event.

use crate::github::client::{ClientError, FetchedList, GitHubClient};
use crate::github::model::{needs_attention_count, CiState, MergeState, PullRequest, ReviewState};
use crate::identity::Source;
use crate::source_poll;
use crate::store::source_cache::Coverage;
use crate::store::{open_db, CachedList};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

/// Default interval between completed queue passes. Continuations of an
/// already-started visible pass have a separate 15-second clock.
pub const DEFAULT_FOCUSED_SECS: u64 = 120;

/// Minimum configured new-pass interval. Historical measurements on 2026-09-11
/// charged 2 GraphQL points per PRS_QUERY document (25 rows), or 4 for the then
/// two-search tick. They do not price today's incremental scans, retries,
/// advisory/detail reads or foreground actions. HTTP attempt bounds are not
/// GraphQL point bounds; current provider quota headers and admission reserves
/// govern requests. Current aggregate GraphQL cost has not been measured.
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

/// Ordinary poll cadence for the current window state. This does not include
/// visible queue continuations or user-triggered traffic and is not a quota proof.
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
/// losing one must never take down polling. macOS 10.14+ clicks carry the complete
/// provider-qualified identity through the native notification delegate.
fn notify_breakage(app: &AppHandle, b: &Breakage) {
    use tauri_plugin_notification::NotificationExt;

    let body = format!(
        "{}: {}#{} {}",
        b.source.host,
        b.repo,
        b.number,
        b.kind.reason()
    );
    #[cfg(target_os = "macos")]
    if crate::notification_navigation::supported() {
        crate::notification_navigation::notify(
            &b.title,
            &body,
            &crate::identity::PrIdentity {
                source: b.source.clone(),
                repo: b.repo.clone(),
                number: b.number,
            },
        );
        return;
    }
    // Keep legacy delivery on macOS 10.13, where UserNotifications is absent.
    // Its notifications retain the platform plugin's existing click behavior.
    if !notification_allowed(app) {
        return;
    }
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
    pub source: Source,
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
    /// The terminal to open Claude commands in, or empty for none.
    ///
    /// A launcher template holding `{command}`, e.g.
    /// `open -a Terminal {command}`. See [`crate::claude::launch`] for
    /// the grammar and for why the command lands in exactly one argv
    /// slot.
    ///
    /// Defaults EMPTY, and that is the whole reason this is a setting
    /// rather than a detection. `claudify_command` records why nothing
    /// is spawned today: macOS has no default-terminal concept, so a
    /// machine with both Terminal.app and iTerm gives no way to know
    /// which the user wants. That argument rules out guessing; it does
    /// not rule out asking once. Unset, the app behaves exactly as it
    /// did -- the buttons copy, and no launch affordance appears.
    #[serde(default)]
    pub terminal_command: String,
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
    /// Whether the Jira integration is switched on (epic #2031).
    ///
    /// A CAPABILITY, like `claude_integrations_enabled` and for the same
    /// reasons. Defaults OFF: most users do not use Jira, and a switched-off
    /// integration spawns no `acli` at all.
    #[serde(default)]
    pub jira_enabled: bool,
    /// Jira project keys NOT to match, for a project whose key collides
    /// with branch-name noise. Empty means every visible project matches.
    #[serde(default)]
    pub jira_excluded_projects: Vec<String>,
    /// What a project's tickets should say while a pull request is open
    /// and once it has merged, by project key (#2034). A project with no
    /// entry -- or an entry with a side unset -- takes the defaults the
    /// drift rules always used: in progress while open, done once merged.
    #[serde(default)]
    pub jira_status_expectations:
        std::collections::BTreeMap<String, crate::jira::model::StatusExpectations>,
    /// Whether pull-request rows show ticket chips. Defaults ON: the
    /// chips are what the integration is for. Off hides them from PR rows
    /// only; the pull request's own page, worktree rows and branch rows
    /// keep theirs.
    #[serde(default = "default_true")]
    pub jira_chips_on_prs: bool,
    /// How "Start work" names a ticket's branch (#2045): `{key}` and
    /// `{summary}` placeholders, filled in by the frontend, and the result
    /// is editable before anything runs. Empty means the default,
    /// `{key}-{summary}`, so a stored row from before this field existed
    /// reads as "not set" rather than as an empty template.
    #[serde(default)]
    pub jira_branch_template: String,
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
            // Empty: no terminal configured, so the buttons copy
            // exactly as they always have.
            terminal_command: String::new(),
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
            // OFF: asked for, not assumed. See the field.
            jira_enabled: false,
            jira_excluded_projects: Vec::new(),
            // Empty: every project uses the drift rules' defaults.
            jira_status_expectations: std::collections::BTreeMap::new(),
            jira_chips_on_prs: true,
            // Empty: the default template. See the field.
            jira_branch_template: String::new(),
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
    !pr.observation.as_ref().is_some_and(|o| {
        o.confirmed_review.is_some()
            || !o.unknown_fields.is_empty()
            || o.state == crate::inventory::ObservationState::Retained
            || !o.retained_fields.is_empty()
    }) && !pr.is_draft
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
                .find(|p| p.identity() == pr.identity())
                .is_none_or(|was| !ready_for_review(was))
        })
        .map(|pr| Breakage {
            source: pr.source.clone(),
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
        .filter(|pr| !previous.iter().any(|p| p.identity() == pr.identity()))
        .map(|pr| Breakage {
            source: pr.source.clone(),
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
            let was = previous.iter().find(|p| p.identity() == pr.identity())?;
            let kind = if pr.ci == CiState::Failure && was.ci != CiState::Failure {
                BreakageKind::CiFailed
            } else if pr.merge == MergeState::Conflicted && was.merge != MergeState::Conflicted {
                BreakageKind::Conflicted
            } else {
                return None;
            };
            Some(Breakage {
                source: pr.source.clone(),
                title: pr.title.clone(),
                repo: pr.repo.clone(),
                number: pr.number,
                url: pr.url.clone(),
                kind,
            })
        })
        .collect()
}

/// Whether a receipt can be the baseline that "new" is measured against
/// (#1939).
///
/// A Complete receipt can. So can a Partial one once this list has had a
/// validated traversal (`completed_at`, which a fresh or tainted pass
/// keeps): every page has then been read, and the rows a later partial
/// pass did not reach are retained rather than dropped, so an identity
/// absent from the baseline was absent from every page observed before.
/// Before the first validated traversal, absence may only mean a page not
/// fetched yet, and announcing it would be a confident claim that might be
/// wrong -- so nothing is announced until then.
pub(crate) fn notification_baseline(receipt: &FetchedList) -> bool {
    matches!(receipt.coverage, Coverage::Complete)
        || receipt
            .scan
            .as_ref()
            .is_some_and(|scan| scan.state.completed_at.is_some())
}

/// Compare an accepted receipt against the last baseline, then keep the
/// receipt as the next baseline only if it can be one (#789, #1939).
///
/// `None` means "no usable baseline": the first tick after launch, or a
/// list never validated end to end. It announces nothing.
pub(crate) fn announce_against(
    baseline: &mut Option<Vec<PullRequest>>,
    receipt: &FetchedList,
    diff: fn(&[PullRequest], &[PullRequest]) -> Vec<Breakage>,
) -> Vec<Breakage> {
    let announced = baseline
        .as_deref()
        .map(|before| diff(before, &receipt.prs))
        .unwrap_or_default();
    *baseline = notification_baseline(receipt).then(|| receipt.prs.clone());
    announced
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
    let Ok(dir) = crate::commands::profile_dir(app) else {
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

/// Tell the UI the review queue changed.
///
/// The counterpart to [`emit_authored`] for the OTHER list this loop
/// fetches. It writes nothing: `source_poll::reconcile_github` already
/// saved the snapshot inside its own transaction before this runs, and the
/// To Review page reads that snapshot on mount (#1118, #1940).
pub(crate) fn emit_reviewing(app: &AppHandle, result: &FetchedList) {
    let short = result
        .total
        .map(|total| total.saturating_sub(crate::inventory::observed_count(&result.prs)));
    let _ = app.emit("reviewing-short", short);
    let _ = app.emit("reviewing-updated", &result.prs);
}

/// Emit `prs-updated` and set the badge for an authored list.
///
/// Writes nothing (#1940). Every caller runs after
/// `source_poll::reconcile_github`, which is the one writer of the queue
/// snapshot: it saves inside the same transaction as the scan checkpoint
/// and the GitHub facts, and fails the publication if that save fails. A
/// second write here, outside that transaction, could only ever persist
/// the same rows again -- or, if it diverged, the wrong ones.
fn emit_authored(app: &AppHandle, prs: &[PullRequest]) {
    if let Err(e) = app.emit("prs-updated", prs) {
        log::warn!("failed to emit prs-updated: {e}");
    }
    // The badge is why polling lives in Rust at all: it has to stay correct
    // while the window is hidden, when no React component is mounted to
    // compute it. Counted here from the same list just persisted, using the
    // model's single owner of the rule.
    crate::tray::set_badge(app, needs_attention_count(prs));
}

fn same_active_client(active: Option<&Arc<GitHubClient>>, captured: &Arc<GitHubClient>) -> bool {
    active.is_some_and(|active| Arc::ptr_eq(active, captured))
}

fn active_recheck_client(app: &AppHandle, captured: &Arc<GitHubClient>) -> bool {
    same_active_client(
        app.try_state::<crate::commands::GhClient>()
            .as_ref()
            .and_then(|state| state.0.as_ref()),
        captured,
    )
}

/// #22: one delayed supplemental refresh of at most three captured Checking
/// identities. This task never schedules another recheck or advances a scan.
fn spawn_recheck(app: AppHandle, client: Arc<GitHubClient>, capture: source_poll::CheckingCapture) {
    spawn_worker(async move {
        tokio::time::sleep(RECHECK_DELAY).await;
        // Keep the original managed Arc, not a newly wrapped read-context clone.
        {
            let Some(_permit) = source_poll::checking_publication(&app, &capture).await else {
                return;
            };
            if !active_recheck_client(&app, &client) {
                return;
            }
        }
        // No publication lock spans provider I/O. Completed earlier targets
        // survive a later timeout; no generic scan fetch or recursive schedule.
        let rows = client
            .fetch_checking(&capture.targets, &capture.owner, FETCH_TIMEOUT)
            .await;
        if rows.is_empty() {
            return;
        }
        let Some(publication) = source_poll::checking_publication(&app, &capture).await else {
            return;
        };
        if !active_recheck_client(&app, &client) {
            return;
        }
        match source_poll::apply_checking(&app, &publication, &capture, rows).await {
            Ok(Some(result)) => {
                emit_authored(&app, &result.prs);
                source_poll::complete_checking(&app, publication, result);
            }
            Ok(None) => {}
            Err(error) => log::warn!("targeted recheck failed: {}", error.message),
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
/// by more than the overlap saves. The removed `fetch_prs_and_reviewing`
/// ran the two searches concurrently, and its own diagnostic comment was
/// about exactly this: a total far above the slower of the two meant they
/// were not actually overlapping.
///
/// # Why 45s
///
/// It must be under `MIN_FOCUSED_SECS` (60) with enough margin that a tick
/// finishing at the ceiling still lands before its successor starts, and
/// it must leave the FIRST fetch its full `FETCH_TIMEOUT` -- the active
/// list is what the UI renders. The other list receives the residual budget
/// and keeps its previous receipt if it cannot refresh. 45s gives the first fetch all 30s
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
/// caller skips the second list below [`MIN_USEFUL_FETCH_BUDGET`]: a
/// request with no time to answer still spends a rate-limit point.
pub fn remaining_tick_budget(spent: Duration) -> Duration {
    TICK_TIMEOUT.saturating_sub(spent).min(FETCH_TIMEOUT)
}

/// The least budget worth sending the second list's request with.
///
/// Below this the request would be issued only to hit its own deadline,
/// and that timeout would then be reported as GitHub failing to answer --
/// the #1050 pattern (#1932). Not issuing it is the honest outcome: the
/// list keeps its last receipt and status, and the next tick asks again.
/// A few seconds, because a measured tick needs ~4.2s for both lists and a
/// completed page inside a short budget is still kept.
pub const MIN_USEFUL_FETCH_BUDGET: Duration = Duration::from_secs(5);

// Sequential avoids multiplying provider contention. The selected list owns
// the first full budget and publishes inside its callback before the next starts.
// The second list is `None` when it was not asked: too little of the tick was
// left to send it (#1932). Its callback is then never invoked, so it neither
// begins an attempt nor spends a request.
async fn prioritized_pair<A, R, T, U>(
    reviewing_first: bool,
    authored: A,
    reviewing: R,
) -> (Option<T>, Option<U>)
where
    A: std::ops::AsyncFnOnce(Duration) -> T,
    R: std::ops::AsyncFnOnce(Duration) -> U,
{
    let started = tokio::time::Instant::now();
    let second_budget = || {
        Some(remaining_tick_budget(started.elapsed()))
            .filter(|budget| *budget >= MIN_USEFUL_FETCH_BUDGET)
    };
    if reviewing_first {
        let review = reviewing(FETCH_TIMEOUT).await;
        let own = match second_budget() {
            Some(budget) => Some(authored(budget).await),
            None => None,
        };
        (own, Some(review))
    } else {
        let own = authored(FETCH_TIMEOUT).await;
        let review = match second_budget() {
            Some(budget) => Some(reviewing(budget).await),
            None => None,
        };
        (Some(own), review)
    }
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
pub struct GithubSourceEnabled(pub Arc<AtomicBool>);

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
///
/// The ONE rule: `source_poll` settles a failed attempt as `Failed` when
/// this holds and `Retrying` when it does not, and `emit_status` derives
/// `poll-error` from that phase. The loop no longer applies a second copy
/// of it to a second counter (#1934).
pub(crate) fn should_surface(transient: bool, consecutive: u32) -> bool {
    !transient || consecutive >= FAILURES_BEFORE_BANNER
}

/// The `poll-state` the authored list's status stands for.
///
/// `emit_status` sends this with every authored status change. It carries
/// the outcome rather than a fixed `"idle"` because a suppressed transient
/// failure has nothing else to announce it: no `poll-error`, no
/// `prs-updated`. Emitting `"idle"` unconditionally told the bar the data was
/// current when the fetch had just failed, so it showed a green "Up to date"
/// over stale rows for up to ten minutes (#1104).
///
/// `Retrying` is a suppressed failure. `Failed` already has its `poll-error`
/// banner, which is the signal, so the bar has no further work to do.
pub(crate) fn tick_state(phase: &source_poll::Phase) -> &'static str {
    match phase {
        source_poll::Phase::Fetching => "fetching",
        source_poll::Phase::Retrying => "retrying",
        _ => "idle",
    }
}

/// A separate 15 s timer advances queue traversals between full ticks.
///
/// An INCOMPLETE traversal advances whether or not anyone is looking
/// (#1935). Each step reads at most two pages, and gating the next step
/// on visibility left it waiting for the full tick -- 600 s hidden -- so
/// a 200-PR queue left in the background took about half an hour to
/// fill. A traversal that has started is work already committed to, and
/// idling halfway through it is the one thing this clock exists to stop.
///
/// It is not a second hidden poll. Nothing here STARTS a traversal --
/// `started_at` must already be set, which only the full tick does -- and
/// the reads go through the same `ReadClass::Background` admission and
/// rate-limit budget as the tick, so a foreground read still goes first
/// and an exhausted budget still refuses. A failed step backs off through
/// `eligible_at` exactly as it does when visible.
///
/// Repairs and candidate confirmations are refinements of a list that is
/// already populated, not population, so they stay on the visible clock.
pub(crate) fn queue_continuation_due(
    state: &crate::queue_scan::State,
    now: i64,
    focused: bool,
    view_needs_github: bool,
    enabled: bool,
) -> bool {
    let visible = focused && view_needs_github;
    let incomplete = !state.done && state.started_at.is_some() && now >= state.eligible_at;
    enabled
        && (incomplete
            || (visible
                && (state.repair_due(now)
                    || state.candidates.iter().any(|c| c.eligible_at <= now))))
}

/// How often the continuation clock looks for a due traversal step.
///
/// Matches the `eligible_at = now + 15` a successful step sets in
/// `github/scan.rs`, so a traversal advances once per step delay rather
/// than once per full tick.
pub(crate) const CONTINUATION_INTERVAL: Duration = Duration::from_secs(15);

/// The production continuation clock and visibility dispatch boundary. The
/// adapter owns checkpoint/provider/publication work; sleeping happens after
/// both lists complete, so pending or refused work cannot create a busy loop.
pub(crate) async fn run_queue_continuations<F, Fut>(
    focused: Arc<AtomicBool>,
    view_needs_github: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    dispatch: F,
) where
    F: FnMut(CachedList, bool) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    run_queue_continuations_observed(
        focused,
        view_needs_github,
        enabled,
        dispatch,
        crate::measurement_desktop::recorder(),
    )
    .await;
}
async fn run_queue_continuations_observed<F, Fut>(
    focused: Arc<AtomicBool>,
    view_needs_github: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    mut dispatch: F,
    recorder: Option<&crate::measurement::Recorder>,
) where
    F: FnMut(CachedList, bool) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        // Before the wait, so a clock stuck in a dispatch is overdue
        // (#1937).
        crate::background::GITHUB_CONTINUATION.tick(CONTINUATION_INTERVAL);
        tokio::time::sleep(CONTINUATION_INTERVAL).await;
        crate::queue_measurement::aggregate(
            recorder,
            crate::measurement::AggregateKind::Tick,
            crate::measurement::WorkClass::Background,
        );
        let mut hidden_counted = false;
        for list in [CachedList::Authored, CachedList::Reviewing] {
            if !enabled.load(Ordering::Relaxed) {
                break;
            }
            // Visibility no longer stops the clock (#1935): it is handed
            // to the dispatcher, whose `queue_continuation_due` decides
            // which work a hidden window still owes. Still counted once
            // per tick, as before, so the journal's hidden-tick figure
            // keeps its meaning.
            let visible =
                focused.load(Ordering::Relaxed) && view_needs_github.load(Ordering::Relaxed);
            if !visible && !hidden_counted {
                hidden_counted = true;
                crate::queue_measurement::aggregate(
                    recorder,
                    crate::measurement::AggregateKind::Hidden,
                    crate::measurement::WorkClass::Background,
                );
            }
            dispatch(list, visible).await;
        }
    }
}

/// Health follows the accepted publication, including its durable write.
/// A fetch that loses publication admission or its write race is neutral.
fn publication_health<T>(
    result: Option<&Result<T, source_poll::Failure>>,
) -> crate::background::Cycle {
    use crate::background::Cycle;
    match result {
        None => Cycle::Superseded,
        Some(Ok(_)) => Cycle::Ok,
        Some(Err(failure)) if failure.superseded => Cycle::Superseded,
        Some(Err(failure)) if failure.not_asked => Cycle::NotAsked,
        Some(Err(failure)) => Cycle::Failed(crate::redact::redact(&failure.message)),
    }
}

async fn continue_queues(
    app: AppHandle,
    client: Arc<GitHubClient>,
    focused: Arc<AtomicBool>,
    view_needs_github: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
) {
    run_queue_continuations(focused, view_needs_github, enabled, move |list, visible| {
        let app = app.clone();
        let client = client.clone();
        async move {
            let Some(owner) = client
                .known_viewer()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
            else {
                return;
            };
            let path = crate::commands::db_path(&app);
            let owner = owner.clone();
            let due = tauri::async_runtime::spawn_blocking(move || {
                let conn = open_db(&path).ok()?;
                let loaded =
                    crate::queue_scan::load(&conn, &Source::default(), list, &owner).ok()?;
                Some(queue_continuation_due(
                    &loaded.state,
                    chrono::Utc::now().timestamp(),
                    visible,
                    visible,
                    true,
                ))
            })
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
            crate::queue_measurement::aggregate(
                crate::measurement_desktop::recorder(),
                if due {
                    crate::measurement::AggregateKind::Continuation
                } else {
                    crate::measurement::AggregateKind::NoWork
                },
                crate::measurement::WorkClass::Background,
            );
            if !due {
                return;
            }
            let attempt = source_poll::begin(&app, Source::default(), list).await;
            let scoped = client.with_read_context(crate::github::admission::ReadContext::new(
                crate::github::admission::ReadClass::Background,
                FETCH_TIMEOUT,
            ));
            let fetched =
                source_poll::fetch_github_continuation(&app, &scoped, list, FETCH_TIMEOUT).await;
            let publication = if fetched.is_ok() {
                source_poll::success_publication(&app, &attempt).await
            } else {
                source_poll::publication(&app, &attempt).await
            };
            let Some(publication) = publication else {
                return;
            };
            let result = match fetched {
                Ok(result) => source_poll::reconcile_github(&app, &publication, result).await,
                Err(error) => Err(source_poll::Failure::from(&error)),
            };
            crate::background::announce_degraded(
                &app,
                &crate::background::GITHUB_CONTINUATION,
                crate::background::GITHUB_CONTINUATION
                    .record_cycle(&[publication_health(Some(&result))]),
            );
            if let Ok(result) = &result {
                if list == CachedList::Reviewing {
                    emit_reviewing(&app, result);
                } else {
                    emit_authored(&app, &result.prs);
                }
            }
            source_poll::complete(&app, publication, result);
        }
    })
    .await;
}

pub fn spawn(
    app: AppHandle,
    client: Arc<GitHubClient>,
    focused: Arc<AtomicBool>,
    waker: Arc<Notify>,
    interval_secs: Arc<AtomicU64>,
    view_needs_github: Arc<AtomicBool>,
    github_source_enabled: Arc<AtomicBool>,
) {
    spawn_worker(continue_queues(
        app.clone(),
        client.clone(),
        focused.clone(),
        view_needs_github.clone(),
        github_source_enabled.clone(),
    ));
    spawn_worker(async move {
        let mut previous: Vec<PullRequest> = Vec::new();
        // The baseline `newly_appeared` compares against, and whether
        // there IS one (#789, #1939).
        //
        // Not `previous` with an emptiness check: a user whose last pull
        // request merged has a genuinely empty list, and the next one they
        // open is news. "Empty" and "never looked" are different answers,
        // and only `None` says the second.
        let mut appeared_baseline: Option<Vec<PullRequest>> = None;
        // The review queue as of the last tick, and whether there HAS
        // been one.
        //
        // `None` rather than an empty Vec: for the ready-to-review
        // notification an absent prior entry means "was not ready", so
        // an empty history and a genuinely empty queue would be
        // indistinguishable -- and the first tick would announce every
        // pull request already waiting.
        let mut previous_reviewing: Option<Vec<PullRequest>> = None;
        loop {
            if !github_source_enabled.load(Ordering::Relaxed) {
                // Source selection pauses GitHub network work, including the
                // initial tick after a relaunch. Switching back wakes us.
                // Waiting on purpose, with no deadline: not a stall (#1937).
                crate::background::GITHUB_POLL.park();
                waker.notified().await;
                continue;
            }
            // `poll-state` and `poll-error` come from `source_poll`'s
            // `emit_status` alone, driven by the authored list's status and
            // its one failure counter. The loop emitting them as well, under
            // its own counter, sent each banner twice and let the two
            // copies disagree (#1934).

            // DIAGNOSTIC LOGGING (Settings > diagnostic log). The background loop
            // shares one client -- and one connection pool -- with
            // whatever the user just clicked, so a tick that overlaps a
            // foreground query can be what makes the foreground query
            // look slow. Logging the tick boundaries makes that overlap
            // visible against the `cmd get_reviewing` bracket.
            crate::diag!("[diag] poll tick start");
            let tick_started = std::time::Instant::now();
            // Publish the active list before asking for the other list. Each
            // collector retains completed pages inside its assigned budget.
            let authored_work = async |budget| {
                let authored_attempt =
                    source_poll::begin(&app, Source::default(), CachedList::Authored).await;
                let started = std::time::Instant::now();
                let scoped = client.with_read_context(crate::github::admission::ReadContext::new(
                    crate::github::admission::ReadClass::Background,
                    budget,
                ));
                let fetched =
                    source_poll::fetch_github_step(&app, &scoped, CachedList::Authored, budget)
                        .await;
                let fetch_ms = started.elapsed().as_millis() as u64;
                // This report is readable from a paired phone; redact the
                // failure while retaining its timeout as a separate measure.
                let tick_failure: Option<(String, Option<u64>)> = match &fetched {
                    Ok(_) => None,
                    Err(e) => Some((
                        crate::redact::redact(&e.to_string()),
                        match e {
                            ClientError::Timeout(s) => Some(*s),
                            _ => None,
                        },
                    )),
                };
                // The loop's own health (#1937). A declined request was
                // not sent, so it neither failed nor succeeded (#1928).
                let health = match (&fetched, &tick_failure) {
                    (Err(e), _) if source_poll::Failure::from(e).not_asked => {
                        crate::background::Cycle::NotAsked
                    }
                    (_, Some((message, _))) => crate::background::Cycle::Failed(message.clone()),
                    _ => crate::background::Cycle::Ok,
                };
                let authored_publication = if fetched.is_ok() {
                    source_poll::success_publication(&app, &authored_attempt).await
                } else {
                    source_poll::publication(&app, &authored_attempt).await
                };
                if let Some(publication) = authored_publication {
                    match fetched {
                        Ok(result) => {
                            let result =
                                match source_poll::reconcile_github(&app, &publication, result)
                                    .await
                                {
                                    Ok(result) => result,
                                    Err(failure) => {
                                        let message = failure.message.clone();
                                        // A lost write race is not a failed
                                        // poll: the winner's data stands (#1933).
                                        let failed = !failure.superseded;
                                        let result: Result<FetchedList, _> = Err(failure);
                                        let health = publication_health(Some(&result));
                                        source_poll::complete(&app, publication, result);
                                        return (
                                            fetch_ms,
                                            failed.then_some((message, None)),
                                            health,
                                        );
                                    }
                                };
                            let appeared =
                                announce_against(&mut appeared_baseline, &result, newly_appeared);
                            let receipt = result.clone();
                            let FetchedList { prs, total, .. } = result;
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
                            // Newly APPEARED pull requests (#789), measured
                            // against `appeared_baseline` above rather than
                            // `previous`: `None` until a receipt that can be a
                            // baseline has been seen, which is the whole of the
                            // first-tick suppression. A Partial receipt counts
                            // once the list has been validated end to end
                            // (#1939), so a long or tainted traversal no longer
                            // silences every arrival.
                            //
                            // `newly_broken` above needs no such guard: it never
                            // fires for a pull request absent from `previous`, so
                            // an empty previous list announces nothing by
                            // construction. This rule is the opposite shape --
                            // absent means new -- so it needs the baseline.
                            for b in appeared {
                                if prefs.wants(b.kind) {
                                    notify_breakage(&app, &b);
                                }
                            }

                            previous = prs.clone();
                            // Null replaces stale numeric advice with unknown completeness.
                            let truncated = total.map(|total| {
                                truncation_payload(crate::inventory::observed_count(&prs), total)
                            });
                            if let Err(e) = app.emit("prs-truncated", truncated) {
                                log::warn!("failed to emit prs-truncated: {e}");
                            }
                            // The heartbeat that makes "it stopped updating"
                            // answerable: if the log ends here, the loop died or
                            // the machine slept; if it keeps ticking, the problem
                            // is downstream. Counts only -- never titles, never
                            // repository names.
                            log::info!(
                                "poll ok: {} open, {} need attention (matching total: {total:?})",
                                prs.len(),
                                needs_attention_count(&prs)
                            );
                            // Fields GitHub refused on this fetch, then cleared:
                            // a later complete response must stop reporting a
                            // shortfall that no longer exists. Emitted even when
                            // zero, so the banner disappears on recovery rather
                            // than sticking until relaunch.
                            let refused =
                                crate::github::client::REFUSED_FIELDS.swap(0, Ordering::Relaxed);
                            if let Err(e) = app.emit("prs-incomplete", refused) {
                                log::warn!("failed to emit prs-incomplete: {e}");
                            }

                            crate::log_dedup::log_cleared!("poll.failed", "poll succeeded again");
                            emit_authored(&app, &prs);
                            let capture =
                                source_poll::complete_with_checking(&app, publication, receipt);
                            if let Some(capture) = capture.filter(|_| has_checking(&prs)) {
                                spawn_recheck(app.clone(), client.clone(), capture);
                            }
                        }
                        // A failed poll leaves the last snapshot in place rather
                        // than blanking the UI; the next tick retries.
                        Err(e) => {
                            crate::log_dedup::log_while!(warn, "poll.failed", "poll failed: {e}");
                            // `complete` settles the status, and `emit_status`
                            // decides from it whether this surfaces as a
                            // banner or stays "retrying" (#1104, #1934).
                            source_poll::complete(
                                &app,
                                publication,
                                Err(source_poll::Failure::from(&e)),
                            );
                        }
                    }
                } else {
                    return (fetch_ms, None, publication_health::<()>(None));
                }
                (fetch_ms, tick_failure, health)
            };
            let reviewing_work = async |budget| {
                let reviewing_attempt =
                    source_poll::begin(&app, Source::default(), CachedList::Reviewing).await;
                let scoped = client.with_read_context(crate::github::admission::ReadContext::new(
                    crate::github::admission::ReadClass::Background,
                    budget,
                ));
                let reviewing_now =
                    source_poll::fetch_github_step(&app, &scoped, CachedList::Reviewing, budget)
                        .await
                        .map_err(|e| source_poll::Failure::from(&e));
                // A declined request was not sent, so it did not fail (#1928).
                let reviewing_outcome = match &reviewing_now {
                    Ok(_) => "ok",
                    Err(failure) if failure.not_asked => "not asked",
                    Err(_) => "failed",
                };
                let health = match &reviewing_now {
                    Ok(_) => crate::background::Cycle::Ok,
                    Err(failure) if failure.not_asked => crate::background::Cycle::NotAsked,
                    Err(failure) => crate::background::Cycle::Failed(crate::redact::redact(
                        &format!("review queue: {}", failure.message),
                    )),
                };
                // Each successful queue survives a failure from the other queue.
                let review_publication = if reviewing_now.is_ok() {
                    source_poll::success_publication(&app, &reviewing_attempt).await
                } else {
                    source_poll::publication(&app, &reviewing_attempt).await
                };
                if let Some(publication) = review_publication {
                    match reviewing_now {
                        Ok(now) => {
                            let now = match source_poll::reconcile_github(&app, &publication, now)
                                .await
                            {
                                Ok(now) => now,
                                Err(failure) => {
                                    let outcome = if failure.superseded {
                                        "superseded"
                                    } else {
                                        "failed"
                                    };
                                    let result: Result<FetchedList, _> = Err(failure);
                                    let health = publication_health(Some(&result));
                                    source_poll::complete(&app, publication, result);
                                    return (outcome, health);
                                }
                            };
                            let prefs = read_notify_prefs(&app).await;
                            // Partial receipts count once the queue has been
                            // validated end to end (#1939).
                            for b in announce_against(&mut previous_reviewing, &now, newly_ready) {
                                if prefs.wants(b.kind) {
                                    notify_breakage(&app, &b);
                                }
                            }
                            emit_reviewing(&app, &now);
                            source_poll::complete(&app, publication, Ok(now));
                        }
                        Err(error) => source_poll::complete(&app, publication, Err(error)),
                    }
                } else {
                    return ("superseded", publication_health::<()>(None));
                }
                (reviewing_outcome, health)
            };
            let (authored_outcome, reviewing_outcome) =
                prioritized_pair(client.reviewing_first(), authored_work, reviewing_work).await;
            // A list left unasked keeps its last status and receipt: it was
            // not sent, so nothing failed (#1932).
            let (reviewing_outcome, reviewing_health) =
                reviewing_outcome.unwrap_or(("not asked", crate::background::Cycle::NotAsked));
            let authored_health = authored_outcome
                .as_ref()
                .map_or(crate::background::Cycle::NotAsked, |(_, _, h)| h.clone());
            crate::background::announce_degraded(
                &app,
                &crate::background::GITHUB_POLL,
                crate::background::GITHUB_POLL.record_cycle(&[authored_health, reviewing_health]),
            );
            crate::diag!(
                "[diag] poll tick fetch done {}ms authored={} reviewing={}",
                tick_started.elapsed().as_millis(),
                match &authored_outcome {
                    None => "not asked",
                    Some((_, None, _)) => "ok",
                    Some((_, Some(_), _)) => "failed",
                },
                reviewing_outcome
            );
            // The report describes the open-pull-request search; a tick that
            // did not send it has no outcome to record for it.
            if let Some((fetch_ms, tick_failure, _)) = authored_outcome {
                let ok = tick_failure.is_none();
                let (error, timed_out_after_secs) = tick_failure.unzip();
                crate::report::record_tick(crate::report::PollTickRecord {
                    at_unix_ms: crate::report::now_unix_ms(),
                    ok,
                    fetch_ms,
                    error,
                    timed_out_after_secs: timed_out_after_secs.flatten(),
                    // The authored status's own counter: one count of
                    // consecutive failures, not a second one kept here.
                    attempt: app
                        .state::<source_poll::SourcePolls>()
                        .get(&Source::default(), CachedList::Authored)
                        .consecutive_failures,
                    reviewing: reviewing_outcome.to_string(),
                });
            }

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
            crate::background::GITHUB_POLL.tick(sleep_for);
            crate::report::note_wait(sleep_for.as_secs());
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

/// Keep the production worker future unchanged while the isolated driver owns shutdown.
fn spawn_worker(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tauri::async_runtime::spawn(future);
    #[cfg(feature = "enterprise-harness")]
    crate::enterprise_harness::track_worker(task);
    #[cfg(not(feature = "enterprise-harness"))]
    drop(task);
}

#[cfg(test)]
mod tests {
    #[test]
    fn publication_health_keeps_lost_races_neutral_and_counts_persistence_failures() {
        use crate::background::{Cycle, Task};
        let task = Task::new("publication-health-test");
        task.record_cycle(&[Cycle::Failed("earlier failure".into())]);
        let before = task.snapshot();
        let lost: Result<(), _> = Err(source_poll::Failure {
            message: "another writer won".into(),
            transient: true,
            not_asked: false,
            superseded: true,
        });
        task.record_cycle(&[publication_health(Some(&lost))]);
        task.record_cycle(&[publication_health::<()>(None)]);
        assert_eq!(
            task.snapshot(),
            before,
            "superseded work is neither a success nor failure"
        );
        let failed: Result<(), _> = Err(source_poll::Failure {
            message: "local snapshot write failed".into(),
            transient: false,
            not_asked: false,
            superseded: false,
        });
        task.record_cycle(&[publication_health(Some(&failed))]);
        let after = task.snapshot();
        assert_eq!(after.total_failures, before.total_failures + 1);
        assert_eq!(after.consecutive_failures, before.consecutive_failures + 1);
        assert_eq!(after.last_success_ms, before.last_success_ms);
        task.record_cycle(&[publication_health(Some(
            &Ok::<(), source_poll::Failure>(()),
        ))]);
        assert_eq!(task.snapshot().consecutive_failures, 0);
    }

    #[tokio::test]
    async fn checking_client_requires_original_managed_arc() {
        let captured = Arc::new(GitHubClient::new(
            octocrab::Octocrab::builder()
                .personal_token("synthetic")
                .build()
                .unwrap(),
        ));
        let same = captured.clone();
        let derived = Arc::new(captured.with_read_context(
            crate::github::admission::ReadContext::new(
                crate::github::admission::ReadClass::Background,
                FETCH_TIMEOUT,
            ),
        ));
        assert!(same_active_client(Some(&same), &captured));
        assert!(!same_active_client(Some(&derived), &captured));
        assert!(!same_active_client(None, &captured));
    }

    #[tokio::test(start_paused = true)]
    async fn measurement_continuation_clock_records_hidden_ticks_and_says_so_to_dispatch() {
        use crate::measurement::{Config, Platform, Recorder, Role};
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(Config {
            directory: dir.path().canonicalize().unwrap().join("journal"),
            epoch: [49; 16],
            role: Role::Desktop,
            platform: Platform::Macos,
            build: "synthetic".into(),
        })
        .unwrap();
        recorder.set_enabled(true);
        let focused = Arc::new(AtomicBool::new(false));
        let visible_calls = std::sync::atomic::AtomicUsize::new(0);
        let hidden_calls = std::sync::atomic::AtomicUsize::new(0);
        let run = run_queue_continuations_observed(
            focused,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
            |_, visible| {
                if visible {
                    &visible_calls
                } else {
                    &hidden_calls
                }
                .fetch_add(1, Ordering::Relaxed);
                async {}
            },
            Some(&recorder),
        );
        assert!(tokio::time::timeout(Duration::from_secs(31), run)
            .await
            .is_err());
        // Two 15 s ticks, both lists each time, every one marked hidden:
        // the dispatcher decides what a hidden window is still owed.
        assert_eq!(visible_calls.load(Ordering::Relaxed), 0);
        assert_eq!(hidden_calls.load(Ordering::Relaxed), 4);
        tokio::time::resume();
        let path = dir.path().canonicalize().unwrap().join("report.jsonl");
        recorder.export_to(path.clone()).await.unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"metric\":\"tick\""));
        assert!(text.contains("\"metric\":\"hidden\""));
    }

    #[test]
    fn queue_continuations_only_advance_due_existing_passes() {
        let mut state = crate::queue_scan::State {
            started_at: Some(1000),
            eligible_at: 1015,
            ..Default::default()
        };
        assert!(!queue_continuation_due(&state, 1014, true, true, true));
        assert!(queue_continuation_due(&state, 1015, true, true, true));
        // An incomplete traversal is owed its next step whether or not
        // the window is focused or the view shows PRs (#1935)...
        assert!(queue_continuation_due(&state, 2000, false, true, true));
        assert!(queue_continuation_due(&state, 2000, true, false, true));
        assert!(queue_continuation_due(&state, 2000, false, false, true));
        // ...but never while the GitHub source is switched off.
        assert!(!queue_continuation_due(&state, 2000, true, true, false));
        state.done = true;
        assert!(!queue_continuation_due(&state, 2000, true, true, true));
        state.done = false;
        state.started_at = None;
        assert!(!queue_continuation_due(&state, 2000, true, true, true));
        // Configured full-poll cadence stays independent of queue progress.
        assert_eq!(interval_for_secs(true, 300), Duration::from_secs(300));
        assert_eq!(interval_for_secs(false, 300), Duration::from_secs(1500));
    }

    /// A finished list's refinements are not population: a hidden window
    /// leaves repairs and candidate confirmations to the visible clock.
    #[test]
    fn hidden_windows_owe_no_repair_or_confirmation() {
        let mut state = crate::queue_scan::State {
            started_at: Some(1000),
            done: true,
            eligible_at: 1015,
            ..Default::default()
        };
        state.candidates.push_back(crate::queue_scan::Candidate {
            isolated: false,
            identity: crate::identity::PrIdentity {
                source: Source::default(),
                repo: "o/r".into(),
                number: 1,
            },
            id: "id".into(),
            head: "h".into(),
            created_at: chrono::Utc::now(),
            negative_at: None,
            eligible_at: 1015,
            failures: 0,
        });
        assert!(queue_continuation_due(&state, 2000, true, true, true));
        assert!(!queue_continuation_due(&state, 2000, false, true, true));
        assert!(!queue_continuation_due(&state, 2000, true, false, true));
    }

    /// #1935: an incomplete traversal left in a hidden window advances on
    /// the continuation clock, not on the 600 s tick. Before the fix the
    /// clock skipped dispatch entirely while hidden, so a 200-PR queue
    /// moved two pages per full tick -- about half an hour.
    #[tokio::test(start_paused = true)]
    async fn a_hidden_incomplete_traversal_steps_at_the_continuation_interval() {
        let hidden = Arc::new(AtomicBool::new(false));
        let state = std::sync::Mutex::new(crate::queue_scan::State {
            started_at: Some(0),
            eligible_at: 0,
            ..Default::default()
        });
        let started = tokio::time::Instant::now();
        let steps = std::sync::Mutex::new(Vec::<Duration>::new());
        let run = run_queue_continuations_observed(
            hidden.clone(),
            hidden,
            Arc::new(AtomicBool::new(true)),
            |list, visible| {
                let elapsed = started.elapsed();
                let now = elapsed.as_secs() as i64;
                let mut s = state.lock().unwrap();
                // The production adapter's decision, with the flags the
                // clock handed over.
                if list == CachedList::Authored
                    && queue_continuation_due(&s, now, visible, visible, true)
                {
                    steps.lock().unwrap().push(elapsed);
                    // What `github/scan.rs` does after a clean step.
                    s.eligible_at = now + 15;
                }
                async {}
            },
            None,
        );
        assert!(tokio::time::timeout(Duration::from_secs(61), run)
            .await
            .is_err());
        let steps = steps.into_inner().unwrap();
        assert_eq!(
            steps,
            (1..=4)
                .map(|n| CONTINUATION_INTERVAL * n)
                .collect::<Vec<_>>(),
            "a hidden incomplete traversal must step every {CONTINUATION_INTERVAL:?}, \
             not wait for the {:?} hidden tick",
            interval_for(false)
        );
        assert!(CONTINUATION_INTERVAL < interval_for(false));
    }

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
        assert!(!should_surface(e.is_transient(), 1), "one blip is weather");
        assert!(
            should_surface(e.is_transient(), FAILURES_BEFORE_BANNER),
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

    /// Every Jira preference survives a write and a read (#2034), in the
    /// wire shape the frontend sends.
    #[test]
    fn jira_prefs_round_trip() {
        use crate::jira::model::{ExpectedCategory, StatusExpectation, StatusExpectations};
        let json = r#"{"hidden_views":[],"close_hides_to_tray":true,
            "jira_enabled":true,"jira_excluded_projects":["XY"],
            "jira_chips_on_prs":false,
            "jira_status_expectations":{"ABC":{
                "on_open":{"kind":"status","name":"In Review"},
                "on_merged":{"kind":"category","category":"done"}}}}"#;
        let p: UiPrefs = serde_json::from_str(json).unwrap();
        assert!(p.jira_enabled);
        assert_eq!(p.jira_excluded_projects, ["XY"]);
        assert!(!p.jira_chips_on_prs);
        assert_eq!(
            p.jira_status_expectations["ABC"],
            StatusExpectations {
                on_open: Some(StatusExpectation::Status {
                    name: "In Review".into()
                }),
                on_merged: Some(StatusExpectation::Category {
                    category: ExpectedCategory::Done
                }),
            }
        );
        let back: UiPrefs = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    /// A row stored before the Jira fields existed takes their defaults:
    /// chips ON, no expectations (so drift behaves as it always did), and
    /// a project entry with one side set leaves the other unset.
    #[test]
    fn jira_prefs_missing_fields_take_the_defaults() {
        let p: UiPrefs =
            serde_json::from_str(r#"{"hidden_views":[],"close_hides_to_tray":true}"#).unwrap();
        assert!(!p.jira_enabled);
        assert!(p.jira_chips_on_prs, "a missing field must not hide chips");
        assert!(p.jira_status_expectations.is_empty());
        assert_eq!(
            p,
            UiPrefs {
                hidden_views: Vec::new(),
                ..UiPrefs::default()
            }
        );

        let half: UiPrefs = serde_json::from_str(
            r#"{"hidden_views":[],"close_hides_to_tray":true,
                "jira_status_expectations":{"ABC":{"on_open":{"kind":"category","category":"new"}}}}"#,
        )
        .unwrap();
        assert_eq!(half.jira_status_expectations["ABC"].on_merged, None);
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
        assert!(
            !should_surface(e.is_transient(), 1),
            "one blip must stay quiet"
        );
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
        assert_eq!(tick_state(&source_poll::Phase::Retrying), "retrying");
    }

    /// A surfaced failure DOES end the tick idle: `poll-error` is already on
    /// screen, and a second simultaneous signal would have the bar competing
    /// with its own banner.
    #[test]
    fn a_surfaced_failure_leaves_the_banner_to_speak() {
        assert_eq!(tick_state(&source_poll::Phase::Failed), "idle");
    }

    /// A successful tick is genuinely idle -- the guard above must not be so
    /// broad that the bar can never return to green.
    #[test]
    fn a_successful_tick_is_idle() {
        assert_eq!(tick_state(&source_poll::Phase::Ready), "idle");
    }

    /// But a real outage must not be hidden. Two in a row stops being
    /// weather.
    #[test]
    fn repeated_transient_failures_do_surface() {
        let e = ClientError::Timeout(90);
        assert!(should_surface(e.is_transient(), FAILURES_BEFORE_BANNER));
        assert!(should_surface(e.is_transient(), FAILURES_BEFORE_BANNER + 5));
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
                should_surface(e.is_transient(), 1),
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
                !should_surface(e.is_transient(), consecutive),
                "an isolated blip must never surface"
            );
            consecutive = 0; // the success arm
        }

        // Two back to back, with no success between them, does surface.
        consecutive += 1;
        assert!(!should_surface(e.is_transient(), consecutive));
        consecutive += 1;
        assert!(should_surface(e.is_transient(), consecutive));
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

    #[tokio::test(start_paused = true)]
    async fn selected_review_queue_publishes_before_slow_authored_work() {
        let published = std::sync::Mutex::new(Vec::new());
        let started = tokio::time::Instant::now();
        let (authored_ok, reviewing_ok) = prioritized_pair(
            true,
            async |budget| {
                assert_eq!(published.lock().unwrap().as_slice(), &["reviewing"]);
                assert_eq!(budget, Duration::from_secs(25));
                tokio::time::sleep(budget).await;
                false
            },
            async |budget| {
                assert_eq!(budget, FETCH_TIMEOUT);
                tokio::time::sleep(Duration::from_secs(20)).await;
                published.lock().unwrap().push("reviewing");
                true
            },
        )
        .await;
        assert_eq!((authored_ok, reviewing_ok), (Some(false), Some(true)));
        assert_eq!(started.elapsed(), TICK_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn selected_authored_queue_keeps_its_full_budget() {
        let published = std::sync::Mutex::new(Vec::new());
        let (authored_ok, reviewing_ok) = prioritized_pair(
            false,
            async |budget| {
                assert_eq!(budget, FETCH_TIMEOUT);
                tokio::time::sleep(Duration::from_secs(30)).await;
                published.lock().unwrap().push("authored");
                true
            },
            async |budget| {
                assert_eq!(published.lock().unwrap().as_slice(), &["authored"]);
                assert_eq!(budget, Duration::from_secs(15));
                false
            },
        )
        .await;
        assert_eq!((authored_ok, reviewing_ok), (Some(true), Some(false)));
    }

    /// #1932: when the first list used up the tick, the second is not sent.
    /// Sending it would spend a rate-limit point on a request that times out
    /// at once, and that timeout would read as GitHub failing to answer.
    /// The callback is never invoked, so no attempt begins and no status
    /// moves -- in either order.
    #[tokio::test(start_paused = true)]
    async fn the_second_list_is_not_asked_when_the_tick_is_spent() {
        for reviewing_first in [false, true] {
            for spent in [TICK_TIMEOUT, TICK_TIMEOUT - MIN_USEFUL_FETCH_BUDGET / 2] {
                let second_invoked = std::cell::Cell::new(false);
                let (authored, reviewing) = prioritized_pair(
                    reviewing_first,
                    async |_budget| {
                        if reviewing_first {
                            second_invoked.set(true);
                        } else {
                            tokio::time::advance(spent).await;
                        }
                        "authored"
                    },
                    async |_budget| {
                        if reviewing_first {
                            tokio::time::advance(spent).await;
                        } else {
                            second_invoked.set(true);
                        }
                        "reviewing"
                    },
                )
                .await;
                assert!(
                    !second_invoked.get(),
                    "reviewing_first={reviewing_first} spent={spent:?}: the second list was asked"
                );
                if reviewing_first {
                    assert_eq!((authored, reviewing), (None, Some("reviewing")));
                } else {
                    assert_eq!((authored, reviewing), (Some("authored"), None));
                }
            }
        }
    }

    /// The other side of the boundary: a second list with a useful budget
    /// left is still asked, so the skip cannot swallow ordinary ticks.
    #[tokio::test(start_paused = true)]
    async fn the_second_list_is_asked_with_a_useful_budget_left() {
        let spent = TICK_TIMEOUT - MIN_USEFUL_FETCH_BUDGET;
        let (authored, reviewing) = prioritized_pair(
            false,
            async |_budget| {
                tokio::time::advance(spent).await;
                "authored"
            },
            async |budget| {
                assert_eq!(budget, MIN_USEFUL_FETCH_BUDGET);
                "reviewing"
            },
        )
        .await;
        assert_eq!((authored, reviewing), (Some("authored"), Some("reviewing")));
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
            observation: None,
            source: Default::default(),
            id: "PR_test".into(),
            number,
            title: "Add retry to the fetch client".into(),
            url: format!("https://github.com/{repo}/pull/{number}"),
            repo: repo.into(),
            head_ref: "feature/x".into(),
            head_oid: "deadbeef".into(),
            head_ref_id: None,
            head_repo: None,
            base_ref: "main".into(),
            author: "octocat".into(),
            is_draft: false,
            created_at: Utc::now(),
            ready_at: None,
            updated_at: Utc::now(),
            ci: CiState::Success,
            merge,
            merge_status: MergeStateStatus::Clean,
            review: ReviewState::Approved,
            in_merge_queue: false,
            labels: Vec::<Label>::new(),
            comment_count: 0,
            unresolved_threads: 0,
            unresolved_threads_floor: false,
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

    /// Preserve the historically measured query shape. This guard prevents
    /// unpriced connection expansion; it does not measure current GraphQL cost
    /// or bound aggregate traffic from continuations/advisories/detail/actions.
    #[test]
    fn queue_query_keeps_historically_measured_connection_shape() {
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
        // was included in the historical document measurement
        // recorded beside PRS_QUERY. Anything else nested here is REFUSED until somebody
        // measures it -- which is the whole inversion #842 asks for.
        const APPROVED: [&str; 8] = [
            // #1407's ready-for-review time. Measured free: 2 before and
            // 2 after, see the measurement beside it in `PRS_QUERY`.
            "timelineItems(",
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
                 -- then record the measurement and add the name here. \
                 (`commits(` was missing from the previous guard and is 1 of the \
                 2 points: measured 2 -> 1 with it removed, 2026-09-11.)"
            );
        }
        // And the approved connections must still BE there: a deny-list
        // refuses additions but cannot notice a removal, and removing one
        // would change the historically measured shape.
        for c in APPROVED {
            assert!(
                nodes.contains(c),
                "`{c}` is approved and measured but no longer in the document; \
                 record the changed query shape and re-measure its cost"
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
                "the suppression lives in `announce_against`'s baseline, not here"
            );
        }

        /// A receipt as the poll loop accepts it: rows, coverage, and the
        /// checkpoint state that says whether the list was ever validated.
        fn receipt(rows: Vec<PullRequest>, complete: bool, validated: bool) -> FetchedList {
            FetchedList {
                viewer: Some("me".into()),
                total: Some(rows.len() as u64 + 10),
                prs: rows,
                coverage: if complete {
                    Coverage::Complete
                } else {
                    Coverage::Partial { total: None }
                },
                scan: Some(crate::queue_scan::Commit {
                    expected_revision: 0,
                    state: crate::queue_scan::State {
                        completed_at: validated.then_some(1000),
                        ..Default::default()
                    },
                    removals: vec![],
                }),
            }
        }

        /// #1939: two Partial receipts of a list already validated end to
        /// end. The second carries a pull request the first did not, in
        /// the head page both fetched. Partial is not nothing: it is news.
        #[test]
        fn a_partial_receipt_announces_what_it_actually_received() {
            let mut baseline = None;
            let first = receipt(vec![open(1), open(2)], false, true);
            assert!(
                announce_against(&mut baseline, &first, newly_appeared).is_empty(),
                "the first tick stays silent"
            );
            let second = receipt(vec![open(3), open(1), open(2)], false, true);
            let out = announce_against(&mut baseline, &second, newly_appeared);
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].number, 3);
            // The same rows again announce nothing: no duplicates.
            let third = receipt(vec![open(3), open(1), open(2)], false, true);
            assert!(announce_against(&mut baseline, &third, newly_appeared).is_empty());
        }

        /// The ready-to-review notification follows the same baseline.
        #[test]
        fn a_partial_review_receipt_announces_a_newly_ready_pull_request() {
            let ready = |n| pr_full("o/r", n, CiState::Success, MergeState::Mergeable);
            let mut baseline = None;
            announce_against(
                &mut baseline,
                &receipt(vec![ready(1)], false, true),
                newly_ready,
            );
            let out = announce_against(
                &mut baseline,
                &receipt(vec![ready(2), ready(1)], false, true),
                newly_ready,
            );
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].kind, BreakageKind::ReadyToReview);
            assert_eq!(out[0].number, 2);
        }

        /// Before the first validated traversal, a pull request missing
        /// from the last receipt may sit on a page not fetched yet. That
        /// is possibly wrong, so it is suppressed -- and a Complete receipt
        /// then becomes the baseline.
        #[test]
        fn an_unvalidated_partial_list_is_no_baseline() {
            let mut baseline = None;
            announce_against(
                &mut baseline,
                &receipt(vec![open(1)], false, false),
                newly_appeared,
            );
            let page_two = receipt(vec![open(1), open(26)], false, false);
            assert!(
                announce_against(&mut baseline, &page_two, newly_appeared).is_empty(),
                "a page fetched for the first time is not news"
            );
            let complete = receipt(vec![open(1), open(26), open(51)], true, false);
            assert!(announce_against(&mut baseline, &complete, newly_appeared).is_empty());
            let next = receipt(vec![open(60), open(1), open(26), open(51)], false, true);
            let out = announce_against(&mut baseline, &next, newly_appeared);
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].number, 60);
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
            observation: None,
            source: Default::default(),
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
            head_repo: None,
            base_ref: "main".into(),
            created_at: t,
            ready_at: Some(t),
            updated_at: t,
            ci,
            merge,
            merge_status: MergeStateStatus::Clean,
            review: crate::github::model::ReviewState::None,
            in_merge_queue: false,
            labels: vec![],
            comment_count: 0,
            unresolved_threads: 0,
            unresolved_threads_floor: false,
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
    fn notifications_and_rechecks_do_not_alias_provider_or_host() {
        let gh = pr_full("o/r", 7, CiState::Success, MergeState::Mergeable);
        for (provider, host) in [
            (crate::identity::Provider::Gitlab, "gitlab.com"),
            (crate::identity::Provider::Github, "other.example"),
        ] {
            let mut other = gh.clone();
            other.source = Source {
                provider,
                host: host.into(),
            };
            other.ci = CiState::Failure;
            assert!(
                newly_broken(std::slice::from_ref(&gh), std::slice::from_ref(&other)).is_empty()
            );
            assert_eq!(
                newly_appeared(std::slice::from_ref(&gh), std::slice::from_ref(&other))[0].source,
                other.source
            );
            let merged = crate::inventory::reconcile(
                vec![gh.clone()],
                vec![other],
                false,
                chrono::Utc::now(),
            );
            assert!(merged
                .iter()
                .any(|row| row.identity() == gh.identity() && row.ci == gh.ci));
        }
    }
}
