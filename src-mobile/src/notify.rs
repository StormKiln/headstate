//! What the phone tells the user, and when (#789).
//!
//! Two things, both decided here and both delivered as LOCAL
//! notifications from inside the background refresh window
//! (`background.rs`): a pull request that has APPEARED, and the paired
//! desktop's health going bad or recovering.
//!
//! # Everything that decides anything is pure
//!
//! [`newly_appeared`] and [`health_transitions`] are comparisons of a
//! previous list against a current one. They touch no clock, no store
//! and no notification API -- the same discipline `poll::newly_broken`
//! and `health::alerts::evaluate` follow on the desktop, and for the
//! same reason: "a first sync must not fire twenty notifications" is
//! testable as a function call rather than by installing the app.
//!
//! # First-sync suppression, and why it is the OPPOSITE of the battery
//! decision
//!
//! A brand-new pull request is, by definition, absent from the previous
//! list -- which makes "absent before, present now" the whole rule and
//! also makes the FIRST sync look like twenty brand-new pull requests.
//! A burst of twenty notifications on install is the worst possible
//! introduction to a feature, and it is the thing that gets
//! notifications switched off for good.
//!
//! So [`Previous::First`] exists and announces nothing. It is not an
//! optimisation; it is the difference between a usable feature and one
//! that is disabled within a minute of being installed.
//!
//! The desktop's battery alerts deliberately do the opposite
//! (`lib.rs`: a relaunch re-arms every condition, in memory rather than
//! in SQLite). Both are right, and the difference is what the
//! notification CLAIMS:
//!
//! - A battery alert is about a STANDING CONDITION. "Your battery is at
//!   18%" is still true and still worth acting on after a relaunch, so
//!   restating it once is a service. The user has just opened the app,
//!   and the cost of the restatement is one notification.
//! - A new-PR notification is about an EVENT -- the moment something
//!   appeared. Re-announcing it after a reinstall would claim an event
//!   that did not happen: the pull request was already there, and the
//!   only thing that changed is that this app had forgotten it.
//!
//! An event cannot be re-armed, because re-arming it fabricates the
//! event. That is the rule, and it is why the two sides of the app
//! treat a fresh start differently.
//!
//! # Health notifications NAME THE MACHINE
//!
//! The companion reaches the desktop through the remote surface, so
//! health data on the phone is the DESKTOP's health. An unqualified
//! "Battery low" on a phone reads as a claim about the phone -- which is
//! both wrong and actively misleading, since the phone's own battery is
//! the one the user can see in the status bar.
//!
//! Every health title here therefore carries the desktop's name, from
//! the pairing record: "Mac mini: battery at 18%". That is why
//! [`health_transitions`] takes a machine name and why it is not
//! optional.
//!
//! # The rules live on the desktop; only the verdict crosses
//!
//! The phone asks `health_alerts` (#789's one new command) and receives
//! conditions the desktop has already evaluated -- key, title, body. It
//! holds no copy of any threshold. The alternative was fetching
//! `system_health_history` and running `health::alerts` and
//! `health::runaway` here, which would put a second implementation of
//! every threshold in a separate crate with a separate lockfile, drifting
//! silently while both test suites stayed green. For a rule whose whole
//! job is deciding when to interrupt someone, a drifted copy is worse
//! than no rule.
//!
//! # Dedup reuses the desktop's own mechanism
//!
//! The phone keeps a [`Fired`] set of alert KEYS -- exactly the
//! transition-only discipline `health::alerts::Fired` provides -- rather
//! than inventing a second one. The keys ARE the desktop's own
//! `Alert::key` strings, so the two sides agree on what "the same
//! condition" means by construction rather than by two implementations
//! happening to match.
//!
//! A separate INSTANCE, though, because the two devices have separately
//! said separate things: the desktop's sampler has already notified at
//! the desktop, and a phone that inherited its set would stay silent
//! about a condition nobody at the phone had been told about.
//!
//! # The delivery tradeoff
//!
//! Best-effort. iOS decides when a refresh window opens, so a new pull
//! request surfaces within the hour rather than instantly. See
//! `tauri-plugin-headstate-notify`'s module docs; the UI copy says so
//! too, because a user who expects instant and gets hourly concludes
//! the feature is broken.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// One pull request, as much of it as a notification needs.
///
/// A narrow local type rather than the desktop's `PullRequest`: the
/// phone receives `get_cached`'s JSON verbatim and never parses it
/// (`events.rs` hands it to the webview as a `RawValue`), so there is no
/// shared struct to borrow. Four fields are what a notification body
/// contains, and decoding only those means a desktop that adds a field
/// does not break the phone's notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub repo: String,
    pub number: u64,
    pub title: String,
    /// Whether the pull request is a draft.
    ///
    /// `#[serde(default)]` so a payload without the key decodes as
    /// "not a draft" rather than failing the whole list -- one unknown
    /// shape must not silence every notification.
    #[serde(default)]
    pub is_draft: bool,
}

impl Pr {
    /// The identity a notification is deduplicated on.
    ///
    /// `(repo, number)` is the same identity the desktop's
    /// `merge_by_identity` and `newly_broken` use. Not the GraphQL node
    /// id, which is also unique: the pair is what the body prints, so
    /// keying on it means the thing compared and the thing shown cannot
    /// drift apart.
    fn identity(&self) -> (&str, u64) {
        (&self.repo, self.number)
    }
}

/// What the phone knew before this sync.
///
/// An enum rather than an `Option<Vec<Pr>>` because the two cases are
/// not "some list or none" -- they are "a list to compare against" and
/// "nothing to compare against, so announce nothing". An `Option` read
/// as an empty list is exactly the first-sync burst this type exists to
/// prevent, and `None.unwrap_or_default()` is one character away from
/// writing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    /// No sync has ever completed on this install. Announce NOTHING.
    First,
    /// The list as of the previous sync.
    Known(Vec<Pr>),
}

/// Pull requests that have just appeared.
///
/// The shape of `poll::newly_ready` and `poll::newly_broken`: a pure
/// comparison of the previous list against the current one, returning
/// what changed rather than what is true.
///
/// Drafts are excluded. A draft is work in progress that the author has
/// explicitly marked as not ready to be looked at, and announcing its
/// creation is announcing someone's intention to start -- the desktop
/// makes the same judgement in `ready_for_review`. It appearing is not
/// news; it leaving draft would be, and that is a transition this
/// release does not detect.
///
/// [`Previous::First`] returns empty, which is the first-sync
/// suppression the module docs are about.
pub fn newly_appeared(previous: &Previous, current: &[Pr]) -> Vec<Pr> {
    let Previous::Known(previous) = previous else {
        return Vec::new();
    };
    let known: HashSet<(&str, u64)> = previous.iter().map(Pr::identity).collect();
    current
        .iter()
        .filter(|pr| !pr.is_draft)
        .filter(|pr| !known.contains(&pr.identity()))
        .cloned()
        .collect()
}

/// The notification for one newly-appeared pull request.
///
/// Title is the pull request's own title, body is `owner/repo#123`,
/// matching `poll::notify_breakage` on the desktop so the two platforms
/// read the same way. No machine name: a pull request is not a fact
/// about a computer, and prefixing one would imply the pull request
/// belongs to that desktop rather than to GitHub.
pub fn appeared_notification(pr: &Pr) -> (String, String) {
    (
        pr.title.clone(),
        format!("{}#{} just appeared", pr.repo, pr.number),
    )
}

/// One health condition on the desktop, as the phone learns it.
///
/// Mirrors the desktop's `health::AlertReport`, which is what
/// `health_alerts` returns. The RULES run on the desktop and only the
/// verdict crosses -- the phone holds no copy of any threshold, because
/// a second implementation of "when is a battery worth interrupting
/// someone for" in a separate crate with a separate lockfile would drift
/// while both test suites stayed green.
///
/// `key` is the desktop's own `Alert::key` string, so the two sides agree
/// on what "the same condition" means without a shared enum. `title` and
/// `body` are the desktop's own wording, for the same reason -- the phone
/// restating a condition in its own words would be two descriptions of
/// one fault.
///
/// Unknown extra fields are ignored by serde's default, so a desktop
/// that adds one does not stop the phone notifying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthAlert {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// What the phone has already said about the desktop's health.
///
/// The same transition-only discipline as `health::alerts::Fired`, and
/// deliberately the same SHAPE: a set of keys, with a condition that
/// clears removing its key so a second episode is news again. It is a
/// separate instance rather than a shared one because the desktop's
/// `Fired` lives on the desktop -- what is shared is the key vocabulary
/// (`Alert::key`), which is the part that must not drift.
///
/// In memory, like the desktop's. A phone relaunch re-arms, and here
/// that IS the right call, for exactly the reason the module docs give
/// for the battery: a health alert is a standing condition, and one
/// restatement of "your Mac is still unwell" after the app reopens is a
/// service rather than a fabricated event.
#[derive(Debug, Default, Clone)]
pub struct Fired {
    keys: HashSet<String>,
}

impl Fired {
    /// The conditions in `present` that are NEW, re-arming any that have
    /// stopped being true.
    ///
    /// One call does both halves, for the reason
    /// `health::alerts::Fired::take_new` gives: a caller that suppressed
    /// but forgot to re-arm would announce each condition once per app
    /// lifetime, which is worse than announcing it repeatedly -- the
    /// user would conclude the feature does not work.
    pub fn take_new(&mut self, present: &[HealthAlert]) -> Vec<HealthAlert> {
        let now: HashSet<&str> = present.iter().map(|a| a.key.as_str()).collect();
        self.keys.retain(|key| now.contains(key.as_str()));
        present
            .iter()
            .filter(|a| self.keys.insert(a.key.clone()))
            .cloned()
            .collect()
    }
}

/// The health conditions worth telling the user about, with the machine
/// named.
///
/// `machine` is the paired desktop's name from the pairing record. NOT
/// optional and not defaulted: the whole point is that an unqualified
/// health alert on a phone reads as a claim about the phone. A caller
/// with no name has a bug to fix rather than a fallback to take.
///
/// Pure: `fired` is passed in and mutated, so the dedup state has one
/// home and this function stays a statement about the data.
pub fn health_transitions(
    fired: &mut Fired,
    present: &[HealthAlert],
    machine: &str,
) -> Vec<(String, String)> {
    fired
        .take_new(present)
        .into_iter()
        .map(|a| (format!("{machine}: {}", a.title), a.body))
        .collect()
}

/// Which notifications the phone should send.
///
/// # Why the phone keeps its OWN preferences
///
/// The desktop's `NotifyPrefs` is `Class::Local` on the remote surface
/// (`surface.rs`), so the phone cannot read or write it -- and that
/// classification is correct: which notifications a desktop shows on a
/// desktop is a decision made at that desktop. It is also the right
/// answer on the merits. The two devices are in different places: a
/// person may well want CI failures on the laptop they are working at
/// and only new pull requests on the phone in their pocket, and one
/// shared setting could not express that.
///
/// So these live in the phone's own store and cover only what the phone
/// sends. The field names match the desktop's where the categories
/// match, so the two structs read as the same vocabulary.
///
/// Defaults to everything ON, matching the desktop's `NotifyPrefs` and
/// for the same reason -- except that on the phone "on" still means
/// nothing happens until the user grants permission, which is the
/// ask-once prompt in `tauri-plugin-headstate-notify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneNotifyPrefs {
    /// The master switch. A separate field rather than a third state of
    /// each category, so turning notifications off does not lose the
    /// per-category choices underneath it -- the desktop's `enabled`
    /// makes the same choice for the same reason.
    pub enabled: bool,
    /// A pull request appearing that was not there before.
    #[serde(default = "yes")]
    pub new_pr: bool,
    /// The desktop's battery going low, draining fast, or draining on
    /// AC.
    ///
    /// One category for all three, not three. They are one subject to a
    /// person -- "something is wrong with that machine's power" -- and
    /// the desktop already treats them as one (`battery_low_percent` is
    /// the only knob; the other two have no threshold to set).
    #[serde(default = "yes")]
    pub health_battery: bool,
    /// The desktop's CPU being busy with nothing in particular (#791).
    ///
    /// Its own category rather than folded into `health_battery`,
    /// because the two answer different questions about a machine you
    /// left running: "is it about to die" and "is it burning a core for
    /// no reason". Someone who wants the first and not the second is
    /// making a reasonable choice.
    #[serde(default = "yes")]
    pub health_cpu: bool,
    /// Claude Code sessions on the desktop: a turn finished, a session
    /// is waiting for input or permission, or a turn errored (#1486).
    /// The global switch; each session can also be muted on its own
    /// ([`SessionMutes`]).
    #[serde(default = "yes")]
    pub sessions: bool,
    /// Whether a session notification shows the session's opening
    /// prompt under its state (#1486). OFF by default, and `serde`'s
    /// plain default for a stored preference written before it existed:
    /// the lock screen names the project and the state and nothing
    /// else unless the owner asks for more.
    #[serde(default)]
    pub session_snippet: bool,
}

/// Serde needs a function, not a literal, for a defaulted bool. Named
/// `yes` rather than `default_true` because it reads as the answer at
/// each use site.
fn yes() -> bool {
    true
}

impl Default for PhoneNotifyPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            new_pr: true,
            health_battery: true,
            health_cpu: true,
            sessions: true,
            session_snippet: false,
        }
    }
}

impl PhoneNotifyPrefs {
    /// Whether a newly-appeared pull request should be announced.
    pub fn wants_new_pr(&self) -> bool {
        self.enabled && self.new_pr
    }

    /// Whether session transitions should be announced at all (#1486).
    pub fn wants_sessions(&self) -> bool {
        self.enabled && self.sessions
    }

    /// Whether a health condition with this key should be announced.
    ///
    /// Matches on the desktop's `Alert::key` strings. An UNKNOWN key --
    /// a condition a newer desktop has and this phone does not know
    /// about -- is allowed through under `enabled`, deliberately. The
    /// alternative is silence about a fault the desktop thought worth
    /// reporting, and "a category you cannot yet switch off" is a much
    /// smaller problem than "a warning you never received". The user can
    /// still turn everything off with the master switch.
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

/// Store key for [`PhoneNotifyPrefs`].
pub const PREFS_KEY: &str = "notify_prefs";

/// Store key for the pull-request identities the last sync saw.
///
/// Separate from the `snapshot` key (`events.rs`) even though both hold
/// a PR list, and that separation is the point. The snapshot is the
/// LIST, kept so the app opens with something to show and overwritten
/// by every event frame the subscriber receives while the app is in the
/// foreground. This is the set of identities the last NOTIFICATION pass
/// compared against.
///
/// Sharing one key would break new-PR detection in a way that is hard
/// to see: the foreground subscriber would keep overwriting the
/// "previous" list with the current one, so by the time a background
/// window ran, every pull request would already be in "previous" and
/// nothing would ever be new. Two keys, two purposes.
pub const SEEN_KEY: &str = "notify_seen";

/// The identities the last notification pass saw.
///
/// Identities rather than whole pull requests: the only question asked
/// of this is "was this one here last time", and storing titles and
/// draft flags would mean a title edit rewrote the record for no
/// behavioural difference. It also keeps the stored value small, which
/// matters in a Stronghold vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub v: u32,
    /// `["owner/repo#123", ...]`.
    ///
    /// One string per pull request rather than a `(repo, number)` tuple
    /// so the stored JSON is readable when someone is working out why a
    /// notification did or did not fire.
    pub prs: Vec<String>,
}

const SEEN_VERSION: u32 = 1;

impl Seen {
    /// The identities of `prs`, in the order given.
    pub fn of(prs: &[Pr]) -> Self {
        Self {
            v: SEEN_VERSION,
            prs: prs
                .iter()
                .map(|pr| format!("{}#{}", pr.repo, pr.number))
                .collect(),
        }
    }

    /// Back into the comparable form [`newly_appeared`] takes.
    ///
    /// Titles are not stored, so the reconstructed entries carry empty
    /// ones. That is sound because only [`Pr::identity`] is read on the
    /// `previous` side of the comparison -- and the title that ends up
    /// in a notification always comes from the CURRENT list, which is a
    /// freshly fetched one.
    pub fn as_previous(&self) -> Previous {
        Previous::Known(
            self.prs
                .iter()
                .filter_map(|id| {
                    let (repo, number) = id.rsplit_once('#')?;
                    Some(Pr {
                        repo: repo.to_string(),
                        number: number.parse().ok()?,
                        title: String::new(),
                        is_draft: false,
                    })
                })
                .collect(),
        )
    }
}

/// What [`decode_prs`] made of a `get_cached` payload.
///
/// # Why this is not just an empty `Vec`
///
/// It was, and that was a latent burst. "The desktop has no open pull
/// requests" and "this build cannot read the desktop's list" are
/// OPPOSITE answers, and the caller does different things with them: the
/// first is a real list to remember, the second must leave the stored
/// record alone.
///
/// Collapsed into an empty `Vec`, the unreadable case overwrote the seen
/// set with nothing -- and the next window that COULD read the payload
/// saw every pull request as new and fired one notification each. That is
/// exactly the failure [`Previous::First`] exists to prevent, arriving by
/// the back door, and an empty `Vec` is one `unwrap_or_default()` away
/// from reintroducing it. Hence a type the caller cannot ignore.
///
/// Same reasoning as `health::Sample`'s "absent is not zero" rule, and
/// the same reasoning that makes [`Previous`] an enum rather than an
/// `Option<Vec<Pr>>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// The list, which may legitimately be empty.
    List(Vec<Pr>),
    /// The payload was not a list this build can read.
    Unreadable,
}

/// Decode the pull requests out of a `get_cached` payload.
///
/// Lenient about INDIVIDUAL entries and strict about the shape: an entry
/// missing the fields [`Pr`] needs is skipped, but a payload that is not
/// JSON, or not an array, or whose every entry was skipped, is
/// [`Decoded::Unreadable`] -- see that type on why the distinction is
/// load-bearing rather than tidy.
///
/// Never an error: the phone's notifications are an affordance, and a
/// desktop whose list shape this build does not recognise must cost the
/// user notifications rather than a failed refresh window. The snapshot
/// that window also stores is handed to the webview verbatim and is
/// unaffected either way.
///
/// Logged when it cannot read a non-empty payload, so "the phone stopped
/// notifying after a desktop upgrade" is answerable from a log rather
/// than by guessing.
pub fn decode_prs(json: &str) -> Decoded {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        log::debug!("notify: the cached list is not JSON");
        return Decoded::Unreadable;
    };
    let Some(array) = value.as_array() else {
        log::debug!("notify: the cached list is not an array");
        return Decoded::Unreadable;
    };
    let prs: Vec<Pr> = array
        .iter()
        .filter_map(|v| serde_json::from_value::<Pr>(v.clone()).ok())
        .collect();
    // A genuinely empty list is a real answer and is remembered as one.
    // Entries that were ALL skipped is not: the desktop said it had
    // pull requests and this build could not read any of them.
    if prs.is_empty() && !array.is_empty() {
        log::debug!(
            "notify: none of the {} cached entries carried repo/number/title",
            array.len()
        );
        return Decoded::Unreadable;
    }
    Decoded::List(prs)
}

/// Decode the conditions out of a `health_alerts` payload.
///
/// Lenient for the same reason [`decode_prs`] is: a shape this build
/// does not recognise costs the user notifications, not a failed refresh
/// window. Each entry is decoded on its own, so one unreadable condition
/// does not silence the readable ones beside it.
pub fn decode_health(json: &str) -> Vec<HealthAlert> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        log::debug!("notify: the health report is not JSON; no notifications from it");
        return Vec::new();
    };
    let Some(array) = value.as_array() else {
        log::debug!("notify: the health report is not an array; no notifications from it");
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|v| serde_json::from_value::<HealthAlert>(v.clone()).ok())
        .collect()
}

/// The live [`Notifier`](crate::background::Notifier): the phone's own
/// store for the preferences and the seen set, and the notification
/// plugin for the posting.
///
/// Holds the health [`Fired`] set in memory for the life of the process.
/// Not persisted, deliberately, and for the reason the module docs give:
/// a health alert is a STANDING CONDITION, so one restatement of "your
/// Mac is still unwell" after the app reopens is a service. A new-PR
/// notification is an event and is suppressed across a fresh install
/// instead -- the two are treated differently because they claim
/// different things.
pub struct PhoneNotifier<R: tauri::Runtime> {
    companion: std::sync::Arc<crate::companion::Companion>,
    app: tauri::AppHandle<R>,
    fired: std::sync::Arc<std::sync::Mutex<Fired>>,
}

impl<R: tauri::Runtime> PhoneNotifier<R> {
    pub fn new(
        companion: std::sync::Arc<crate::companion::Companion>,
        app: tauri::AppHandle<R>,
    ) -> Self {
        Self {
            companion,
            app,
            fired: std::sync::Arc::new(std::sync::Mutex::new(Fired::default())),
        }
    }
}

impl<R: tauri::Runtime> crate::background::Notifier for PhoneNotifier<R> {
    fn prefs(&self) -> PhoneNotifyPrefs {
        self.companion.notify_prefs()
    }

    fn seen(&self) -> Previous {
        self.companion.notify_seen()
    }

    fn remember(&self, prs: &[Pr]) -> Result<(), String> {
        self.companion.record_notify_seen(prs)
    }

    fn machine(&self) -> Option<String> {
        self.companion.desktop_name()
    }

    fn post(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_headstate_notify::HeadstateNotifyExt;
        self.app
            .headstate_notify()
            .post(&tauri_plugin_headstate_notify::Notification {
                title: title.to_string(),
                body: body.to_string(),
                session: None,
            })
            .map_err(|e| e.to_string())
    }

    fn health_fired(&self) -> std::sync::Arc<std::sync::Mutex<Fired>> {
        self.fired.clone()
    }

    fn sessions_seen(&self) -> SessionsPrevious {
        self.companion.sessions_seen()
    }

    fn remember_sessions(&self, seen: &SessionsSeen) -> Result<(), String> {
        self.companion.record_sessions_seen(seen)
    }

    fn forget_sessions(&self) -> Result<(), String> {
        self.companion.forget_sessions_seen()
    }

    fn session_mutes(&self) -> SessionMutes {
        self.companion.session_mutes()
    }

    fn post_session(&self, title: &str, body: &str, session: &str) -> Result<(), String> {
        use tauri_plugin_headstate_notify::HeadstateNotifyExt;
        self.app
            .headstate_notify()
            .post(&tauri_plugin_headstate_notify::Notification {
                title: title.to_string(),
                body: body.to_string(),
                session: Some(session.to_string()),
            })
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------
// Claude Code sessions (#1486)
// ---------------------------------------------------------------------
//
// # What is announced, and from what
//
// The desktop's `claude_session_digest` answers, per session, an id, a
// project label, whether it is waiting on the user NOW (and since when),
// and the last turn's end time and outcome -- and NO transcript text
// (`claude::digest` on the desktop pins that). Three transitions come out
// of comparing it against what the last pass saw:
//
// - **finished**: a turn ended, completed, later than the last one seen.
// - **now waiting**: a wait began later than the last one seen, for
//   input (`idle_prompt`) or for a permission (`permission_prompt`).
// - **errored**: a turn ended in a `StopFailure`.
//
// Every one is keyed on a TIMESTAMP the desktop stated, and announced
// only when that timestamp moves forward. That is what makes it one
// notification per transition rather than one per poll: a session that
// has been waiting for an hour carries the same `since` on every pass,
// and a time that did not move is not news.
//
// # Absence is never a transition
//
// A session missing from the digest, a `last_turn` of `None`, a
// `waiting` of `None`: none of them announces anything, and none of them
// erases what was remembered. `None` is "the desktop could not say", not
// "it stopped" -- a session that exits, or drops out of the bounded
// digest, is not news, and forgetting its marks would make its last
// event look new when it came back.
//
// # First sync, and sessions this phone has never seen
//
// [`SessionsPrevious::First`] announces nothing, as [`Previous::First`]
// does for pull requests and for the same reason. After that, a session
// with no marks is compared against the previous pass's `as_of` -- the
// DESKTOP's clock at that pass -- so an event from before the phone last
// looked is history, and one after it is news.
//
// # One notification per session per pass
//
// A background window may find a session that finished a turn AND is now
// waiting for input, because `idle_prompt` follows the end of a turn.
// Those are one story, so one notification, chosen by
// [`SessionState::rank`]: errored, then waiting for a permission, then
// finished, then waiting for input. Every mark still advances.
//
// # Delivery is best-effort
//
// These ride the same background refresh window as #789's, which iOS
// grants when it chooses. The settings say "best-effort, delivery can be
// delayed by iOS" where this is switched on. Instant delivery needs an
// APNs relay, which is #1492.

/// One session as the digest states it: what a transition needs, and
/// nothing else. Decoded leniently -- unknown fields from a newer
/// desktop are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DigestSession {
    pub session_id: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub waiting: Option<DigestWaiting>,
    #[serde(default)]
    pub last_turn: Option<DigestTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DigestWaiting {
    pub kind: String,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DigestTurn {
    pub ended_at: String,
    pub outcome: DigestOutcome,
}

/// How a turn ended. An outcome this build does not know decodes as
/// [`DigestOutcome::Other`] rather than failing the row, and announces
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum DigestOutcome {
    Completed,
    Failed {
        #[serde(default)]
        error_type: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// The digest, as the phone reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Digest {
    pub as_of: String,
    pub sessions: Vec<DigestSession>,
}

/// Decode a `claude_session_digest` answer. `None` when it is not one --
/// which the caller treats as "leave the marks alone", never as an
/// empty digest (the back-door burst [`Decoded`] documents).
pub fn decode_digest(json: &str) -> Option<Digest> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let as_of = value.get("as_of")?.as_str()?.to_string();
    let rows = value.get("sessions")?.as_array()?;
    let sessions = rows
        .iter()
        .filter_map(|v| serde_json::from_value::<DigestSession>(v.clone()).ok())
        .collect();
    Some(Digest { as_of, sessions })
}

/// What happened to one session, as the notification states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// A turn ended and completed.
    Finished,
    /// Waiting for the user's next message (`idle_prompt`).
    WaitingForInput,
    /// Waiting for the user to allow something (`permission_prompt`).
    WaitingForPermission,
    /// A turn ended in a failure; Claude Code's `error_type` code.
    Errored(Option<String>),
}

impl SessionState {
    /// Which of two states for one session in one pass is announced.
    /// Higher wins: a failure is the most urgent thing to know, a
    /// permission prompt blocks work until answered, and "finished" says
    /// more than the "waiting for input" that follows every finished
    /// turn.
    fn rank(&self) -> u8 {
        match self {
            SessionState::Errored(_) => 4,
            SessionState::WaitingForPermission => 3,
            SessionState::Finished => 2,
            SessionState::WaitingForInput => 1,
        }
    }

    /// The state, as the lock screen says it.
    pub fn phrase(&self) -> String {
        match self {
            SessionState::Finished => "finished".into(),
            SessionState::WaitingForInput => "waiting for your input".into(),
            SessionState::WaitingForPermission => "waiting for your permission".into(),
            SessionState::Errored(Some(code)) => {
                format!("stopped with an error ({})", code.replace('_', " "))
            }
            SessionState::Errored(None) => "stopped with an error".into(),
        }
    }
}

/// One transition to announce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    pub session_id: String,
    pub project: Option<String>,
    pub state: SessionState,
}

/// What the phone remembers about one session between passes: the
/// newest event times it has seen. Times, not states, because the
/// transitions are "a time moved forward".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMarks {
    #[serde(default)]
    pub waiting_since: Option<String>,
    #[serde(default)]
    pub turn_ended_at: Option<String>,
    /// Whether the turn at `turn_ended_at` was seen as failed. Lets a
    /// failure recorded a moment after the idle transition correct a
    /// "finished" into "errored" without the time having moved.
    #[serde(default)]
    pub turn_failed: bool,
}

/// Store key for [`SessionsSeen`].
pub const SESSIONS_SEEN_KEY: &str = "notify_sessions_seen";

/// What the last session pass saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsSeen {
    pub v: u32,
    /// The digest's `as_of` at that pass: the DESKTOP's clock.
    pub as_of: String,
    pub sessions: std::collections::BTreeMap<String, SessionMarks>,
}

const SESSIONS_SEEN_VERSION: u32 = 1;

/// [`Previous`], for sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionsPrevious {
    /// No session pass has completed since install, or since sessions
    /// were last switched off. Announce NOTHING.
    First,
    Known(SessionsSeen),
}

/// Sessions the owner muted on this phone, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMutes {
    pub sessions: std::collections::BTreeSet<String>,
}

/// Store key for [`SessionMutes`].
pub const SESSION_MUTES_KEY: &str = "notify_session_mutes";

/// The result of one pass: what to announce, and what to remember.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransitions {
    pub announce: Vec<SessionEvent>,
    pub next: SessionsSeen,
}

fn instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Whether `current` is strictly later than `before`. An unparseable
/// `current` is never news; an absent or unparseable `before` is nothing
/// to compare against, so any readable `current` is later than it. The
/// caller decides what `before` is -- a session's own mark, or the
/// previous pass's clock for a session with none.
fn later(current: &str, before: Option<&str>) -> bool {
    let Some(now) = instant(current) else {
        return false;
    };
    match before.and_then(instant) {
        Some(then) => now > then,
        None => true,
    }
}

/// The later of two optional stamps, by time. An unparseable one loses.
fn newest(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => match (instant(a), instant(b)) {
            (Some(ta), Some(tb)) => Some(if tb > ta { b } else { a }.to_string()),
            (Some(_), None) => Some(a.to_string()),
            (None, Some(_)) => Some(b.to_string()),
            (None, None) => Some(a.to_string()),
        },
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

/// The session transitions between the last pass and this digest.
///
/// Pure: no store, no clock, no notification API. `muted` sessions are
/// left out of `announce` and their marks STILL advance, so unmuting one
/// announces what happens next rather than what happened while it was
/// muted.
pub fn session_transitions(
    previous: &SessionsPrevious,
    digest: &Digest,
    muted: &SessionMutes,
) -> SessionTransitions {
    let (known, floor) = match previous {
        SessionsPrevious::First => (None, None),
        SessionsPrevious::Known(seen) => (Some(&seen.sessions), Some(seen.as_of.as_str())),
    };
    let mut announce = Vec::new();
    let mut next = std::collections::BTreeMap::new();
    for row in &digest.sessions {
        let before = known.and_then(|k| k.get(&row.session_id));
        // A session with marks compares against them; one without is
        // compared against the previous pass's clock.
        let bar = |mark: Option<&String>| -> Option<String> {
            match before {
                Some(_) => mark.cloned(),
                None => floor.map(str::to_string),
            }
        };
        let mut candidates: Vec<SessionState> = Vec::new();

        if let Some(w) = &row.waiting {
            let prev = bar(before.and_then(|b| b.waiting_since.as_ref()));
            if later(&w.since, prev.as_deref()) {
                match w.kind.as_str() {
                    "permission_prompt" => candidates.push(SessionState::WaitingForPermission),
                    "idle_prompt" => candidates.push(SessionState::WaitingForInput),
                    // A kind this build does not know is not announced:
                    // the desktop only states `Now` for the two prompt
                    // types, so this is a newer desktop's vocabulary,
                    // and a notification that cannot say what it is
                    // waiting for is worse than none.
                    _ => {}
                }
            }
        }
        if let Some(t) = &row.last_turn {
            let prev = bar(before.and_then(|b| b.turn_ended_at.as_ref()));
            let failed = matches!(t.outcome, DigestOutcome::Failed { .. });
            let moved = later(&t.ended_at, prev.as_deref());
            // The same turn, first seen as completed and now as failed:
            // the failure landed after the pass that saw the idle write.
            let corrected = before.is_some_and(|b| {
                failed && !b.turn_failed && b.turn_ended_at.as_deref() == Some(t.ended_at.as_str())
            });
            if moved || corrected {
                match &t.outcome {
                    DigestOutcome::Completed if moved => candidates.push(SessionState::Finished),
                    DigestOutcome::Failed { error_type } => {
                        candidates.push(SessionState::Errored(error_type.clone()))
                    }
                    _ => {}
                }
            }
        }

        if known.is_some() && !muted.sessions.contains(&row.session_id) {
            if let Some(state) = candidates.into_iter().max_by_key(SessionState::rank) {
                announce.push(SessionEvent {
                    session_id: row.session_id.clone(),
                    project: row.project.clone(),
                    state,
                });
            }
        }

        // Marks only ever move FORWARD, and absence keeps what was known.
        let prev = before.cloned().unwrap_or_default();
        let (turn_ended_at, turn_failed) = match &row.last_turn {
            Some(t) if later(&t.ended_at, prev.turn_ended_at.as_deref()) => (
                Some(t.ended_at.clone()),
                matches!(t.outcome, DigestOutcome::Failed { .. }),
            ),
            Some(t) if prev.turn_ended_at.as_deref() == Some(t.ended_at.as_str()) => (
                prev.turn_ended_at.clone(),
                prev.turn_failed || matches!(t.outcome, DigestOutcome::Failed { .. }),
            ),
            _ => (prev.turn_ended_at.clone(), prev.turn_failed),
        };
        next.insert(
            row.session_id.clone(),
            SessionMarks {
                waiting_since: newest(
                    prev.waiting_since.as_deref(),
                    row.waiting.as_ref().map(|w| w.since.as_str()),
                ),
                turn_ended_at,
                turn_failed,
            },
        );
    }
    // Sessions absent from this digest keep their marks: absence is not
    // a transition, and forgetting would make their last event news when
    // they reappear. Bounded by what the desktop's digest has ever
    // returned in the recency window; pruned when the marks are older
    // than the digest's own window.
    if let Some(known) = known {
        let horizon = instant(&digest.as_of).map(|t| t - chrono::TimeDelta::hours(48));
        for (id, marks) in known {
            if next.contains_key(id) {
                continue;
            }
            let newest_mark = newest(
                marks.waiting_since.as_deref(),
                marks.turn_ended_at.as_deref(),
            );
            let stale = match (horizon, newest_mark.as_deref().and_then(instant)) {
                (Some(h), Some(t)) => t < h,
                // No time to judge by: nothing to announce from it later
                // either, so keeping it buys nothing.
                (_, None) => true,
                (None, Some(_)) => false,
            };
            if !stale {
                next.insert(id.clone(), marks.clone());
            }
        }
    }
    SessionTransitions {
        announce,
        next: SessionsSeen {
            v: SESSIONS_SEEN_VERSION,
            as_of: digest.as_of.clone(),
            sessions: next,
        },
    }
}

/// Longest opening-prompt snippet shown on a lock screen, in characters.
pub const SNIPPET_CHARS: usize = 100;

/// The notification for one session transition: the project as the
/// title and the state as the body, and nothing else unless the owner
/// turned the snippet on.
///
/// "hello-world" / "Waiting for your input". Never the session's name:
/// that is Claude Code's `aiTitle`, written from the conversation, and
/// the digest does not carry it for exactly that reason.
pub fn session_notification(event: &SessionEvent, snippet: Option<&str>) -> (String, String) {
    let title = event
        .project
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "Claude Code session".to_string());
    let mut body = capitalise(&event.state.phrase());
    if let Some(s) = snippet.map(str::trim).filter(|s| !s.is_empty()) {
        body.push('\n');
        body.push_str(&clamp_chars(s, SNIPPET_CHARS));
    }
    (title, body)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// At most `max` characters, with an ellipsis when cut. By `char`, so a
/// multi-byte character is never split.
fn clamp_chars(s: &str, max: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The opening prompt out of a `claude_transcript_opening_prompt`
/// answer, or `None` for anything else.
pub fn decode_prompt(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.get("prompt")?.as_str().map(str::to_string)
}

/// Serialises the read-compare-write of the session marks, so the
/// foreground toast poll and a background window cannot both announce
/// one transition. Held for no `.await`: the digest is fetched before it
/// is taken.
static SESSIONS_PASS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Compare a digest against the stored marks, store the new marks, and
/// return what to announce. The one place both delivery paths go
/// through, so a transition toasted in the foreground is not notified
/// again from the background, or the other way round.
pub fn advance_sessions<N: crate::background::Notifier + ?Sized>(
    notifier: &N,
    digest: &Digest,
) -> Vec<SessionEvent> {
    let _guard = SESSIONS_PASS.lock().unwrap_or_else(|e| e.into_inner());
    let t = session_transitions(&notifier.sessions_seen(), digest, &notifier.session_mutes());
    if let Err(e) = notifier.remember_sessions(&t.next) {
        // As for pull requests: a store that keeps failing makes every
        // pass a first sync, which announces nothing -- the feature is
        // silently dead, so this is a warning.
        log::warn!("notify: could not record what the session pass saw: {e}");
    }
    t.announce
}

/// One in-app toast (#1486): a transition that happened while the app
/// was open, for a session other than the one on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionToast {
    pub session_id: String,
    pub title: String,
    pub body: String,
}

/// The foreground half: the same transitions as a background window,
/// returned to the webview instead of posted. `viewing` is left out --
/// the owner is looking at it -- but its marks still advance.
pub async fn foreground_toasts<N, D>(
    notifier: &N,
    desktop: &D,
    viewing: Option<&str>,
) -> Result<Vec<SessionToast>, String>
where
    N: crate::background::Notifier + ?Sized,
    D: crate::background::Desktop + ?Sized,
{
    let prefs = notifier.prefs();
    if !prefs.wants_sessions() {
        return Ok(Vec::new());
    }
    let json = desktop.session_digest().await?;
    let Some(digest) = decode_digest(&json) else {
        return Err("the desktop's session status could not be read".into());
    };
    Ok(advance_sessions(notifier, &digest)
        .into_iter()
        .filter(|e| Some(e.session_id.as_str()) != viewing)
        .map(|e| {
            let (title, body) = session_notification(&e, None);
            SessionToast {
                session_id: e.session_id,
                title,
                body,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------
// Commands the Settings UI drives
// ---------------------------------------------------------------------

/// The phone's own notification preferences.
///
/// Its own command pair rather than reaching the desktop's
/// `get_notify_prefs`, which is `Class::Local` and correctly so: which
/// notifications a desktop shows is a decision made at that desktop. See
/// [`PhoneNotifyPrefs`] on why one shared setting could not express what
/// a person actually wants from two devices in two places.
#[tauri::command]
pub fn get_phone_notify_prefs(
    state: tauri::State<'_, std::sync::Arc<crate::companion::Companion>>,
) -> PhoneNotifyPrefs {
    state.notify_prefs()
}

#[tauri::command]
pub fn set_phone_notify_prefs(
    state: tauri::State<'_, std::sync::Arc<crate::companion::Companion>>,
    prefs: PhoneNotifyPrefs,
) -> Result<(), String> {
    // Counts only -- which repositories or machines are involved is not
    // a setting and is never logged (CONTRIBUTING, check-privacy.sh).
    log::info!(
        "notify: enabled={} new_pr={} battery={} cpu={} sessions={} snippet={}",
        prefs.enabled,
        prefs.new_pr,
        prefs.health_battery,
        prefs.health_cpu,
        prefs.sessions,
        prefs.session_snippet
    );
    state.set_notify_prefs(&prefs)
}

/// The sessions muted on this phone (#1486), by id.
#[tauri::command]
pub fn get_session_mutes(
    state: tauri::State<'_, std::sync::Arc<crate::companion::Companion>>,
) -> Vec<String> {
    state.session_mutes().sessions.into_iter().collect()
}

/// Mute or unmute one session's notifications on this phone (#1486).
///
/// A muted session's marks keep advancing, so unmuting it announces
/// what happens NEXT rather than what happened while it was muted.
#[tauri::command]
pub fn set_session_muted(
    state: tauri::State<'_, std::sync::Arc<crate::companion::Companion>>,
    session_id: String,
    muted: bool,
) -> Result<(), String> {
    // Never the id: which sessions are muted is not worth a log line.
    log::info!(
        "notify: a session was {}",
        if muted { "muted" } else { "unmuted" }
    );
    state.set_session_muted(&session_id, muted)
}

/// The in-app toasts for session transitions while the app is open
/// (#1486). `viewing` is the session on screen, which gets none.
#[tauri::command]
pub async fn poll_session_toasts<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, std::sync::Arc<crate::companion::Companion>>,
    viewing: Option<String>,
) -> Result<Vec<SessionToast>, String> {
    let companion = state.inner().clone();
    let notifier = PhoneNotifier::new(companion.clone(), app);
    foreground_toasts(&notifier, companion.as_ref(), viewing.as_deref()).await
}

/// The session of the notification the owner last tapped, cleared as it
/// is read (#1486). `None` on a platform with no notifications.
#[tauri::command]
pub fn take_notification_session<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Option<String> {
    use tauri_plugin_headstate_notify::HeadstateNotifyExt;
    match app.headstate_notify().take_tapped() {
        Ok(session) => session,
        Err(tauri_plugin_headstate_notify::Error::Unavailable) => None,
        Err(e) => {
            log::info!("notify: could not read the tapped notification: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(repo: &str, number: u64, title: &str) -> Pr {
        Pr {
            repo: repo.to_string(),
            number,
            title: title.to_string(),
            is_draft: false,
        }
    }

    fn draft(repo: &str, number: u64) -> Pr {
        Pr {
            is_draft: true,
            ..pr(repo, number, "work in progress")
        }
    }

    /// **The first-sync suppression test, and the one that matters
    /// most.**
    ///
    /// A fresh install syncs and finds twenty pull requests. Every one
    /// of them is absent from "previous", so the naive rule announces
    /// twenty. That burst is what gets notifications turned off for
    /// good, within a minute of the feature being installed.
    #[test]
    fn a_first_sync_announces_nothing() {
        let current: Vec<Pr> = (1..=20)
            .map(|n| pr("octocat/hello-world", n, "x"))
            .collect();
        assert!(
            newly_appeared(&Previous::First, &current).is_empty(),
            "a first sync must never fire a burst"
        );
    }

    /// And the other half: once there IS a previous list, a genuinely
    /// new pull request is announced. A rule that never fires is as
    /// useless as one that always does.
    #[test]
    fn a_pull_request_that_appears_is_announced() {
        let before = Previous::Known(vec![pr("octocat/hello-world", 1, "a")]);
        let after = vec![
            pr("octocat/hello-world", 1, "a"),
            pr("octocat/hello-world", 2, "Add a spoon"),
        ];
        let new = newly_appeared(&before, &after);
        assert_eq!(new.len(), 1, "{new:?}");
        assert_eq!(new[0].number, 2);
        let (title, body) = appeared_notification(&new[0]);
        assert_eq!(title, "Add a spoon");
        assert!(body.contains("octocat/hello-world#2"), "{body}");
    }

    /// An empty previous list is NOT a first sync. A user whose last
    /// pull request merged has an empty list, and the next one they open
    /// is news.
    #[test]
    fn an_empty_previous_list_is_not_a_first_sync() {
        let new = newly_appeared(
            &Previous::Known(vec![]),
            &[pr("octocat/hello-world", 7, "x")],
        );
        assert_eq!(new.len(), 1, "{new:?}");
    }

    /// A list that has not changed announces nothing, however many
    /// times it is compared. The standing-condition rule, applied to a
    /// list.
    #[test]
    fn an_unchanged_list_announces_nothing() {
        let list = vec![pr("octocat/hello-world", 1, "a"), pr("o/r", 2, "b")];
        assert!(newly_appeared(&Previous::Known(list.clone()), &list).is_empty());
    }

    /// A pull request that DISAPPEARED -- merged, closed -- announces
    /// nothing. Only appearances are in scope; the desktop's own
    /// transitions cover the rest.
    #[test]
    fn a_closed_pull_request_announces_nothing() {
        let before = Previous::Known(vec![pr("o/r", 1, "a"), pr("o/r", 2, "b")]);
        assert!(newly_appeared(&before, &[pr("o/r", 1, "a")]).is_empty());
    }

    /// The same number in two repositories is two pull requests. Keying
    /// on the number alone would silence the second one.
    #[test]
    fn the_identity_is_the_repo_and_the_number() {
        let before = Previous::Known(vec![pr("octocat/hello-world", 7, "a")]);
        let new = newly_appeared(&before, &[pr("octocat/spoon-knife", 7, "b")]);
        assert_eq!(new.len(), 1, "{new:?}");
        assert_eq!(new[0].repo, "octocat/spoon-knife");
    }

    /// A re-titled pull request is not a new one. The identity is the
    /// pair, not the title.
    #[test]
    fn a_retitled_pull_request_is_not_new() {
        let before = Previous::Known(vec![pr("o/r", 7, "the old title")]);
        assert!(newly_appeared(&before, &[pr("o/r", 7, "the new title")]).is_empty());
    }

    /// A draft appearing is not news: the author has explicitly said it
    /// is not ready to be looked at.
    #[test]
    fn a_new_draft_is_not_announced() {
        let before = Previous::Known(vec![pr("o/r", 1, "a")]);
        assert!(newly_appeared(&before, &[pr("o/r", 1, "a"), draft("o/r", 2)]).is_empty());
    }

    // ---- The round trip through the store ----------------------------

    /// What is stored and read back is the identity set, and it compares
    /// equal to the list it came from -- so a pass that stores the
    /// current list sees nothing new on the next pass.
    #[test]
    fn the_seen_set_round_trips_and_announces_nothing_twice() {
        let list = vec![pr("octocat/hello-world", 1347, "a"), pr("o/r", 2, "b")];
        let seen = Seen::of(&list);
        assert_eq!(seen.prs, vec!["octocat/hello-world#1347", "o/r#2"]);
        let json = serde_json::to_string(&seen).unwrap();
        let back: Seen = serde_json::from_str(&json).unwrap();
        assert!(newly_appeared(&back.as_previous(), &list).is_empty());
    }

    /// A repository name containing a `#` would break a naive split.
    /// GitHub does not allow one, but `rsplit_once` is what makes the
    /// parse correct regardless -- and a malformed entry is skipped
    /// rather than failing the whole set.
    #[test]
    fn a_malformed_identity_is_skipped_rather_than_fatal() {
        let seen = Seen {
            v: 1,
            prs: vec![
                "octocat/hello-world#7".into(),
                "nonsense".into(),
                "o/r#notanumber".into(),
            ],
        };
        let Previous::Known(prs) = seen.as_previous() else {
            panic!("Known");
        };
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 7);
    }

    // ---- Health -----------------------------------------------------

    fn alert(key: &str, title: &str) -> HealthAlert {
        HealthAlert {
            key: key.to_string(),
            title: title.to_string(),
            body: "because reasons".to_string(),
        }
    }

    /// **The copy requirement.** A health notification on a phone is
    /// about the DESKTOP, and the title must say so -- otherwise it
    /// reads as a claim about the phone the user is holding, whose
    /// battery they can see in the status bar.
    #[test]
    fn a_health_notification_names_the_machine() {
        let mut fired = Fired::default();
        let out = health_transitions(&mut fired, &[alert("low", "Battery at 18%")], "Mac mini");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Mac mini: Battery at 18%");
        assert!(
            out[0].0.starts_with("Mac mini"),
            "the machine comes first, so it is readable in a truncated banner: {}",
            out[0].0
        );
    }

    /// A standing condition is announced ONCE, not on every window.
    #[test]
    fn a_standing_health_condition_is_announced_once() {
        let mut fired = Fired::default();
        let present = [alert("low", "Battery at 18%")];
        assert_eq!(health_transitions(&mut fired, &present, "Mac").len(), 1);
        assert!(health_transitions(&mut fired, &present, "Mac").is_empty());
        assert!(health_transitions(&mut fired, &present, "Mac").is_empty());
    }

    /// ...and re-arms once it clears, so a second episode is news.
    #[test]
    fn a_cleared_health_condition_re_arms() {
        let mut fired = Fired::default();
        let present = [alert("low", "Battery at 18%")];
        assert_eq!(health_transitions(&mut fired, &present, "Mac").len(), 1);
        assert!(health_transitions(&mut fired, &[], "Mac").is_empty());
        assert_eq!(
            health_transitions(&mut fired, &present, "Mac").len(),
            1,
            "after clearing, the condition is news again"
        );
    }

    /// The key is the condition, not the wording. A battery falling
    /// through the twenties changes the title on every reading and must
    /// not notify on every one.
    #[test]
    fn a_changing_title_is_still_one_condition() {
        let mut fired = Fired::default();
        assert_eq!(
            health_transitions(&mut fired, &[alert("low", "Battery at 24%")], "Mac").len(),
            1
        );
        for percent in [23, 22, 21] {
            let title = format!("Battery at {percent}%");
            assert!(
                health_transitions(&mut fired, &[alert("low", &title)], "Mac").is_empty(),
                "{title} re-notified"
            );
        }
    }

    /// Two different conditions are two notifications, and each is
    /// deduplicated on its own.
    #[test]
    fn two_conditions_are_tracked_separately() {
        let mut fired = Fired::default();
        let both = [alert("low", "Battery at 18%"), alert("diffuse_cpu", "Busy")];
        assert_eq!(health_transitions(&mut fired, &both, "Mac").len(), 2);
        // One clears, the other stands: nothing new either way.
        assert!(health_transitions(&mut fired, &both[..1], "Mac").is_empty());
        // And the cleared one is news again.
        assert_eq!(health_transitions(&mut fired, &both, "Mac").len(), 1);
    }

    // ---- Preferences -------------------------------------------------

    #[test]
    fn everything_is_on_by_default() {
        let d = PhoneNotifyPrefs::default();
        assert!(d.wants_new_pr());
        assert!(d.wants_health("low"));
        assert!(d.wants_health("diffuse_cpu"));
    }

    #[test]
    fn the_master_switch_silences_every_category() {
        let off = PhoneNotifyPrefs {
            enabled: false,
            ..Default::default()
        };
        assert!(!off.wants_new_pr());
        assert!(!off.wants_health("low"));
        assert!(!off.wants_health("diffuse_cpu"));
        assert!(
            !off.wants_health("a_condition_from_the_future"),
            "including ones this build does not know"
        );
    }

    /// The categories are independent: the three battery conditions are
    /// one subject, CPU is another.
    #[test]
    fn the_health_categories_are_independent() {
        let no_cpu = PhoneNotifyPrefs {
            health_cpu: false,
            ..Default::default()
        };
        assert!(no_cpu.wants_health("low"));
        assert!(no_cpu.wants_health("fast_discharge"));
        assert!(no_cpu.wants_health("draining_on_ac"));
        assert!(!no_cpu.wants_health("diffuse_cpu"));

        let no_battery = PhoneNotifyPrefs {
            health_battery: false,
            ..Default::default()
        };
        assert!(!no_battery.wants_health("low"));
        assert!(!no_battery.wants_health("draining_on_ac"));
        assert!(no_battery.wants_health("diffuse_cpu"));
    }

    /// An unknown key from a newer desktop is allowed through: a
    /// category you cannot yet switch off is a much smaller problem
    /// than a warning you never received.
    #[test]
    fn an_unknown_condition_is_not_silently_dropped() {
        let d = PhoneNotifyPrefs::default();
        assert!(d.wants_health("thermal_critical"));
        let no_cpu = PhoneNotifyPrefs {
            health_cpu: false,
            ..Default::default()
        };
        assert!(no_cpu.wants_health("thermal_critical"));
    }

    /// A stored preference written before a field existed still
    /// decodes. Without `serde(default)` a missing key would fail the
    /// whole struct and silently reset every other choice -- the exact
    /// failure the desktop's `NotifyPrefs` documents.
    #[test]
    fn an_older_stored_preference_still_decodes() {
        let p: PhoneNotifyPrefs = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert!(!p.enabled);
        assert!(p.new_pr, "a missing field is the default, not false");
        assert!(p.health_battery);
        assert!(p.health_cpu);
        assert!(p.sessions, "session notifications default on");
        assert!(
            !p.session_snippet,
            "the lock-screen snippet defaults OFF, including for a stored preference \
             written before it existed"
        );
    }

    // ---- Decoding ----------------------------------------------------

    fn list(json: &str) -> Vec<Pr> {
        match decode_prs(json) {
            Decoded::List(prs) => prs,
            Decoded::Unreadable => panic!("expected a readable list: {json}"),
        }
    }

    #[test]
    fn the_cached_list_decodes_to_the_fields_a_notification_needs() {
        let json = r#"[
            {"repo":"octocat/hello-world","number":1347,"title":"Add a spoon","isDraft":false,"ci":"success"},
            {"repo":"octocat/hello-world","number":1348,"title":"Remove a fork","is_draft":true}
        ]"#;
        let prs = list(json);
        assert_eq!(prs.len(), 2, "{prs:?}");
        assert_eq!(prs[0].number, 1347);
        assert_eq!(prs[0].title, "Add a spoon");
        // The desktop serialises `is_draft` in snake_case (serde's
        // default for the field name), which is what the second entry
        // uses; the first proves an unknown extra field is ignored.
        assert!(prs[1].is_draft);
    }

    /// Absent `is_draft` is "not a draft", not a failed decode. One
    /// unknown shape must not silence every notification.
    #[test]
    fn a_missing_draft_flag_is_not_a_draft() {
        let prs = list(r#"[{"repo":"o/r","number":1,"title":"x"}]"#);
        assert_eq!(prs.len(), 1);
        assert!(!prs[0].is_draft);
    }

    /// A payload this build cannot read is `Unreadable`, not an empty
    /// list.
    #[test]
    fn an_unreadable_payload_is_not_an_empty_list() {
        assert_eq!(decode_prs("not json"), Decoded::Unreadable);
        assert_eq!(decode_prs(r#"{"prs": []}"#), Decoded::Unreadable);
        assert_eq!(decode_prs(r#"[{"unexpected": true}]"#), Decoded::Unreadable);
    }

    /// **The distinction that prevents a burst.**
    ///
    /// "The desktop has no open pull requests" and "this build cannot
    /// read the desktop's list" are OPPOSITE answers. A genuinely empty
    /// array is a real list and is remembered as one; a non-empty array
    /// whose every entry was skipped is not, because remembering it as
    /// empty would wipe the seen set -- and the next window that COULD
    /// read the payload would see every pull request as new and fire one
    /// notification each.
    ///
    /// That is the first-sync burst arriving by the back door, which is
    /// why these two cases must not collapse into the same value.
    #[test]
    fn an_empty_list_is_a_real_answer_but_all_entries_skipped_is_not() {
        assert_eq!(
            decode_prs("[]"),
            Decoded::List(vec![]),
            "no open pull requests is a real list, and must be remembered"
        );
        assert_eq!(
            decode_prs(r#"[{"shape":"from a newer desktop"},{"also":"unreadable"}]"#),
            Decoded::Unreadable,
            "the desktop said it had two; we could read neither, so we know nothing"
        );
        // A partially readable payload IS a list: one unreadable entry
        // among readable ones is a skipped notification, not an unknown
        // list.
        assert_eq!(
            decode_prs(r#"[{"shape":"unknown"},{"repo":"o/r","number":1,"title":"x"}]"#),
            Decoded::List(vec![Pr {
                repo: "o/r".into(),
                number: 1,
                title: "x".into(),
                is_draft: false,
            }])
        );
    }

    /// `health_alerts` returns the desktop's own wording, which the phone
    /// passes through rather than restating.
    #[test]
    fn the_health_report_decodes_to_key_title_and_body() {
        let json = r#"[
            {"key":"low","title":"Battery at 18%","body":"Charge has fallen below 25%."},
            {"key":"diffuse_cpu","title":"CPU is busy with nothing in particular","body":"About 71%..."}
        ]"#;
        let alerts = decode_health(json);
        assert_eq!(alerts.len(), 2, "{alerts:?}");
        assert_eq!(alerts[0].key, "low");
        assert_eq!(alerts[0].title, "Battery at 18%");
        assert_eq!(alerts[1].key, "diffuse_cpu");
    }

    /// One unreadable condition does not silence the readable ones
    /// beside it, and an unreadable report costs notifications rather
    /// than the window.
    #[test]
    fn one_unreadable_condition_does_not_silence_the_rest() {
        let json = r#"[{"nonsense":1},{"key":"low","title":"t","body":"b"}]"#;
        let alerts = decode_health(json);
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].key, "low");
        assert!(decode_health("not json").is_empty());
        assert!(decode_health(r#"{"alerts":[]}"#).is_empty());
    }

    /// A condition a newer desktop carries extra fields on still
    /// decodes: serde ignores what it does not know, which is what keeps
    /// a desktop upgrade from silencing the phone.
    #[test]
    fn extra_fields_from_a_newer_desktop_are_ignored() {
        let alerts = decode_health(r#"[{"key":"low","title":"t","body":"b","severity":"warn"}]"#);
        assert_eq!(alerts.len(), 1);
    }

    // ---- Sessions (#1486) ---------------------------------------------

    fn session(id: &str) -> DigestSession {
        DigestSession {
            session_id: id.into(),
            project: Some("hello-world".into()),
            waiting: None,
            last_turn: None,
        }
    }

    fn waiting(mut s: DigestSession, kind: &str, since: &str) -> DigestSession {
        s.waiting = Some(DigestWaiting {
            kind: kind.into(),
            since: since.into(),
        });
        s
    }

    fn turn(mut s: DigestSession, ended_at: &str, failed: Option<&str>) -> DigestSession {
        s.last_turn = Some(DigestTurn {
            ended_at: ended_at.into(),
            outcome: match failed {
                Some(code) => DigestOutcome::Failed {
                    error_type: Some(code.into()),
                },
                None => DigestOutcome::Completed,
            },
        });
        s
    }

    fn digest(as_of: &str, sessions: Vec<DigestSession>) -> Digest {
        Digest {
            as_of: as_of.into(),
            sessions,
        }
    }

    /// Run one pass against the marks the previous one left, as the
    /// phone does.
    fn pass(prev: &mut SessionsPrevious, d: &Digest, muted: &SessionMutes) -> Vec<SessionEvent> {
        let t = session_transitions(prev, d, muted);
        *prev = SessionsPrevious::Known(t.next);
        t.announce
    }

    fn states(events: &[SessionEvent]) -> Vec<(String, SessionState)> {
        events
            .iter()
            .map(|e| (e.session_id.clone(), e.state.clone()))
            .collect()
    }

    /// **No burst on first sync.** A fresh install finds ten sessions,
    /// every one of which has finished a turn or is waiting -- and
    /// announces none of them, while remembering all of them so the NEXT
    /// pass has something to compare against.
    #[test]
    fn a_first_session_sync_announces_nothing() {
        let rows: Vec<DigestSession> = (0..10)
            .map(|i| {
                let s = turn(session(&format!("s{i}")), "2026-09-26T11:00:00Z", None);
                waiting(s, "permission_prompt", "2026-09-26T11:01:00Z")
            })
            .collect();
        let mut prev = SessionsPrevious::First;
        let d = digest("2026-09-26T12:00:00Z", rows);
        assert!(pass(&mut prev, &d, &SessionMutes::default()).is_empty());
        // And the same digest again is still nothing: it was remembered.
        assert!(pass(&mut prev, &d, &SessionMutes::default()).is_empty());
    }

    /// **One notification per transition, not per poll.** A session
    /// that finished once and then sits idle for five more polls is
    /// announced once.
    #[test]
    fn a_standing_state_is_announced_once() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1")]),
            &none,
        );
        let finished = digest(
            "2026-09-26T12:10:00Z",
            vec![turn(session("s1"), "2026-09-26T12:05:00Z", None)],
        );
        assert_eq!(
            states(&pass(&mut prev, &finished, &none)),
            vec![("s1".to_string(), SessionState::Finished)]
        );
        for _ in 0..5 {
            assert!(pass(&mut prev, &finished, &none).is_empty(), "re-announced");
        }
        // The NEXT turn is news again.
        let again = digest(
            "2026-09-26T12:30:00Z",
            vec![turn(session("s1"), "2026-09-26T12:25:00Z", None)],
        );
        assert_eq!(pass(&mut prev, &again, &none).len(), 1);
    }

    /// A new wait is news; the same wait, polled again, is not. Input
    /// and permission are different states.
    #[test]
    fn waiting_for_input_and_for_permission_are_announced_per_episode() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1"), session("s2")]),
            &none,
        );
        let d = digest(
            "2026-09-26T12:10:00Z",
            vec![
                waiting(session("s1"), "idle_prompt", "2026-09-26T12:05:00Z"),
                waiting(session("s2"), "permission_prompt", "2026-09-26T12:06:00Z"),
            ],
        );
        assert_eq!(
            states(&pass(&mut prev, &d, &none)),
            vec![
                ("s1".to_string(), SessionState::WaitingForInput),
                ("s2".to_string(), SessionState::WaitingForPermission),
            ]
        );
        assert!(pass(&mut prev, &d, &none).is_empty());
    }

    /// A failed turn is "errored", with Claude Code's code in the copy.
    #[test]
    fn a_failed_turn_is_announced_as_errored() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1")]),
            &none,
        );
        let d = digest(
            "2026-09-26T12:10:00Z",
            vec![turn(
                session("s1"),
                "2026-09-26T12:05:00Z",
                Some("rate_limit"),
            )],
        );
        let events = pass(&mut prev, &d, &none);
        assert_eq!(
            states(&events),
            vec![(
                "s1".to_string(),
                SessionState::Errored(Some("rate_limit".into()))
            )]
        );
        let (_, body) = session_notification(&events[0], None);
        assert_eq!(body, "Stopped with an error (rate limit)");
    }

    /// A failure recorded a moment after the pass that saw the idle
    /// write corrects "finished" into "errored" -- once.
    #[test]
    fn a_late_failure_for_the_same_turn_is_announced_once() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1")]),
            &none,
        );
        let at = "2026-09-26T12:05:00.000Z";
        let first = digest("2026-09-26T12:05:01Z", vec![turn(session("s1"), at, None)]);
        assert_eq!(pass(&mut prev, &first, &none).len(), 1);
        let late = digest(
            "2026-09-26T12:10:00Z",
            vec![turn(session("s1"), at, Some("overloaded"))],
        );
        assert_eq!(
            states(&pass(&mut prev, &late, &none)),
            vec![(
                "s1".to_string(),
                SessionState::Errored(Some("overloaded".into()))
            )]
        );
        assert!(pass(&mut prev, &late, &none).is_empty());
    }

    /// Finished AND now waiting for input, seen in one pass, is one
    /// story and one notification: "finished".
    #[test]
    fn one_session_is_one_notification_per_pass() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1")]),
            &none,
        );
        let s = turn(session("s1"), "2026-09-26T12:05:00Z", None);
        let s = waiting(s, "idle_prompt", "2026-09-26T12:06:00Z");
        let events = pass(&mut prev, &digest("2026-09-26T12:10:00Z", vec![s]), &none);
        assert_eq!(
            states(&events),
            vec![("s1".to_string(), SessionState::Finished)]
        );
    }

    /// **Mute honoured.** A muted session announces nothing, its marks
    /// still advance, and unmuting it announces what happens NEXT rather
    /// than what happened while it was muted.
    #[test]
    fn a_muted_session_is_silent_and_unmuting_is_not_a_backlog() {
        let mut prev = SessionsPrevious::First;
        let muted = SessionMutes {
            sessions: ["s1".to_string()].into_iter().collect(),
        };
        let none = SessionMutes::default();
        pass(
            &mut prev,
            &digest("2026-09-26T12:00:00Z", vec![session("s1"), session("s2")]),
            &muted,
        );
        let d = digest(
            "2026-09-26T12:10:00Z",
            vec![
                turn(session("s1"), "2026-09-26T12:05:00Z", None),
                turn(session("s2"), "2026-09-26T12:05:00Z", None),
            ],
        );
        assert_eq!(
            states(&pass(&mut prev, &d, &muted)),
            vec![("s2".to_string(), SessionState::Finished)],
            "only the unmuted session"
        );
        assert!(
            pass(&mut prev, &d, &none).is_empty(),
            "unmuting must not announce what happened while muted"
        );
    }

    /// Absence is never a transition, and never erases a mark: a session
    /// that drops out of the digest and comes back with the same turn
    /// end is not news.
    #[test]
    fn a_session_that_disappears_and_returns_is_not_news() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        let with = digest(
            "2026-09-26T12:00:00Z",
            vec![turn(session("s1"), "2026-09-26T11:55:00Z", None)],
        );
        pass(&mut prev, &with, &none);
        assert!(pass(&mut prev, &digest("2026-09-26T12:10:00Z", vec![]), &none).is_empty());
        let back = digest(
            "2026-09-26T12:20:00Z",
            vec![turn(session("s1"), "2026-09-26T11:55:00Z", None)],
        );
        assert!(pass(&mut prev, &back, &none).is_empty());
        // `last_turn: None` does not erase the mark either.
        pass(
            &mut prev,
            &digest("2026-09-26T12:30:00Z", vec![session("s1")]),
            &none,
        );
        assert!(pass(&mut prev, &back, &none).is_empty());
    }

    /// A session this phone has never seen is compared against the
    /// previous pass's clock: an event after it is news, one before it
    /// is history.
    #[test]
    fn a_new_session_is_judged_against_the_last_pass() {
        let mut prev = SessionsPrevious::First;
        let none = SessionMutes::default();
        pass(&mut prev, &digest("2026-09-26T12:00:00Z", vec![]), &none);
        let d = digest(
            "2026-09-26T12:10:00Z",
            vec![
                turn(session("new"), "2026-09-26T12:05:00Z", None),
                turn(session("old"), "2026-09-26T11:00:00Z", None),
            ],
        );
        assert_eq!(
            states(&pass(&mut prev, &d, &none)),
            vec![("new".to_string(), SessionState::Finished)]
        );
    }

    /// **The lock screen names the project and the state, and nothing
    /// else** -- unless the snippet is on, when the opening prompt
    /// follows, clamped.
    #[test]
    fn the_lock_screen_text_is_project_and_state_only() {
        let e = SessionEvent {
            session_id: "s1".into(),
            project: Some("hello-world".into()),
            state: SessionState::WaitingForInput,
        };
        assert_eq!(
            session_notification(&e, None),
            (
                "hello-world".to_string(),
                "Waiting for your input".to_string()
            )
        );
        let long = "x".repeat(300);
        let (_, body) = session_notification(&e, Some(&long));
        let snippet = body.split_once('\n').unwrap().1;
        assert_eq!(snippet.chars().count(), SNIPPET_CHARS);
        assert!(snippet.ends_with('…'));
        // No project: a generic name, never the session id or a title.
        let anon = SessionEvent { project: None, ..e };
        assert_eq!(session_notification(&anon, None).0, "Claude Code session");
    }

    /// The digest decodes leniently, and an unreadable one is `None`
    /// -- never an empty digest that would wipe the marks.
    #[test]
    fn the_digest_decodes_leniently_and_unreadable_is_not_empty() {
        let json = r#"{"as_of":"2026-09-26T12:00:00Z","total":2,"sessions":[
            {"session_id":"s1","project":"hello-world","liveness":"running",
             "waiting":{"kind":"idle_prompt","since":"2026-09-26T11:59:00Z"},
             "last_turn":{"ended_at":"2026-09-26T11:58:00Z","outcome":{"state":"failed","error_type":"overloaded"}}},
            {"session_id":"s2","liveness":"dead","waiting":null,"last_turn":
             {"ended_at":"2026-09-26T11:00:00Z","outcome":{"state":"from-the-future"}}},
            {"no_id":true}
        ]}"#;
        let d = decode_digest(json).unwrap();
        assert_eq!(d.sessions.len(), 2);
        assert_eq!(
            d.sessions[1].last_turn.as_ref().unwrap().outcome,
            DigestOutcome::Other
        );
        assert_eq!(decode_digest("[]"), None);
        assert_eq!(decode_digest("not json"), None);
        assert_eq!(
            decode_prompt(r#"{"prompt":"fix the flaky test","masking":{"hidden":0}}"#).as_deref(),
            Some("fix the flaky test")
        );
        assert_eq!(decode_prompt(r#"{"prompt":null}"#), None);
    }
}
