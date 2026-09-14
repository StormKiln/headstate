//! Deciding when busy CPU is a FAULT rather than work (#791).
//!
//! Ten orphaned `yes` processes burned a core each for two days on the
//! reporting machine and nothing noticed. This module is the part of
//! #791 that ships: the aggregate rule, plus a shadow log of what the
//! two per-process rules WOULD have said.
//!
//! # Why the obvious rule is not here
//!
//! "100% CPU for five minutes" would have caught those ten and would
//! also fire on every `yarn build`, Vite dev server, `terraform plan`,
//! alembic migration, ollama run and Rust compile on the same machine.
//! That is alert fatigue inside a day, and an ignored alert is worse
//! than no alert. The CPU LEVEL was never the distinguishing signal:
//! duration, orphanhood and doing no work were.
//!
//! # What ships, and what is only logged
//!
//! | tier | rule | this release |
//! |---|---|---|
//! | 3 | total CPU >= [`AGGREGATE_PERCENT`] for >= [`AGGREGATE_MINUTES`] min, no single process explains it | **alerts** |
//! | watch | a process >= [`WATCH_PERCENT`] for >= [`WATCH_MINUTES`] min, not allowlisted (#865) | [`Notice`], page only |
//! | load | load average / cores >= [`OVERSUBSCRIBED_RATIO`] for >= [`OVERSUBSCRIBED_MINUTES`] min (#872) | [`Notice`], page only |
//! | 1 | a process >= [`TIER1_PERCENT`] for >= [`TIER1_MINUTES`] min, PPID 1, not allowlisted | shadow log only |
//! | 2 | a process >= [`TIER2_PERCENT`] for >= [`TIER2_MINUTES`] min | shadow log only |
//!
//! A [`Notice`] is an INDICATOR and an [`Alert`] is an INTERRUPTION, and
//! the two are kept apart on purpose: the watch tier and the load rule
//! both describe conditions that a large build produces legitimately, so
//! notifying on either would be the #853 outcome (~40 false positives,
//! guard turned off). They reach `health_alerts` and the System Health
//! page; nothing converts one into an `Alert`, and two tests assert that
//! rather than leaving it to review.
//!
//! # Why load average is here at all (#872)
//!
//! Because `cpu_percent` SATURATES and load average does not. The mean
//! of per-core usage is capped at 100, so twelve busy cores and twelve
//! busy cores with forty-one more threads queued behind them read the
//! same. #865's incident reached **load average 53 on 12 cores** and no
//! rule in this module consulted it -- the aggregate rule saw 50%
//! machine-wide and stayed under its own 60% bar. Load average is the one
//! signal that says how many things are waiting for a core that does not
//! exist, and it was already being sampled and stored the whole time.
//!
//! Tier 3 ships because it is satisfiable against the schema that
//! already exists: `cpu_percent` is stored per sample
//! (`store::schema`), and `store::health::history` is already
//! gap-preserving and downsampled, so the rule is arithmetic over a
//! series the charts already draw. It is also the rule that matters
//! most for the incident that prompted the issue -- ten processes at
//! 93% each looked unremarkable in a list sorted by CPU, which is
//! exactly why they hid for two days. Ten processes at 93% is a
//! different condition from one at 93%, and **only an aggregate rule
//! sees it**.
//!
//! Tiers 1 and 2 are logged and notify for nothing, deliberately.
//! Their thresholds -- 30 minutes, 2 hours -- were reasoned from one
//! machine's workload, not measured against a baseline. The issue asks
//! for a week of would-be firings before any of them interrupts
//! anyone, and that log is the only thing that turns those numbers
//! from plausible into justified. [`Shadow`] is that log and nothing
//! more: it has no [`Alert`] variant, so there is no path from it to a
//! notification even by accident.
//!
//! Deferred with the notifications, and deliberately NOT built here:
//! per-PID duration tracking as persisted storage keyed on
//! `(pid, start_time)`, the maintained full-path daemon allowlist,
//! sibling-cluster detection, and the idle-work/IO check. #791 keeps
//! them; this release adds no migration.
//!
//! # Everything that decides anything is pure
//!
//! [`evaluate`] and [`shadow`] take data and return verdicts. Neither
//! touches a clock, a database, the process table or a notification
//! API -- the same rule `alerts::evaluate` follows, and for the same
//! reason: the cases this module exists for ("a two-day-old condition
//! must not read as fifteen minutes of burn") are testable as
//! arithmetic instead of by arranging hardware to misbehave for a
//! quarter of an hour.
//!
//! # Gap discipline is the whole of the duration rule
//!
//! The sampler runs at 60s cadence and ONLY while the app is open
//! (`lib.rs`), so a fifteen-minute rule is about fifteen samples and
//! cannot span an app restart or a closed lid. Without gap discipline
//! the first sample after a two-day absence sits next to the last one
//! before it, and a condition nobody watched would be "detected" as
//! fifteen minutes of sustained burn the moment the app reopens --
//! reporting a window the app was not running for.
//!
//! So every duration here is measured over CONSECUTIVE samples no more
//! than [`alerts::GAP_MS`] apart, and a gap ends the run rather than
//! being skipped over. That is the same constant, and the same
//! refusal, as the battery rate rules; `a_gap_is_not_sustained_burn`
//! below is the mutation test for it.
//!
//! # Why Tier 3 needs "no single process explains it"
//!
//! Total CPU at 60% is completely ordinary during a build. What is not
//! ordinary is 60% that no single process accounts for: that is either
//! a fan-out nobody asked for (a spawn loop, a stuck parallel job) or
//! a pile of orphans each individually too small to notice. The
//! largest single share is therefore part of the rule, not decoration
//! -- see [`Aggregate::largest_share`] and
//! [`SINGLE_PROCESS_EXPLAINS`].
//!
//! That share is a LIVE reading and the history is not: per-process
//! data lives in `health::footprint`, is sampled on demand and is
//! never persisted. So the duration half of the rule comes from the
//! stored series and the "no single process" half from the current
//! process table, which is sound because the question the second half
//! answers is about now: if one process explains the load at this
//! instant, the machine is busy rather than broken, and the whole
//! point of the rule is that nothing in the list looks wrong.

use super::alerts::GAP_MS;
use super::Sample;

/// Machine-wide CPU, in percent, that the aggregate rule cares about.
///
/// #791's figure. Low on purpose: the rule's discriminating power is
/// not the level but the combination of a long duration with no single
/// process to blame. A higher bar would have missed the incident --
/// ten `yes` processes on a many-core machine never pushed the
/// machine-wide figure near 100%, which is a second reason they hid.
pub const AGGREGATE_PERCENT: f64 = 60.0;

/// How long the aggregate load must hold, in minutes.
///
/// Fifteen is #791's figure, and at the sampler's 60s cadence it is
/// about fifteen samples -- long enough to outlast a test run or a
/// dependency install, short enough that a genuine runaway is named
/// within the quarter hour rather than in two days.
///
/// It is also deliberately the SHORTEST duration in this module, which
/// is only defensible because the "no single process" clause carries
/// the discrimination. A fifteen-minute rule on level alone would fire
/// on every build on the machine the issue came from.
pub const AGGREGATE_MINUTES: f64 = 15.0;

/// The share of machine CPU above which one process is the explanation.
///
/// Expressed as a fraction of the machine-wide figure rather than an
/// absolute percentage, because that is the actual question: "is this
/// one process, or is it a crowd". Half is the boundary that makes the
/// sentence true -- above it, one process is the majority of the load
/// and the CPU page's sorted list already names it, so an aggregate
/// alert would be a second notification about something the user can
/// already see.
///
/// `Process::cpu_percent` is a percentage of ONE core and the
/// machine-wide figure is a percentage of ALL of them, so the share is
/// computed after normalising by core count -- see [`largest_share`].
///
/// [`largest_share`]: Aggregate::largest_share
pub const SINGLE_PROCESS_EXPLAINS: f64 = 0.5;

/// The "worth a look" floor, as a percentage of one core (#865).
///
/// Half a core. Far below tier 1's 80 and tier 2's 90, deliberately: the
/// gap those two left is exactly the band the user reported as worth
/// investigating -- "even a process holding 50% cpu for longer than 5
/// minutes would be a concern to investigate."
///
/// A process at 50% satisfied NOTHING before this tier. Tier 1 needed 80
/// and PPID 1; tier 2 needed 90. Both floors sit above the level that
/// actually warrants a human glance.
pub const WATCH_PERCENT: f64 = 50.0;

/// How long [`WATCH_PERCENT`] must hold before it is worth surfacing.
///
/// Five minutes, and the duration is the whole discriminator here --
/// which is the opposite weighting from tiers 1 and 2, where the level
/// does the work.
///
/// The reasoning, in the user's own framing: "60% for 2-3 minutes would
/// be a compiler, but longer might be something suspicious". A build, a
/// test run, an install: all of them are hot and SHORT. Nothing
/// legitimate that a person is waiting on holds half a core for five
/// minutes without them knowing why. So a short burst at any level is
/// silence, and a moderate level that will not stop is the signal.
pub const WATCH_MINUTES: f64 = 5.0;

/// Where "worth a look" becomes "this has been going a long time".
///
/// Thirty minutes. Not a different rule -- the same condition, reported
/// more firmly, because the longer a moderate burn persists the less
/// likely any build explains it. Kept well under tier 2's two hours: by
/// the time something has held half a core for half an hour, the user
/// should already have been told rather than still waiting for a higher
/// bar.
pub const WATCH_LONG_MINUTES: f64 = 30.0;

/// The nice value above which a burn is more suspicious, not less.
///
/// Zero, so any positive nice qualifies. See
/// [`ProcessObservation::nice`] on why: nothing a user is waiting on
/// runs niced, and nicing is what hid #865's twelve spinners under the
/// aggregate rule's level gate.
///
/// Used to RAISE the report, never to suppress one. A nice of `None`
/// (Windows, or a process that exited mid-walk) therefore changes
/// nothing rather than defaulting either way.
pub const SUSPICIOUS_NICE: i32 = 0;

/// Runnable threads per core above which the machine is OVERSUBSCRIBED
/// (#872).
///
/// Load average normalised by core count: 1.0 is "exactly as many
/// things want a core as there are cores". This is 1.5, so a 12-core
/// machine qualifies at a load of 18.
///
/// # Why load average at all, when `cpu_percent` exists
///
/// Because a mean of per-core usage SATURATES. `collect.rs` computes
/// `cpu_percent` as the mean of `cpu_per_core`, and every core is capped
/// at 100 -- so a machine with twelve busy cores and a machine with
/// twelve busy cores plus forty-one more threads queued behind them both
/// read 100%. #865's incident reached load average **53 on 12 cores**
/// and no rule in this module consulted it; the aggregate rule saw 50%
/// machine-wide and stayed under its own 60% bar.
///
/// Load average is the only signal here that says how many things are
/// waiting for a core that does not exist. It is already sampled
/// (`collect.rs`) and already stored (`store::schema`'s `load_1`,
/// `load_5`, `load_15`), so this is arithmetic over a series the charts
/// already draw -- the same claim [`AGGREGATE_PERCENT`] makes.
///
/// # Why 1.5 rather than 1.0, and why it is measured rather than chosen
///
/// 1.0 is the textbook figure and it is wrong for this rule: it fires on
/// builds. Measured on a 12-core machine (the incident's own core count)
/// during `cargo build -j12` of this repo, 2026-09-12, sampling
/// `vm.loadavg` every 5-6 seconds across a 68-second build:
///
/// ```text
///            peak    / 12 cores
///   1-min    51.78     4.31x
///   5-min    16.86     1.40x
///   15-min    8.06     0.67x
/// ```
///
/// The 5-minute figure crossed 1.0x and STAYED there for about three and
/// a half minutes. So a rule at 1.0 on the five-minute average would
/// have fired on an ordinary build of this very repository -- which is
/// the #853 outcome (~40 false positives, guard disabled) arrived at
/// from a different direction.
///
/// 1.5x clears that build's 15-minute peak by better than a factor of
/// two and its 5-minute peak with room to spare, while the incident --
/// 53/12 = 4.4x -- clears 1.5x by nearly three times. The gap between
/// "the worst ordinary thing measured" and "the incident" is wide, and
/// 1.5 sits in it rather than at either edge.
pub const OVERSUBSCRIBED_RATIO: f64 = 1.5;

/// Which of the three load averages this rule reads.
///
/// Index 2, the FIFTEEN-minute average. `Sample::load` is
/// `[one, five, fifteen]`, per `collect.rs`.
///
/// This is the discriminating choice in the whole rule, and it is the
/// one the measurement above settled. During the `-j12` build the
/// one-minute average hit 4.31x core count -- a rule reading index 0
/// would fire on every link step on the machine -- and the five-minute
/// average held above 1.0x for three and a half minutes. The
/// fifteen-minute average peaked at 0.67x and never reached core count
/// at all.
///
/// A fifteen-minute average is itself a duration requirement, built into
/// the kernel's own arithmetic: a sixty-second spike enters it damped by
/// roughly the ratio of the spike to the window. That is why this rule
/// can afford a relatively low ratio where a one-minute reading could
/// not.
///
/// It is NOT a substitute for [`OVERSUBSCRIBED_MINUTES`]: the kernel's
/// window tells us the load was high for a while, and the sample run
/// tells us that the CONDITION is still true now and was true when last
/// looked at. A single high fifteen-minute reading can be the tail of
/// something that has already stopped.
///
/// Out of range is a COMPILE error rather than a runtime panic --
/// `Sample::load` is a `[f64; 3]` and this is a const index, so `rustc`
/// refuses a `3` with "this operation will panic at runtime". Verified by
/// setting it to 3 and watching the build fail, rather than assumed.
pub const OVERSUBSCRIBED_INDEX: usize = 2;

/// How long [`OVERSUBSCRIBED_RATIO`] must hold, in minutes.
///
/// Ten. Shorter than the eight and a half hours the incident ran and
/// longer than anything measured above: the build's fifteen-minute
/// average never crossed the ratio at all, so this duration is not what
/// excludes it -- it is the second line of defence, for the machine
/// whose builds are larger than this repo's.
///
/// Deliberately NOT five, the figure [`WATCH_MINUTES`] uses. That rule
/// reads a per-process instantaneous CPU figure, where five minutes of
/// samples is five minutes of evidence. This one reads a fifteen-minute
/// kernel average, so two samples ten minutes apart already describe
/// roughly twenty-five minutes of machine history.
///
/// # Why not longer, given the incident ran for 8.5 hours
///
/// Because `store::health::history` DOWNSAMPLES. It returns at most
/// `MAX_POINTS` (120) rows across `RETENTION_HOURS` (24), so on a
/// machine that has been open all day consecutive samples are twelve
/// minutes apart, not sixty seconds. A threshold of thirty minutes would
/// need three such samples and a threshold of ten needs two -- and two
/// is the minimum [`sustained_above_load`] can measure any duration from
/// at all. Pushing this higher buys nothing against the incident (which
/// would satisfy any figure up to eight hours) and costs detection on a
/// freshly opened app, where the series is short.
pub const OVERSUBSCRIBED_MINUTES: f64 = 10.0;

/// Tier 1's CPU floor, as a percentage of one core.
pub const TIER1_PERCENT: f64 = 80.0;

/// Tier 1's duration, in minutes. Thirty rather than five, because
/// orphanhood is already doing the discriminating work.
pub const TIER1_MINUTES: f64 = 30.0;

/// Tier 2's CPU floor, as a percentage of one core.
pub const TIER2_PERCENT: f64 = 90.0;

/// Tier 2's duration, in minutes: two hours, past almost any
/// legitimate build on the reporting machine.
pub const TIER2_MINUTES: f64 = 120.0;

/// Something about the machine's CPU worth interrupting someone for.
///
/// One variant, because one tier ships. A second variant would be the
/// change that takes a shadow-logged tier live, and it should be made
/// deliberately and with the distribution in hand -- see the module
/// docs.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// Machine-wide CPU has been high for a long time and no single
    /// process accounts for it.
    ///
    /// The one the incident in #791 needed. The numbers are the wording
    /// and not the identity; see [`Alert::key`].
    DiffuseCpu {
        /// The mean machine-wide CPU across the sustained run, 0-100.
        percent: f64,
        /// How long it has held, in minutes, measured over ungapped
        /// samples only.
        minutes: f64,
        /// How many processes were running when the share was read.
        /// Part of the body: "nothing in particular, out of 1436" is
        /// the sentence that makes the alert actionable.
        process_count: usize,
    },
}

impl Alert {
    /// The identity the caller remembers, so a standing condition is
    /// announced ONCE rather than every sixty seconds.
    ///
    /// Deliberately excludes every number, exactly as
    /// `alerts::Alert::key` does: keying on the percentage would make
    /// 61% and 62% different alerts, and a machine grinding at a
    /// wandering 60-something would notify on every tick of the way.
    /// The condition is the identity; the numbers are only the wording.
    pub fn key(&self) -> &'static str {
        match self {
            Alert::DiffuseCpu { .. } => "diffuse_cpu",
        }
    }

    /// The notification title.
    ///
    /// Names the SHAPE of the fault rather than the level, because the
    /// level is not the news -- "CPU at 62%" is a thing a user can see
    /// and would not thank anyone for a notification about.
    pub fn title(&self) -> String {
        match self {
            Alert::DiffuseCpu { .. } => "CPU is busy with nothing in particular".to_string(),
        }
    }

    /// The notification body: what was measured, and why it is being
    /// said.
    ///
    /// Says the duration and says that no one process explains it,
    /// because that second clause is the entire reason the alert is
    /// worth reading. Without it the sentence is "your machine is
    /// busy", which the user knows.
    pub fn body(&self) -> String {
        match self {
            Alert::DiffuseCpu {
                percent,
                minutes,
                process_count,
            } => format!(
                "About {percent:.0}% of the CPU has been in use for {minutes:.0} minutes, and no single process among the {process_count} running accounts for it. That pattern is usually several runaway or orphaned processes rather than one busy program."
            ),
        }
    }
}

/// What a deferred tier WOULD have fired, had it been live.
///
/// Not an [`Alert`] and deliberately not convertible into one: the
/// point of shadow mode is that these produce log lines and nothing
/// else, and a `From` impl would be the one line that quietly turned a
/// week of measurement into a week of notifications.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    /// `"tier1"` or `"tier2"`, the tier that would have fired.
    pub tier: &'static str,
    /// Whether it would have been an alert (tier 1) or a warn (tier 2).
    /// Recorded so the eventual tuning can tell the two distributions
    /// apart without re-deriving them from the thresholds.
    pub severity: &'static str,
    /// The process name, as the OS reported it.
    ///
    /// A name, never a path and never a command line. The privacy rule
    /// (CONTRIBUTING, `check-privacy.sh`) is about owners and tokens,
    /// but a full `exe()` path on a developer machine carries directory
    /// names that are nobody's business in a log file -- and the name
    /// is what the eventual allowlist will be tuned against anyway.
    pub name: String,
    /// CPU as a percentage of ONE core, so a multi-core process
    /// legitimately reads above 100.
    pub cpu_percent: f64,
    /// How long the condition has held, in minutes, over ungapped
    /// observations only.
    pub minutes: f64,
    /// Whether the process's parent is PID 1.
    ///
    /// Part of tier 1's rule and recorded for tier 2 as well, because
    /// the distribution of "parented but runaway" is exactly what
    /// decides whether tier 2 is worth having.
    pub orphaned: bool,
    /// Whether the name matched [`SEED_DAEMONS`].
    pub allowlisted: bool,
}

impl Shadow {
    /// The log line. Deliberately one line per would-be firing, with
    /// every field the later tuning needs, so a week of logs greps into
    /// a distribution without anyone having to reconstruct what the
    /// thresholds were at the time.
    pub fn line(&self) -> String {
        format!(
            "runaway shadow: {} ({}) would have fired for {} at {:.0}% of one core for {:.0} min (orphaned={}, allowlisted={})",
            self.tier, self.severity, self.name, self.cpu_percent, self.minutes, self.orphaned, self.allowlisted
        )
    }
}

/// The scheduling nice value of a process, or `None`.
///
/// `sysinfo` 0.39 does not expose this -- there is no `nice` or
/// `priority` accessor anywhere in the crate -- so it is read through
/// `getpriority(2)`, which is POSIX. `libc` was already in `Cargo.lock`
/// transitively, so this adds a direct edge rather than a new crate.
///
/// # The errno dance is not optional
///
/// `getpriority` returns `-1` on failure AND `-1` is a legal nice value
/// (a high-priority process). The only way to tell them apart is to
/// clear `errno` first and inspect it after, which is what this does.
/// A naive `if v == -1 { None }` would report every high-priority
/// process as unreadable -- quietly, and in the direction that loses
/// exactly the processes most able to starve the machine.
///
/// Verified against `ps -o nice` on real niced processes before being
/// built on: nice 10 and 17 read back as 10 and 17, a nonexistent pid
/// reads back `None`.
#[cfg(unix)]
pub fn nice_of(pid: u32) -> Option<i32> {
    // SAFETY: `getpriority` takes two integers and touches no memory we
    // own. `errno` is thread-local, so clearing it cannot race another
    // thread's reading of it.
    unsafe {
        *errno_location() = 0;
        // `PRIO_PROCESS` is `c_int` on Apple and `__priority_which_t` on
        // Linux, so the cast is inferred rather than named -- writing
        // either concrete type breaks the other platform.
        let v = libc::getpriority(libc::PRIO_PROCESS as _, pid as libc::id_t);
        if v == -1 && *errno_location() != 0 {
            None
        } else {
            Some(v)
        }
    }
}

/// `errno`'s address, which libc spells differently per platform.
#[cfg(unix)]
unsafe fn errno_location() -> *mut i32 {
    #[cfg(target_vendor = "apple")]
    {
        libc::__error()
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        libc::__errno_location()
    }
}

/// Windows has no nice, so there is nothing to read.
///
/// `None` rather than `Some(0)`: see [`ProcessObservation::nice`] on why
/// a false "normal priority" is worse than an admitted unknown.
#[cfg(not(unix))]
pub fn nice_of(_pid: u32) -> Option<i32> {
    None
}

/// The seed daemon names for the shadow log's tier-1 clause.
///
/// **Not the allowlist #791 asks for, and must not be mistaken for
/// it.** The issue is explicit that the real list matches on FULL PATH,
/// because `node` is far too generic to allowlist by name -- a bare
/// name match here would exonerate any runaway that happened to be
/// called `node`, which is most of them on a JavaScript machine.
///
/// It is a bare-name match anyway, for one release, because this is the
/// SHADOW log: a name that is wrongly exonerated is a log line that
/// does not appear, and the only cost is a slightly thinner
/// distribution to tune against. Getting it wrong in a live rule would
/// be a missed alert. The maintained full-path list is deferred with
/// the tiers that depend on it, and [`Shadow::allowlisted`] records the
/// verdict per firing rather than filtering it out, so a week of logs
/// still shows what the allowlist cost.
///
/// # Why the allowlist is load-bearing rather than a nicety
///
/// `footprint.rs` already documents it: on macOS a great many
/// LEGITIMATE user processes reparent to `launchd` (PID 1) when their
/// spawner exits -- a dev server started from a terminal that was then
/// closed is an orphan by this test and entirely healthy. So PPID 1 is
/// much weaker evidence on macOS than on Linux, where an orphan is
/// genuinely unusual. On the platform this app primarily ships to,
/// tier 1 is "high CPU for half an hour AND not on a list", and the
/// list is carrying most of the weight. That is the main thing the
/// shadow week is meant to measure.
pub const SEED_DAEMONS: &[&str] = &[
    "ollama",
    "terraform",
    "docker",
    "com.docker.backend",
    "claude",
    "mds",
    "mds_stores",
    "mdworker_shared",
    "backupd",
    "kernel_task",
    "launchd",
    "WindowServer",
    "photoanalysisd",
    "Spotlight",
];

/// One process as the rules read it.
///
/// A local type rather than `footprint::Process`, for two reasons that
/// both matter. First, the rules need `parent` and `start_time`, which
/// that type does not carry and should not grow for a shadow log.
/// Second, `footprint::Footprint` is being reshaped by #795/#796 and a
/// dependency here would couple a health rule to that churn; this
/// struct is built straight from `sysinfo` by [`Watcher`] instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessObservation {
    pub pid: u32,
    /// Seconds since the epoch, as the platform reports it.
    ///
    /// Paired with `pid` as the process identity. PIDs are RECYCLED, so
    /// a pid alone would let a fresh process inherit the elapsed burn
    /// of a dead one that happened to have the same number -- which is
    /// the failure mode that makes a long-duration rule fire on
    /// something three seconds old.
    pub start_time: u64,
    pub name: String,
    /// CPU as a percentage of ONE core, like `footprint::Process`.
    pub cpu_percent: f64,
    /// The parent PID, or `None` where the platform will not say.
    ///
    /// `None` is NOT treated as PID 1: "we could not read the parent"
    /// and "this process has no living parent" are opposite answers,
    /// and conflating them would make every unreadable process an
    /// orphan. Absent is not zero, the same rule as the rest of
    /// `health`.
    pub parent: Option<u32>,
    /// The scheduling nice value, or `None` where it cannot be read.
    ///
    /// `None` on Windows, which has no nice, and on a Unix process that
    /// vanished between the table walk and the read. Not defaulted to 0:
    /// zero is the common REAL value, so defaulting would assert
    /// "normal priority" about a process nothing could measure -- and
    /// this field's whole purpose is to raise suspicion, which a false
    /// zero would silently lower.
    ///
    /// # Why a niced process is MORE suspicious, not less
    ///
    /// Nothing a user is waiting on runs niced. A build, a test run, a
    /// language server: all of them compete at normal priority because
    /// someone wants the answer. Nice is what a background job is given
    /// -- or what an abandoned one was given and then forgotten.
    ///
    /// It is also how #865's incident hid. Twelve orphaned busy-loops
    /// were niced to 5, so the scheduler throttled each to ~50% of a
    /// core instead of 100%, and twelve of those on twelve cores
    /// averaged 50% machine-wide -- under `AGGREGATE_PERCENT`'s 60. The
    /// processes were saturating every core; the nicing is the only
    /// reason the aggregate rule did not see it.
    pub nice: Option<i32>,
}

impl ProcessObservation {
    /// Whether this process's parent is PID 1.
    ///
    /// See [`SEED_DAEMONS`] on why this is much weaker evidence on
    /// macOS than the phrase "orphaned" suggests.
    pub fn orphaned(&self) -> bool {
        self.parent == Some(1)
    }

    /// Whether the name is one of [`SEED_DAEMONS`].
    pub fn allowlisted(&self) -> bool {
        let name = self.name.to_ascii_lowercase();
        SEED_DAEMONS
            .iter()
            .any(|d| name == d.to_ascii_lowercase() || name.starts_with(&d.to_ascii_lowercase()))
    }
}

/// The live half of the aggregate rule: what the process table says
/// right now.
///
/// Separate from the stored series because the two halves come from
/// different places and refresh on different schedules -- see the
/// module docs on why that is sound rather than a compromise.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    /// The largest single process's CPU, as a percentage of one core.
    pub top_cpu_percent: f64,
    /// How many logical cores the machine has, so the figure above can
    /// be compared with a machine-wide percentage.
    ///
    /// Never zero: [`Aggregate::largest_share`] treats a zero as
    /// "unknown" and returns `None`, because dividing by it would
    /// produce an infinity that clears every threshold at once.
    pub cores: usize,
    /// How many processes were running. Carried into the alert body.
    pub process_count: usize,
}

impl Aggregate {
    /// The biggest process's share of the WHOLE machine, 0.0-1.0, or
    /// `None` when the core count is unknown.
    ///
    /// `top_cpu_percent` is per-core and a machine-wide percentage is
    /// per-machine, so 93% of one core on a ten-core machine is 9.3% of
    /// the machine. Comparing the two numbers directly -- which is the
    /// obvious mistake, and the one that would have declared a single
    /// `yes` process "the explanation" for the whole incident -- is
    /// what this division exists to prevent.
    pub fn largest_share(&self) -> Option<f64> {
        if self.cores == 0 {
            return None;
        }
        Some(self.top_cpu_percent / (self.cores as f64 * 100.0))
    }

    /// Whether one process accounts for the machine's load.
    ///
    /// `None` (unknown core count) is NOT "no single process explains
    /// it": an unknown cannot be evidence for the clause the alert
    /// depends on, so it reads as "cannot say", and [`evaluate`]
    /// stays silent. Silence on missing data is the same choice
    /// `alerts::evaluate` makes for a missing battery reading.
    pub fn one_process_explains_it(&self) -> Option<bool> {
        self.largest_share()
            .map(|share| share >= SINGLE_PROCESS_EXPLAINS)
    }
}

/// A run of consecutive, ungapped samples at the end of the series
/// whose `cpu_percent` is all at or above `floor`.
///
/// Returns `(minutes, mean_percent)`, or `None` when the run does not
/// reach `floor` at the newest sample, when a reading is missing, or
/// when there is not enough measured time.
///
/// Walks BACKWARDS from the newest sample and stops at the first thing
/// that is not comparable -- a gap wider than [`GAP_MS`], a missing
/// `cpu_percent`, an out-of-order timestamp, or a sample below the
/// floor. Stopping rather than skipping is the point: a run of high
/// samples from before a six-hour hole says nothing about the present,
/// and stitching the two sides together is arithmetic over a period
/// nobody measured.
///
/// `cpu_percent` is `Option` on [`Sample`] and a `None` breaks the run
/// for the same reason a missing battery reading breaks the battery
/// trend: "not measured" is an unknown, not a low reading, and treating
/// it as either would be a claim about a minute nobody sampled.
fn sustained_above(samples: &[Sample], floor: f64) -> Option<(f64, f64)> {
    let mut total_minutes = 0.0;
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut newer: Option<(i64, f64)> = None;

    for s in samples.iter().rev() {
        let Some(cpu) = s.cpu_percent else { break };
        if cpu < floor {
            break;
        }
        let Ok(at) = chrono::DateTime::parse_from_rfc3339(&s.sampled_at) else {
            break;
        };
        let ms = at.timestamp_millis();
        if let Some((newer_ms, _)) = newer {
            let spacing = newer_ms - ms;
            // Non-positive spacing means the rows are out of order or
            // share an instant; a wider one than GAP_MS means the two
            // samples are not comparable at all. Either way the run
            // ends HERE, with the samples already counted kept.
            if spacing <= 0 || spacing > GAP_MS {
                break;
            }
            total_minutes += spacing as f64 / 60_000.0;
        }
        sum += cpu;
        count += 1;
        newer = Some((ms, cpu));
    }

    // One sample is a reading, not a duration: it spans no measured
    // time at all, so there is nothing to compare against a
    // minutes-long threshold.
    if count < 2 {
        return None;
    }
    Some((total_minutes, sum / count as f64))
}

/// Everything the aggregate rule currently finds true of this series.
///
/// `samples` is oldest-first, as `store::health::history` returns it.
/// `live` is the current process table's answer, or `None` when it
/// could not be read.
///
/// Returns what IS true, not what is NEW: deduplication is
/// `alerts::Fired`'s job, and keeping the two apart is what lets this
/// function stay a pure statement about the data with the "have we said
/// this already" state in exactly one place.
pub fn evaluate(samples: &[Sample], live: Option<&Aggregate>) -> Vec<Alert> {
    let mut out = Vec::new();

    let Some((minutes, percent)) = sustained_above(samples, AGGREGATE_PERCENT) else {
        return out;
    };
    if minutes < AGGREGATE_MINUTES {
        return out;
    }

    // No live reading is silence, not a firing. The "no single process
    // explains it" clause is the whole content of this alert -- without
    // it the sentence is "your machine is busy", which the user can see
    // -- so an unreadable process table means the alert cannot be
    // justified rather than that it can be assumed.
    let Some(live) = live else { return out };
    match live.one_process_explains_it() {
        // One process is the majority of the load: the machine is busy,
        // and the CPU page's sorted list already names the culprit. An
        // alert here would be a notification about something visible.
        Some(true) => return out,
        // Unknown core count: cannot say, so say nothing.
        None => return out,
        Some(false) => {}
    }

    out.push(Alert::DiffuseCpu {
        percent,
        minutes,
        process_count: live.process_count,
    });
    out
}

/// What tiers 1 and 2 WOULD have fired, given one pass of observations
/// and how long each has been above its floor.
///
/// `durations` maps `(pid, start_time)` to minutes observed above the
/// tier floor, which is what [`Watcher`] accumulates. Keyed on the pair
/// rather than the pid alone because PIDs are recycled -- see
/// [`ProcessObservation::start_time`].
///
/// Pure, like [`evaluate`], and for the same reason: "thirty minutes of
/// burn" is testable as arithmetic over a map rather than by keeping a
/// process pinned for half an hour.
///
/// Tier 1 is checked before tier 2 and a process that matches both is
/// reported ONCE, as tier 1. Two log lines about one process would
/// double-count it in the distribution the shadow week exists to
/// produce, which is the one thing this log must get right.
/// A process worth a human glance, for the page rather than a
/// notification (#865).
///
/// # Why this is not an `Alert`
///
/// `Alert` is what INTERRUPTS someone, and
/// `nothing_converts_a_shadow_into_an_alert` asserts that exactly one
/// variant ships -- a second would mean a deferred tier had started
/// notifying, which is a decision to make deliberately rather than as a
/// side effect. This type is the other thing the user asked for: "should
/// at least have an indicator in the UI to indicate that a user might
/// want to investigate."
///
/// So a `Notice` reaches `health_alerts`, which #870 made a page can
/// read, and never reaches `notify_runaway`. Seen when looked at, not
/// pushed.
///
/// # Why this is an enum (#872)
///
/// It shipped in #865 as a struct describing one process. #872 adds a
/// condition of the MACHINE rather than of a process -- sustained
/// oversubscription, read from load average -- and the two share a
/// destination and nothing else: the page, `health_alerts`, and the rule
/// that neither ever becomes an [`Alert`].
///
/// Flattening both into one struct would mean a `name` and a
/// `cpu_percent` on a row that is about no process in particular, which
/// is the shape that invites a zero to be read as a measurement. Two
/// unrelated types would mean [`Watched`] holding two lists and
/// `health_alerts` concatenating them, for a consumer that only ever
/// calls [`key`], [`title`] and [`body`]. So: one enum, two variants,
/// three methods.
///
/// [`key`]: Notice::key
/// [`title`]: Notice::title
/// [`body`]: Notice::body
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// One process worth a human glance (#865).
    Process {
        name: String,
        /// The process id, for a notice about exactly ONE process (#943).
        ///
        /// `Some` when `count == 1`; `None` the moment two are collapsed.
        /// The notice's own last sentence is "Worth a look", and the thing
        /// a reader looks WITH is a pid to type into `ps` -- which this
        /// module had in hand at the observation and dropped when building
        /// the notice, leaving the sentence with nothing behind it.
        ///
        /// # Why `None` on a collapsed notice
        ///
        /// A "3 node processes have been busy" row is about three pids and
        /// no single one of them. Naming one would name a process the row
        /// is not about -- exactly the mistake the grouped process table
        /// avoids by suppressing its PID column under grouping, and the
        /// same reasoning [`Notice::name`] gives for returning `None` on
        /// the machine-wide variant rather than inventing "system".
        ///
        /// # Why it is not in `key`
        ///
        /// [`key`] must stay keyed on the CONDITION: a process wandering
        /// between 51% and 58% is one row, and #908's collapse-by-name
        /// depends on two `zsh` notices sharing a key. A pid is stable for
        /// the life of a process but not across a restart, so keying on it
        /// would turn a standing condition into a new row every time the
        /// thing restarted. It goes in the payload.
        ///
        /// [`key`]: Notice::key
        pid: Option<u32>,
        /// How many processes of this name met the rule (#908).
        ///
        /// Collapsed into ONE notice rather than emitted per process,
        /// because `key` is derived from the name: two `zsh` processes
        /// produced two notices with an IDENTICAL key, which rendered as
        /// two indistinguishable blocks and read as one condition repeated.
        /// A user reported exactly that as a suspected staleness bug.
        ///
        /// It also broke the contract `key` is for -- a caller
        /// deduplicating a standing condition saw two rows it could not
        /// tell apart, and a React list keyed on it had duplicate keys.
        count: usize,
        /// CPU as a percentage of ONE core, so a multi-core process reads
        /// above 100 legitimately. The WORST of the collapsed set, not a
        /// mean: the question is whether anything is pegging a core.
        cpu_percent: f64,
        minutes: f64,
        /// Raises the wording, never gates the notice. See
        /// [`ProcessObservation::nice`].
        niced: bool,
        orphaned: bool,
        /// Past [`WATCH_LONG_MINUTES`]: the same condition, said more
        /// firmly, because duration is what separates a build from an
        /// abandoned loop.
        long: bool,
    },
    /// More runnable threads than the machine has cores, held (#872).
    ///
    /// The condition #865's incident was screaming and nothing read:
    /// load average 53 on 12 cores, for eight and a half hours. See
    /// [`oversubscribed`] and [`OVERSUBSCRIBED_RATIO`].
    Oversubscribed {
        /// The mean load average across the run -- the FIFTEEN-minute
        /// figure, per [`OVERSUBSCRIBED_INDEX`]. Absolute, as the kernel
        /// reports it, because that is the number the System Health page
        /// shows under "Load (15m)" and the user should be able to match
        /// the two.
        load: f64,
        /// The core count it was normalised by. Never 0 -- a 0 makes
        /// [`oversubscribed`] return `None` rather than divide.
        cores: usize,
        /// `load / cores`: runnable threads per core, so 1.0 is exactly
        /// saturated and 4.4 is the incident.
        ratio: f64,
        /// How long it has held, in minutes, over ungapped samples only.
        minutes: f64,
    },
}

impl Notice {
    /// One stable key per condition.
    ///
    /// Keyed on the process NAME, or on nothing at all for the machine-wide
    /// variant, and never on the figures -- the same rule `Alert::key`
    /// states: a process wandering between 51% and 58%, or a load wandering
    /// between 19 and 21, must stay ONE row rather than becoming a new one
    /// every poll.
    pub fn key(&self) -> String {
        match self {
            Notice::Process { name, .. } => format!("cpu_watch:{name}"),
            // No figures and no name: there is one machine, so there is
            // one row, however the load moves.
            Notice::Oversubscribed { .. } => "cpu_oversubscribed".to_string(),
        }
    }

    /// The worst CPU of the processes this row stands for, as a percentage
    /// of one core. `0.0` for the machine-wide variant, which has no single
    /// process to report.
    pub fn cpu_percent(&self) -> f64 {
        match self {
            Notice::Process { cpu_percent, .. } => *cpu_percent,
            Notice::Oversubscribed { .. } => 0.0,
        }
    }

    /// How many processes this row stands for. 1 for the machine-wide
    /// variant, which is not a process count at all.
    pub fn count(&self) -> usize {
        match self {
            Notice::Process { count, .. } => *count,
            Notice::Oversubscribed { .. } => 1,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Notice::Process { name, count, .. } if *count > 1 => {
                format!("{count} {name} processes have been busy for a while")
            }
            Notice::Process { name, .. } => format!("{name} has been busy for a while"),
            // Names the SHAPE, not the level, for the reason
            // `Alert::title` gives: "load average 53" is a number the
            // page already shows, and "more work queued than cores"
            // is the thing it means.
            Notice::Oversubscribed { .. } => {
                "More work is queued than this machine has cores".to_string()
            }
        }
    }

    pub fn body(&self) -> String {
        match self {
            Notice::Process {
                name,
                cpu_percent,
                minutes,
                niced,
                orphaned,
                ..
            } => {
                // The suspicious facts are stated, not scored. The user
                // decides; this sentence only gives them what a glance at
                // `ps` would have.
                let mut why = String::new();
                if *niced {
                    why.push_str(
                        " It is running at low priority, which usually means a background job --                  nothing you are waiting on runs niced.",
                    );
                }
                if *orphaned {
                    why.push_str(" Its parent has exited, so nothing is supervising it.");
                }
                format!(
                    "{name} has held about {cpu_percent:.0}% of one CPU core for {minutes:.0} minutes.{why} Worth a look if you              did not start something long-running."
                )
            }
            // Says the ratio in words rather than only the raw load,
            // because "53" means nothing without "on 12 cores" beside
            // it -- and the whole reason this rule exists is that a
            // saturating percentage cannot express the difference.
            Notice::Oversubscribed {
                load,
                cores,
                ratio,
                minutes,
            } => format!(
                "The load average has been about {load:.0} on {cores} cores for {minutes:.0} minutes -- roughly {ratio:.1} times as much work queued as there are cores to run it. A large build does this briefly; for this long it usually means something is not finishing."
            ),
        }
    }

    /// The process name, or `None` for a machine-wide notice.
    ///
    /// An accessor rather than a field because the machine-wide variant
    /// has no name, and inventing one ("system") would put a string that
    /// names no process where callers read process names.
    pub fn name(&self) -> Option<&str> {
        match self {
            Notice::Process { name, .. } => Some(name),
            Notice::Oversubscribed { .. } => None,
        }
    }

    /// The process id, or `None` where there is no single one (#943).
    ///
    /// Three ways to be `None` and all three are the same answer: the
    /// machine-wide variant is about no process, a collapsed notice is
    /// about several, and neither has a pid a reader could paste into `ps`
    /// and get the row back. An accessor like [`name`], for the same
    /// reason: inventing a number here would be worse than inventing
    /// "system" as a name, because a pid that names the wrong process
    /// looks exactly like one that names the right one.
    ///
    /// [`name`]: Notice::name
    pub fn pid(&self) -> Option<u32> {
        match self {
            Notice::Process { pid, .. } => *pid,
            Notice::Oversubscribed { .. } => None,
        }
    }

    /// How long the condition has held, in minutes. Both variants have
    /// one, and it is what [`watch`] sorts on.
    pub fn minutes(&self) -> f64 {
        match self {
            Notice::Process { minutes, .. } | Notice::Oversubscribed { minutes, .. } => *minutes,
        }
    }

    /// Whether the process is niced. `false` for a machine-wide notice,
    /// which is about no process and so has no priority.
    pub fn niced(&self) -> bool {
        matches!(self, Notice::Process { niced: true, .. })
    }

    /// Whether the process's parent has exited. `false` for a
    /// machine-wide notice, for the same reason as [`niced`].
    ///
    /// [`niced`]: Notice::niced
    pub fn orphaned(&self) -> bool {
        matches!(self, Notice::Process { orphaned: true, .. })
    }

    /// Whether the condition is past its "this has been going a long
    /// time" mark ([`WATCH_LONG_MINUTES`]).
    pub fn long(&self) -> bool {
        matches!(self, Notice::Process { long: true, .. })
    }
}

/// The most recent [`watch`] result, for a page to read.
///
/// # Why shared state rather than recomputed per call
///
/// A `Notice` is half level and half DURATION, and duration only exists
/// in the poll loop's long-lived [`Watcher`] -- it is accumulated across
/// 60-second passes. `health_alerts` builds a fresh `Table` per call, so
/// it has no history at all: every process would read 0.0 minutes and
/// nothing would ever qualify.
///
/// The poll loop already computes the durations once a minute. This
/// hands that result to whoever asks, so there is ONE accumulator rather
/// than a second one in the command that could disagree with it.
///
/// Empty before the first pass, which is honest: on a cold start nothing
/// has been observed long enough to have held anything for five minutes.
/// How stale an answer may be before it is withheld (#908).
///
/// The poll loop writes every 60 seconds, so three minutes is three missed
/// passes -- generous enough that a slow sampler or a machine that just woke
/// from sleep does not blank the panel, short enough that a dead loop stops
/// presenting its last answer as the present.
///
/// The direction matters. Withholding a stale answer makes the panel say
/// nothing, which is honest; serving one makes it assert a live condition
/// it cannot support, which is this codebase's characteristic defect
/// (#769, #841, #847). A user reported exactly this suspicion -- two
/// notices they believed were an hour stale -- and on that occasion the
/// processes were genuinely still running. The notices were right and there
/// was no way for them to tell.
pub const NOTICE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(180);

/// The most recent [`watch`] result and WHEN it was taken.
///
/// The timestamp is the whole point of the pairing. `set` replaces the list
/// each pass, so a process that has exited drops out correctly -- verified
/// -- but only while the loop runs. If the poll thread dies, the last value
/// would otherwise sit in this mutex forever and `health_alerts` would keep
/// serving it, rendered identically to a fresh answer.
///
/// `Instant` rather than a wall clock: this is an elapsed-time question, and
/// a clock that jumps (NTP correction, sleep/wake) must not make an answer
/// look fresh or ancient. The same reasoning `Watcher` gives for GAP_MS.
#[derive(Debug, Default)]
pub struct Watched(std::sync::Mutex<Option<(std::time::Instant, Vec<Notice>)>>);

impl Watched {
    pub fn set(&self, notices: Vec<Notice>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((std::time::Instant::now(), notices));
    }

    /// The notices, and how long ago they were taken.
    ///
    /// `None` before the first pass -- a cold start has observed nothing
    /// long enough to have held anything for five minutes -- and `None`
    /// again once the newest pass is older than [`NOTICE_MAX_AGE`], so a
    /// caller cannot accidentally present a dead loop's last answer.
    pub fn get(&self) -> Option<(std::time::Duration, Vec<Notice>)> {
        let held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (at, notices) = held.as_ref()?;
        let age = at.elapsed();
        if age > NOTICE_MAX_AGE {
            return None;
        }
        Some((age, notices.clone()))
    }
}

/// A notice body with the age of its evidence appended (#908).
///
/// Below a minute says "just now" rather than a number of seconds: the poll
/// cadence is 60s, so any smaller figure is noise about when the timer
/// happened to fire rather than information about the machine.
///
/// Separate from `Notice::body` so the notice stays a statement about a
/// process and the freshness stays a statement about the reading. They have
/// different lifetimes -- the body is the same on every pass, the age is not
/// -- and keying a notice on a body that changed every minute would make a
/// standing condition a new row each tick, which is the trap `Notice::key`
/// and `Alert::key` both document.
pub fn with_age(body: &str, age: std::time::Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{body} (as of just now)")
    } else {
        let mins = secs / 60;
        let plural = if mins == 1 { "" } else { "s" };
        format!("{body} (as of {mins} minute{plural} ago)")
    }
}

/// Processes worth surfacing on the page, cheapest clause first.
///
/// # Why the level floor is low and the duration carries the rule
///
/// This is the inverse weighting of tiers 1 and 2, on purpose. Those
/// need 80% and 90% of a core, which is above the band that actually
/// warrants a glance, and tier 1 additionally requires PPID 1 -- so a
/// parented process at half a core forever matched nothing at all.
///
/// Duration is the discriminator because level is not: a compiler, a
/// test run and an install are all hot and SHORT. Something holding half
/// a core past five minutes is either known to the user or worth their
/// attention, and the second case is the one nothing else in this module
/// catches.
///
/// Allowlisted daemons are excluded for the same reason tier 1 excludes
/// them, and the nice value only ever RAISES a notice that already
/// qualified -- a guard that cries wolf gets turned off, which is the
/// lesson this repo has already paid for once (#853's ~40 false
/// positives).
pub fn watch(
    observations: &[ProcessObservation],
    durations: &std::collections::HashMap<(u32, u64), f64>,
) -> Vec<Notice> {
    let mut out = Vec::new();
    for p in observations {
        // The cheap clauses first: a level test and a map lookup before
        // anything derived. `shadow` above orders itself the same way.
        if p.cpu_percent < WATCH_PERCENT {
            continue;
        }
        let minutes = durations
            .get(&(p.pid, p.start_time))
            .copied()
            .unwrap_or(0.0);
        if minutes < WATCH_MINUTES {
            continue;
        }
        if p.allowlisted() {
            continue;
        }
        out.push(Notice::Process {
            name: p.name.clone(),
            // The pid was already in hand two lines up, for the duration
            // lookup (#943). It used to stop there.
            pid: Some(p.pid),
            count: 1,
            cpu_percent: p.cpu_percent,
            minutes,
            niced: p.nice.is_some_and(|n| n > SUSPICIOUS_NICE),
            orphaned: p.orphaned(),
            long: minutes >= WATCH_LONG_MINUTES,
        });
    }

    // Collapse by name, because `key` is derived from the name (#908). Two
    // processes of one name produced two notices with the SAME key, which
    // is both a broken deduplication contract and two rows a reader cannot
    // tell apart.
    //
    // The collapsed row takes the WORST of each figure rather than a mean:
    // the question a reader has is "is something pegging a core, and for how
    // long", and an average of one runaway and one mild process answers
    // neither. `niced`/`orphaned`/`long` are OR-ed for the same reason --
    // they raise the wording, so any member raising it is enough.
    let mut collapsed: Vec<Notice> = Vec::new();
    for n in out {
        let Notice::Process {
            name,
            cpu_percent,
            minutes,
            niced,
            orphaned,
            long,
            ..
        } = &n
        else {
            collapsed.push(n);
            continue;
        };
        if let Some(Notice::Process {
            pid: existing_pid,
            count: c,
            cpu_percent: cp,
            minutes: m,
            niced: ni,
            orphaned: o,
            long: l,
            ..
        }) = collapsed
            .iter_mut()
            .find(|e| matches!(e, Notice::Process { name: existing, .. } if existing == name))
        {
            // The pid goes the moment there are two (#943). It is the ONE
            // field that is not a worst-of or an OR, because it is not a
            // figure about the group at all: "3 node processes" is about
            // three pids, and keeping the first one would offer a reader a
            // number that names one member of a row that is about all of
            // them. Cleared rather than kept-as-first for the same reason
            // the grouped process table suppresses its PID column.
            *existing_pid = None;
            *c += 1;
            *cp = cp.max(*cpu_percent);
            *m = m.max(*minutes);
            *ni |= *niced;
            *o |= *orphaned;
            *l |= *long;
            continue;
        }
        collapsed.push(n);
    }

    // Longest first: if the list is ever truncated for display, the one
    // that has been going longest is the one that survives.
    collapsed.sort_by(|a, b| b.minutes().total_cmp(&a.minutes()));
    collapsed
}

/// A run of consecutive, ungapped samples at the end of the series whose
/// normalised load average is all at or above `ratio` (#872).
///
/// Returns `(minutes, mean_ratio, mean_load)`, or `None` when the newest
/// sample is below the ratio, when a load reading is missing, when the
/// core count is unknown, or when there is not enough measured time.
///
/// The sibling of [`sustained_above`] and deliberately a separate
/// function rather than a generic over "some field of `Sample`". Three
/// things differ and all three are the substance of the rule: the field
/// is an `Option<[f64; 3]>` and not an `Option<f64>`, the comparison is
/// against a NORMALISED figure rather than the stored one, and the
/// normalisation can itself fail. A closure-taking generic would hide
/// exactly those three decisions behind a call site.
///
/// # `cores == 0` is silence, not a division
///
/// `Aggregate::cores` is 0 when `available_parallelism` failed, and this
/// follows [`Aggregate::largest_share`] exactly: return `None` rather
/// than divide. `load / 0` is `+inf` in IEEE 754, which would clear
/// every threshold this module has at once -- a single unreadable core
/// count would turn the rule into "always fire", on every machine it
/// could not measure. That is the characteristic failure this repo calls
/// absent-is-not-zero, in its most expensive form.
///
/// # A missing load reading breaks the run
///
/// `Sample::load` is `None` on Windows BY DESIGN -- `collect.rs` maps
/// Windows' three zeroes to absent, because Windows has no load average
/// and three zeroes are indistinguishable from a genuinely idle machine.
/// So a `None` ends the run, the same way a missing `cpu_percent` ends
/// [`sustained_above`]: "not measured" is an unknown and not a low
/// reading. On Windows every sample is `None`, the run is empty, and the
/// rule says nothing at all -- which is the correct answer for a
/// platform that cannot be asked the question.
fn sustained_above_load(samples: &[Sample], cores: usize, ratio: f64) -> Option<(f64, f64, f64)> {
    if cores == 0 {
        return None;
    }
    let cores = cores as f64;
    let mut total_minutes = 0.0;
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut newer_ms: Option<i64> = None;

    for s in samples.iter().rev() {
        // Absent on Windows, and absent is not zero. The run ends here.
        let Some(load) = s.load else { break };
        let reading = load[OVERSUBSCRIBED_INDEX];
        // A NaN or an infinity from the platform is not a reading. Left
        // in, a NaN loses every comparison silently -- so it would not
        // break the run, it would be averaged into a mean that is NaN
        // forever after.
        if !reading.is_finite() || reading / cores < ratio {
            break;
        }
        let Ok(at) = chrono::DateTime::parse_from_rfc3339(&s.sampled_at) else {
            break;
        };
        let ms = at.timestamp_millis();
        if let Some(newer) = newer_ms {
            let spacing = newer - ms;
            // Identical to `sustained_above`'s rule, and the same
            // constant: out-of-order or same-instant rows, and spacings
            // wider than GAP_MS, are not intervals. The run ends HERE
            // with what is already counted kept, rather than stitching
            // across hours nobody sampled.
            if spacing <= 0 || spacing > GAP_MS {
                break;
            }
            total_minutes += spacing as f64 / 60_000.0;
        }
        sum += reading;
        count += 1;
        newer_ms = Some(ms);
    }

    // One sample is a reading, not a duration -- `sustained_above` says
    // the same and for the same reason. It spans no measured time, so
    // there is nothing to compare against a minutes-long threshold.
    if count < 2 {
        return None;
    }
    let mean_load = sum / count as f64;
    Some((total_minutes, mean_load / cores, mean_load))
}

/// Whether the machine has been oversubscribed for long enough to say so
/// (#872).
///
/// `samples` is oldest-first, as `store::health::history` returns it.
/// `cores` is the logical core count, 0 when unknown -- which produces
/// `None`, never a division.
///
/// Returns `None` on every form of "cannot say": no load average
/// (Windows), an unknown core count, too short a run, or a ratio under
/// [`OVERSUBSCRIBED_RATIO`]. The caller cannot tell those apart and does
/// not need to; all of them mean the same thing, which is that nothing
/// should be shown.
///
/// # Why this is a [`Notice`] and not an [`Alert`]
///
/// Oversubscription during a large build is normal, and the measurement
/// in [`OVERSUBSCRIBED_RATIO`] is how normal: an ordinary `cargo build
/// -j12` of this repo drove the one-minute load to 4.3x core count.
/// Interrupting on that would be crying wolf at a condition the user
/// created on purpose thirty seconds earlier, and a guard that cries wolf
/// gets turned off -- #853's ~40 false positives are the receipt.
///
/// So this is an indicator on the System Health page: it reaches
/// `health_alerts` and the page, and never `notify_runaway`.
/// `nothing_converts_a_notice_into_an_alert` holds that boundary, and
/// `nothing_converts_a_shadow_into_an_alert` asserts that exactly one
/// `Alert` variant still ships -- this change deliberately adds none.
///
/// Pure, like [`evaluate`] and [`watch`]: no clock, no database, no
/// process table. The incident it exists for ran for eight and a half
/// hours, and a test must be able to state that as arithmetic rather than
/// arrange it.
pub fn oversubscribed(samples: &[Sample], cores: usize) -> Option<Notice> {
    let (minutes, ratio, load) = sustained_above_load(samples, cores, OVERSUBSCRIBED_RATIO)?;
    if minutes < OVERSUBSCRIBED_MINUTES {
        return None;
    }
    Some(Notice::Oversubscribed {
        load,
        cores,
        ratio,
        minutes,
    })
}

pub fn shadow(
    observations: &[ProcessObservation],
    durations: &std::collections::HashMap<(u32, u64), f64>,
) -> Vec<Shadow> {
    let mut out = Vec::new();
    for p in observations {
        let minutes = durations
            .get(&(p.pid, p.start_time))
            .copied()
            .unwrap_or(0.0);
        let orphaned = p.orphaned();
        let allowlisted = p.allowlisted();

        let tier = if p.cpu_percent >= TIER1_PERCENT
            && minutes >= TIER1_MINUTES
            && orphaned
            && !allowlisted
        {
            Some(("tier1", "alert"))
        } else if p.cpu_percent >= TIER2_PERCENT && minutes >= TIER2_MINUTES {
            Some(("tier2", "warn"))
        } else {
            None
        };

        if let Some((tier, severity)) = tier {
            out.push(Shadow {
                tier,
                severity,
                name: p.name.clone(),
                cpu_percent: p.cpu_percent,
                minutes,
                orphaned,
                allowlisted,
            });
        }
    }
    out
}

/// The in-memory per-process duration approximation behind the shadow
/// log.
///
/// # Why in memory, and why an approximation
///
/// #791 asks for per-PID duration tracking keyed on
/// `(pid, start_time)`, persisted so it survives a restart. That is
/// deferred: it needs a schema migration, and it is only worth its
/// migration once a tier actually notifies. A shadow log does not need
/// to survive anything -- a process whose accumulated burn is forgotten
/// on relaunch is a log line that does not appear, and the
/// distribution is thinner by one entry. So this is a `HashMap` in the
/// sampler's own thread and nothing else. **No storage, no migration.**
///
/// The approximation is therefore deliberately conservative in one
/// direction: it can only ever UNDER-report. A process that has burned
/// for two days shows up here with however many minutes the app has
/// been open for, never more. That is the right direction for the
/// failure it is avoiding -- see the gap discussion in the module docs
/// -- because over-reporting is what would invent sustained burn the
/// app never watched.
///
/// # Gap discipline, again
///
/// [`Watcher::observe`] takes the elapsed milliseconds since the
/// previous pass and REFUSES to credit a span wider than [`GAP_MS`].
/// The sampler's own clock is what produces that number, and across a
/// closed lid or an app restart it is hours: crediting it would hand
/// every process on the machine half a day of "sustained" burn the
/// moment the app reopens. An over-wide span resets the accumulation
/// instead, which is the same decision `sustained_above` makes for the
/// stored series.
#[derive(Debug, Default)]
pub struct Watcher {
    /// `(pid, start_time)` to minutes observed at or above the floor.
    minutes: std::collections::HashMap<(u32, u64), f64>,
}

impl Watcher {
    /// Credit `elapsed_ms` to every observation still above the floor,
    /// reset the ones that dropped below it, and forget the ones that
    /// are gone.
    ///
    /// The floor is [`TIER2_PERCENT`] or [`TIER1_PERCENT`], whichever
    /// is lower: one accumulator serves both tiers, and tracking from
    /// the lower floor is what lets a process that crosses into tier 2
    /// territory already have its tier-1 duration behind it.
    ///
    /// Returns the duration map [`shadow`] reads, so a caller cannot
    /// evaluate against a map it forgot to update.
    pub fn observe(
        &mut self,
        observations: &[ProcessObservation],
        elapsed_ms: i64,
    ) -> &std::collections::HashMap<(u32, u64), f64> {
        // The LOWEST floor any rule reads, which is now #865's watch
        // band rather than tier 1's. One accumulator serves every tier,
        // and tracking from the lowest floor is what lets a process that
        // climbs into a higher tier already have its duration behind it.
        //
        // This line was `TIER1_PERCENT.min(TIER2_PERCENT)` -- 80% -- and
        // `watch` would have been decorative without changing it: a
        // process at 50% was never accumulated, so its duration was
        // always 0.0 and a five-minute rule could never fire. Any new
        // tier with a lower floor has to appear here too.
        let floor = WATCH_PERCENT.min(TIER1_PERCENT).min(TIER2_PERCENT);
        // A span nobody measured credits nothing. Past GAP_MS the two
        // passes are not comparable, so the accumulated minutes are
        // dropped rather than extended -- the same refusal as
        // `sustained_above`, applied to the live side.
        let credit = if elapsed_ms > 0 && elapsed_ms <= GAP_MS {
            elapsed_ms as f64 / 60_000.0
        } else {
            self.minutes.clear();
            0.0
        };

        let mut next = std::collections::HashMap::with_capacity(observations.len());
        for p in observations {
            if p.cpu_percent < floor {
                // Dropped below the floor: the run is over, and the
                // next one starts from zero rather than resuming. A
                // process that is busy for a minute every hour has not
                // been busy for thirty minutes.
                continue;
            }
            let key = (p.pid, p.start_time);
            let so_far = self.minutes.get(&key).copied().unwrap_or(0.0);
            next.insert(key, so_far + credit);
        }
        // Replaced rather than merged, so a process that has EXITED is
        // forgotten. Left to grow, this map would be an unbounded leak
        // in a loop that runs forever -- and a recycled PID would
        // inherit a dead process's minutes, which is precisely what the
        // start_time half of the key exists to prevent.
        self.minutes = next;
        &self.minutes
    }
}

/// Reads the process table for the shadow log.
///
/// Its own `sysinfo::System` rather than a call into
/// `health::footprint`: the rules need `parent()` and `start_time()`,
/// which `footprint::Process` does not carry, and `Footprint` is being
/// reshaped by #795/#796. A refresh asks for CPU only -- not memory,
/// not tasks, not command lines -- because that is all the two rules
/// read.
///
/// Like `Footprints`, ONE instance per process and held across calls:
/// `sysinfo` reports CPU use since the previous refresh of the same
/// `System`, so a fresh instance per pass would report an idle machine
/// forever.
pub struct Table {
    system: std::sync::Mutex<sysinfo::System>,
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

impl Table {
    pub fn new() -> Self {
        Self {
            system: std::sync::Mutex::new(sysinfo::System::new()),
        }
    }

    /// Two passes [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] apart, for a
    /// caller that does not already have a previous reading.
    ///
    /// # Why this exists, and why it SLEEPS
    ///
    /// `sysinfo` reports CPU use as a delta since the previous refresh
    /// of the same `System`, and it refuses to recompute one inside
    /// `MINIMUM_CPU_UPDATE_INTERVAL` -- so two back-to-back [`read`]
    /// calls yield a second reading of ZERO for every process. That is
    /// not a cosmetic zero here: a zero `top_cpu_percent` makes
    /// [`Aggregate::one_process_explains_it`] answer `Some(false)`, which
    /// is the clause that lets [`evaluate`] fire. A caller that forgot
    /// the interval would therefore report "no single process explains
    /// it" during a `yarn build` -- exactly the false positive
    /// `a_legitimate_build_does_not_fire` exists to prevent, reintroduced
    /// one layer down where that test cannot see it.
    ///
    /// So the wait lives HERE rather than at each call site, and the
    /// sleep is the honest cost of asking a question that is defined as
    /// a rate. The sampler loop does not use this -- it holds a `Table`
    /// across its sixty-second ticks and already has an interval, which
    /// is the whole reason the instance is long-lived.
    ///
    /// Blocking for the length of the interval (~200ms), so callers keep
    /// it on a blocking worker.
    ///
    /// [`read`]: Table::read
    pub fn read_twice(&self) -> (Vec<ProcessObservation>, Aggregate) {
        let _ = self.read();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        self.read()
    }

    /// One pass over the process table: the observations, and the
    /// aggregate rule's live half.
    ///
    /// The CPU figures are a delta since the PREVIOUS pass of this same
    /// `Table`, so a caller with no previous pass wants [`read_twice`]
    /// instead -- see its docs on the false positive a missing interval
    /// causes.
    ///
    /// Blocking -- it reads the kernel -- so callers keep it on the
    /// sampler's own thread, which is already blocking by design.
    ///
    /// [`read_twice`]: Table::read_twice
    pub fn read(&self) -> (Vec<ProcessObservation>, Aggregate) {
        let mut sys = self.system.lock().unwrap_or_else(|e| e.into_inner());
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cpu(),
        );

        let mut observations = Vec::with_capacity(sys.processes().len());
        let mut top = 0.0f64;
        for (pid, proc) in sys.processes() {
            let cpu = f64::from(proc.cpu_usage());
            // NaN is skipped rather than compared: `sysinfo` yields one
            // where a platform's accounting failed, and a NaN loses
            // every comparison silently -- so a NaN left in would make
            // `top` wrong in a way nothing would ever surface.
            if cpu.is_finite() && cpu > top {
                top = cpu;
            }
            observations.push(ProcessObservation {
                pid: pid.as_u32(),
                start_time: proc.start_time(),
                name: proc.name().to_string_lossy().to_string(),
                cpu_percent: cpu,
                parent: proc.parent().map(|p| p.as_u32()),
                // One syscall per process. Measured rather than assumed
                // safe: see `a_sample_stays_cheap_with_nice_reads`.
                nice: nice_of(pid.as_u32()),
            });
        }
        let process_count = observations.len();
        drop(sys);

        // `available_parallelism` rather than a second `sysinfo` call:
        // the `System` above was refreshed for processes only, so its
        // CPU list is not populated, and refreshing CPUs here purely
        // for a core count would do a second kernel pass for a number
        // the standard library already has.
        //
        // `Err` becomes 0, which `Aggregate::largest_share` reads as
        // "unknown" and which makes `evaluate` stay silent -- absent is
        // not a default, the same rule as everywhere else in `health`.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0);

        (
            observations,
            Aggregate {
                top_cpu_percent: top,
                cores,
                process_count,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Memory, Sample};
    use std::collections::HashMap;

    /// A series at the sampler's own cadence, ending now: each entry is
    /// a machine-wide CPU percentage, oldest first, one minute apart.
    fn series(points: &[f64]) -> Vec<Sample> {
        let end = chrono::Utc::now();
        points
            .iter()
            .enumerate()
            .map(|(i, cpu)| {
                let at = end - chrono::Duration::minutes((points.len() - 1 - i) as i64);
                let mut s = bare(&at.to_rfc3339());
                s.cpu_percent = Some(*cpu);
                s
            })
            .collect()
    }

    fn bare(at: &str) -> Sample {
        Sample {
            sampled_at: at.to_string(),
            load: None,
            cpu_percent: None,
            cpu_per_core: vec![],
            memory: Memory {
                total: 1,
                used: 1,
                available: 0,
                swap_total: 0,
                swap_used: 0,
            },
            gpus: vec![],
            disks: vec![],
            battery: None,
            thermal: None,
            networks: vec![],
            uptime_secs: 1,
        }
    }

    /// A ten-core machine whose busiest process holds `percent` of ONE
    /// core, with `count` processes running.
    fn live(percent: f64, count: usize) -> Aggregate {
        Aggregate {
            top_cpu_percent: percent,
            cores: 10,
            process_count: count,
        }
    }

    /// Twenty minutes at 70% with the biggest process holding 93% of
    /// one core -- 9.3% of a ten-core machine, so nothing explains the
    /// load. This is the incident in #791, and it MUST fire: ten
    /// processes at 93% each is a different condition from one at 93%,
    /// and only an aggregate rule sees it.
    #[test]
    fn a_diffuse_load_that_nothing_explains_fires() {
        let s = series(&[70.0; 20]);
        let alerts = evaluate(&s, Some(&live(93.0, 1436)));
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].key(), "diffuse_cpu");
        match &alerts[0] {
            Alert::DiffuseCpu {
                percent,
                minutes,
                process_count,
            } => {
                assert!((percent - 70.0).abs() < 0.01, "{percent}");
                assert!(*minutes >= AGGREGATE_MINUTES, "{minutes}");
                assert_eq!(*process_count, 1436);
            }
        }
    }

    /// **The false-positive test this module exists to pass.**
    ///
    /// A `yarn build` or a Rust compile pins the machine for far longer
    /// than fifteen minutes, and it is not a fault. What separates it
    /// from the incident is that ONE process is the load: here the
    /// compiler holds 600% of one core, six whole cores of a ten-core
    /// machine, well past half of it.
    ///
    /// If this test fails, the rule fires on every build on the machine
    /// the issue came from, and the alerts get ignored within a day.
    #[test]
    fn a_legitimate_build_does_not_fire() {
        let s = series(&[85.0; 40]);
        let alerts = evaluate(&s, Some(&live(600.0, 412)));
        assert!(
            alerts.is_empty(),
            "one process holding six of ten cores IS the explanation: {alerts:?}"
        );
    }

    /// **The mutation test for #791's gap requirement.**
    ///
    /// The ten `yes` processes burned for two days while the app was
    /// closed. The first sample after reopening sits next to the last
    /// one before it, and both are high -- so a rule that measures
    /// duration as "time between the oldest and newest high sample"
    /// sees two days of burn, or at best fifteen minutes of a window
    /// nobody watched.
    ///
    /// If this fails, a two-day-old condition is "detected" the instant
    /// the app reopens, with a duration the app did not measure.
    #[test]
    fn a_gap_is_not_sustained_burn() {
        let end = chrono::Utc::now();
        let at = |mins: i64| (end - chrono::Duration::minutes(mins)).to_rfc3339();
        let mut samples = Vec::new();
        // Twenty high samples, then the app was closed for two days.
        for mins in (2880..2900).rev() {
            let mut s = bare(&at(mins));
            s.cpu_percent = Some(75.0);
            samples.push(s);
        }
        // Reopened: three high samples, which is three minutes of
        // measured time and nothing like fifteen.
        for mins in [2, 1, 0] {
            let mut s = bare(&at(mins));
            s.cpu_percent = Some(75.0);
            samples.push(s);
        }

        let alerts = evaluate(&samples, Some(&live(50.0, 1436)));
        assert!(
            alerts.is_empty(),
            "a duration must never be measured across a period nobody sampled: {alerts:?}"
        );
    }

    /// And the other half of that test: the run either side of the gap
    /// is not merged, so the POST-gap run alone is what counts -- and
    /// once it is long enough on its own, it fires.
    #[test]
    fn the_run_after_a_gap_is_measured_on_its_own() {
        let end = chrono::Utc::now();
        let at = |mins: i64| (end - chrono::Duration::minutes(mins)).to_rfc3339();
        let mut samples = Vec::new();
        for mins in (2880..2890).rev() {
            let mut s = bare(&at(mins));
            s.cpu_percent = Some(75.0);
            samples.push(s);
        }
        for mins in (0..20).rev() {
            let mut s = bare(&at(mins));
            s.cpu_percent = Some(75.0);
            samples.push(s);
        }
        let alerts = evaluate(&samples, Some(&live(50.0, 900)));
        assert_eq!(alerts.len(), 1, "nineteen measured minutes: {alerts:?}");
    }

    /// A brief spike is not a runaway. Five minutes of high CPU is a
    /// test run.
    #[test]
    fn a_short_burst_says_nothing() {
        let s = series(&[90.0; 5]);
        assert!(evaluate(&s, Some(&live(20.0, 900))).is_empty());
    }

    /// A dip below the floor ends the run, so the duration restarts
    /// from the dip rather than spanning it. Otherwise a machine that
    /// is busy half the time would accumulate to fifteen minutes and
    /// report a sustained condition it never had.
    #[test]
    fn a_dip_below_the_floor_ends_the_run() {
        let mut points = vec![70.0; 20];
        // Newest three are high, but the fourth-newest dipped.
        points[16] = 20.0;
        let s = series(&points);
        assert!(
            evaluate(&s, Some(&live(20.0, 900))).is_empty(),
            "only the three newest samples are in the run"
        );
    }

    /// A missing reading breaks the run the same way a gap does:
    /// "not measured" is an unknown, not a low number.
    #[test]
    fn a_missing_cpu_reading_is_not_a_low_one() {
        let mut s = series(&[70.0; 20]);
        s[16].cpu_percent = None;
        assert!(evaluate(&s, Some(&live(20.0, 900))).is_empty());
    }

    /// No live process table means the "no single process explains it"
    /// clause cannot be checked, and that clause is the whole content
    /// of the alert. Silence, not a firing.
    #[test]
    fn an_unreadable_process_table_is_silence() {
        let s = series(&[70.0; 20]);
        assert!(evaluate(&s, None).is_empty());
    }

    /// An unknown core count is "cannot say", not "nothing explains
    /// it". A zero would otherwise divide into an infinity that clears
    /// the threshold and fires on every busy build.
    #[test]
    fn an_unknown_core_count_is_silence() {
        let s = series(&[70.0; 20]);
        let unknown = Aggregate {
            top_cpu_percent: 600.0,
            cores: 0,
            process_count: 900,
        };
        assert!(evaluate(&s, Some(&unknown)).is_empty());
        assert_eq!(unknown.largest_share(), None);
        assert_eq!(unknown.one_process_explains_it(), None);
    }

    /// An empty series is not a quiet machine, it is no information.
    #[test]
    fn an_empty_series_says_nothing() {
        assert!(evaluate(&[], Some(&live(20.0, 900))).is_empty());
        assert!(evaluate(&series(&[90.0]), Some(&live(20.0, 900))).is_empty());
    }

    /// The per-core / per-machine conversion, explicitly. 93% of one
    /// core on a ten-core machine is 9.3% of the machine, and reading
    /// the two numbers as comparable is the mistake that would have
    /// called a single `yes` process "the explanation".
    #[test]
    fn one_cores_worth_is_not_the_whole_machine() {
        let a = live(93.0, 10);
        let share = a.largest_share().expect("ten cores is known");
        assert!((share - 0.093).abs() < 0.0001, "{share}");
        assert_eq!(a.one_process_explains_it(), Some(false));

        let busy = live(600.0, 10);
        assert_eq!(busy.one_process_explains_it(), Some(true));
    }

    // ---- The watch tier (#865) --------------------------------------

    /// A stale answer is WITHHELD rather than served (#908).
    ///
    /// The expiry is the whole point of stamping the set. `set` replaces the
    /// list each pass, so an exited process drops out -- but only while the
    /// loop runs. If the poll thread dies, the last value would sit in the
    /// mutex forever and the page would render it identically to a fresh
    /// answer.
    #[test]
    fn a_stale_answer_is_withheld() {
        let w = Watched::default();
        assert!(w.get().is_none(), "nothing before the first pass");

        let p = proc(910, "node", 60.0, Some(42));
        w.set(watch(std::slice::from_ref(&p), &durations(&[(&p, 10.0)])));
        let (age, notices) = w.get().expect("a fresh pass is served");
        assert_eq!(notices.len(), 1);
        assert!(age < NOTICE_MAX_AGE, "just written, so young");

        // Reach past the accessor to age the stamp, because the alternative
        // is sleeping for three minutes in a unit test -- a wall-clock wait
        // is exactly what `invariants.rs` Invariant 8 exists to keep out.
        {
            let mut held = w.0.lock().expect("not poisoned");
            let (at, kept) = held.take().expect("just set");
            *held = Some((
                at - NOTICE_MAX_AGE - std::time::Duration::from_secs(1),
                kept,
            ));
        }
        assert!(
            w.get().is_none(),
            "past NOTICE_MAX_AGE the answer is withheld, not served stale"
        );
    }

    /// Processes of one name are ONE row, counting them (#908).
    ///
    /// `key` is derived from the name, so per-process notices collided: two
    /// `zsh` processes produced two notices with an identical key, which
    /// rendered as indistinguishable repeated blocks. A user reported that
    /// as a suspected staleness bug -- the notices were correct and there
    /// was no way to tell them apart.
    #[test]
    fn processes_of_one_name_collapse_into_a_counted_row() {
        let a = proc_niced(920, "zsh", 55.0, Some(1), Some(5));
        let b = proc_niced(921, "zsh", 99.0, Some(42), None);
        let c = proc(922, "node", 60.0, Some(42));
        let out = watch(
            &[a.clone(), b.clone(), c.clone()],
            &durations(&[(&a, 6.0), (&b, 40.0), (&c, 10.0)]),
        );

        assert_eq!(out.len(), 2, "two names, two rows");
        let zsh = out
            .iter()
            .find(|n| n.title().contains("zsh"))
            .expect("a zsh row");
        assert_eq!(zsh.count(), 2);
        assert!(
            zsh.title().starts_with("2 zsh processes"),
            "{}",
            zsh.title()
        );
        // WORST of each, not a mean: the question is whether anything is
        // pegging a core and for how long.
        assert!(
            (zsh.cpu_percent() - 99.0).abs() < f64::EPSILON,
            "the worst CPU, not an average"
        );
        assert!(
            (zsh.minutes() - 40.0).abs() < f64::EPSILON,
            "the longest run"
        );
        // OR-ed, because these raise the wording: one niced member is
        // enough for the sentence to be worth saying.
        assert!(zsh.niced(), "one of the two is niced");
        assert!(zsh.orphaned(), "one of the two is orphaned");

        // The pid is the ONE field that is not a worst-of or an OR (#943).
        // "2 zsh processes have been busy" is about two pids and no single
        // one of them, so keeping either would name a process the row is not
        // about -- the same mistake the grouped process table avoids by
        // suppressing its PID column.
        assert_eq!(zsh.pid(), None, "a collapsed row names no single process");
        // And the single-process row DOES carry one, in the same test, so
        // this cannot pass by `pid()` always answering None.
        let node = out
            .iter()
            .find(|n| n.title().contains("node"))
            .expect("a node row");
        assert_eq!(node.count(), 1);
        assert_eq!(node.pid(), Some(922), "the one process it is about");
    }

    /// The pid a watch notice is about, and the three ways there isn't one
    /// (#943).
    ///
    /// The notice's own body ends "Worth a look if you did not start
    /// something long-running", and the thing a reader looks WITH is a pid to
    /// type into `ps`. It was already in hand at the observation -- `watch`
    /// reads `durations.get(&(p.pid, p.start_time))` two lines above the
    /// `Notice::Process` it builds -- and stopped there.
    #[test]
    fn a_single_process_notice_carries_the_pid_it_is_about() {
        let p = proc(4242, "acme-loop", 58.0, Some(1));
        let out = watch(std::slice::from_ref(&p), &durations(&[(&p, 12.0)]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid(), Some(4242));

        // The machine-wide variant is about no process at all, and inventing
        // a pid there would be worse than inventing "system" as a name: a
        // pid that names the wrong process is indistinguishable from one that
        // names the right one.
        let machine = Notice::Oversubscribed {
            load: 53.0,
            cores: 12,
            ratio: 4.4,
            minutes: 510.0,
        };
        assert_eq!(machine.pid(), None);
        assert_eq!(machine.name(), None, "for the same reason");
    }

    /// The pid must NOT reach `key` (#943).
    ///
    /// `key` is the condition's identity, and #908's collapse-by-name depends
    /// on two `zsh` notices sharing one. A pid is stable for the life of a
    /// process but not across a restart, so keying on it would turn a
    /// standing condition into a new row every time the thing restarted --
    /// and would break the collapse, since two processes of one name have
    /// two pids.
    #[test]
    fn the_pid_stays_out_of_the_key() {
        let a = proc(1, "zsh", 58.0, Some(1));
        let b = proc(2, "zsh", 58.0, Some(1));
        let one = watch(std::slice::from_ref(&a), &durations(&[(&a, 12.0)]));
        let other = watch(std::slice::from_ref(&b), &durations(&[(&b, 12.0)]));

        assert_eq!(one[0].key(), other[0].key(), "one condition, one key");
        assert_ne!(one[0].pid(), other[0].pid(), "but different processes");
    }

    /// The age reaches the body, and reads as prose below a minute (#908).
    #[test]
    fn a_body_carries_the_age_of_its_evidence() {
        let fresh = with_age("x", std::time::Duration::from_secs(5));
        assert!(fresh.ends_with("(as of just now)"), "{fresh}");
        let one = with_age("x", std::time::Duration::from_secs(60));
        assert!(one.ends_with("(as of 1 minute ago)"), "{one}");
        let many = with_age("x", std::time::Duration::from_secs(185));
        assert!(many.ends_with("(as of 3 minutes ago)"), "{many}");
    }

    /// THE incident, as a fixture. Twelve orphaned busy-loops, niced to
    /// 5, each holding ~50% of a core on a 12-core machine for 8.5
    /// hours, load average 53 -- and System Health said nothing for the
    /// whole of it.
    ///
    /// Kept as a test rather than a note because it is the only
    /// real-world sample this module has of its own central failure, and
    /// because the aggregate rule it was supposed to trip CANNOT see it:
    /// 12 x 50% on 12 cores is 50% machine-wide, under
    /// `AGGREGATE_PERCENT`'s 60. Any rule set that does not fire here is
    /// not finished.
    #[test]
    fn the_twelve_spinner_incident_is_surfaced() {
        let obs: Vec<ProcessObservation> = (0..12)
            .map(|i| proc_niced(13_574 + i, "zsh", 50.0, Some(1), Some(5)))
            .collect();
        let pairs: Vec<(&ProcessObservation, f64)> = obs.iter().map(|p| (p, 8.5 * 60.0)).collect();

        let out = watch(&obs, &durations(&pairs));

        // ONE row, counting twelve (#908). This asserted 12 rows until the
        // user who hit this incident reported the result as a suspected
        // staleness bug: twelve notices sharing one `key` rendered as
        // indistinguishable repeated blocks. The condition is still fully
        // surfaced -- the count is on the row -- and the deduplication
        // contract `key` exists for is no longer broken.
        assert_eq!(out.len(), 1, "twelve spinners are ONE condition");
        assert_eq!(out[0].count(), 12, "and the row says there are twelve");
        let n = &out[0];
        assert!(
            n.niced(),
            "nice 5 is recorded -- it is why the aggregate rule missed these"
        );
        assert!(n.orphaned(), "PPID 1");
        assert!(n.long(), "8.5 hours is well past the long mark");
        assert!(
            n.body().contains("low priority"),
            "the body says WHY it is suspicious"
        );
        assert!(n.body().contains("parent has exited"));
    }

    /// The user's own discriminator: "60% for 2-3 minutes would be a
    /// compiler". A short hot burst is silence at any level.
    #[test]
    fn a_compiler_is_not_surfaced() {
        let p = proc(900, "rustc", 98.0, Some(42));
        let out = watch(std::slice::from_ref(&p), &durations(&[(&p, 2.5)]));
        assert!(
            out.is_empty(),
            "two and a half minutes is a build, not a runaway"
        );
    }

    /// And the case the old tiers could not express at all: half a core,
    /// parented, past five minutes. Tier 1 needed 80% AND PPID 1; tier 2
    /// needed 90%. This matched nothing before #865.
    #[test]
    fn half_a_core_past_five_minutes_is_surfaced_even_when_parented() {
        let p = proc(901, "node", 52.0, Some(42));
        let out = watch(std::slice::from_ref(&p), &durations(&[(&p, 6.0)]));
        assert_eq!(out.len(), 1);
        assert!(!out[0].orphaned(), "parented, and surfaced anyway");
        assert!(!out[0].niced());
        assert!(
            !out[0].long(),
            "six minutes is worth a look, not yet a long burn"
        );
    }

    /// Just under each threshold, so the boundaries are asserted rather
    /// than assumed.
    #[test]
    fn the_watch_boundaries_hold() {
        let quiet = proc(902, "a", WATCH_PERCENT - 0.1, Some(42));
        assert!(
            watch(std::slice::from_ref(&quiet), &durations(&[(&quiet, 60.0)])).is_empty(),
            "below the level floor, however long"
        );
        let brief = proc(903, "b", 99.0, Some(42));
        assert!(
            watch(
                std::slice::from_ref(&brief),
                &durations(&[(&brief, WATCH_MINUTES - 0.1)])
            )
            .is_empty(),
            "above the level, below the duration"
        );
    }

    /// An allowlisted daemon is excluded, the same as tier 1 excludes
    /// them: `mds_stores` holding a core during a reindex is the
    /// machine working, not a fault.
    #[test]
    fn a_known_daemon_is_not_surfaced() {
        let d = proc(904, SEED_DAEMONS[0], 95.0, Some(1));
        assert!(watch(std::slice::from_ref(&d), &durations(&[(&d, 600.0)])).is_empty());
    }

    /// `None` nice changes nothing. Windows reports none, and a process
    /// that exits mid-walk reports none -- neither is evidence either
    /// way, so the notice stands on its level and duration alone.
    #[test]
    fn an_unreadable_nice_neither_raises_nor_suppresses() {
        let p = proc_niced(905, "c", 60.0, Some(42), None);
        let out = watch(std::slice::from_ref(&p), &durations(&[(&p, 10.0)]));
        assert_eq!(out.len(), 1, "surfaced on level and duration alone");
        assert!(!out[0].niced(), "unknown is not 'niced'");
        assert!(!out[0].body().contains("low priority"));
    }

    /// The accumulator has to track from the WATCH floor or this whole
    /// tier is decorative: a 50% process would never be credited any
    /// minutes, so a five-minute rule could never fire however long it
    /// ran. This asserts the floor, not the arithmetic.
    #[test]
    fn the_accumulator_tracks_the_watch_band() {
        let mut w = Watcher::default();
        let p = proc(906, "d", WATCH_PERCENT + 1.0, Some(42));
        // Six minutes of credit, in one-minute steps.
        for _ in 0..6 {
            w.observe(std::slice::from_ref(&p), 60_000);
        }
        let mins = w
            .observe(std::slice::from_ref(&p), 60_000)
            .get(&(p.pid, p.start_time))
            .copied()
            .expect("a process in the watch band is tracked");
        assert!(
            mins >= WATCH_MINUTES,
            "{mins} minutes accumulated at the watch floor"
        );
    }

    /// Longest first, so a truncated display keeps the worst one.
    #[test]
    fn notices_are_ordered_by_duration() {
        let a = proc(907, "young", 60.0, Some(42));
        let b = proc(908, "old", 55.0, Some(42));
        let out = watch(
            &[a.clone(), b.clone()],
            &durations(&[(&a, 6.0), (&b, 400.0)]),
        );
        assert_eq!(out[0].name(), Some("old"));
    }

    // ---- Oversubscription, from load average (#872) ------------------

    /// A series of FIFTEEN-minute load averages at the sampler's own
    /// cadence, ending now: oldest first, one minute apart.
    ///
    /// # The one- and five-minute slots are deliberately LOW, not equal
    ///
    /// They were equal in the first draft of these tests, and sabotage
    /// proved that made `OVERSUBSCRIBED_INDEX` -- the single most
    /// load-bearing decision in the rule -- untestable: changing the
    /// constant from 2 to 0 left all 47 tests PASSING, because every slot
    /// held the same number. That is the v5.14.0 lesson again, in the one
    /// place it would have cost the most.
    ///
    /// So the shorter windows carry a quiet machine's figures while index
    /// 2 carries the oversubscribed one. The combination is not artificial
    /// -- it is what the tail of a long runaway looks like once the
    /// one-minute average has settled -- and it means a rule reading the
    /// wrong index reports silence on the incident fixture and fails
    /// loudly.
    fn load_series(points: &[f64]) -> Vec<Sample> {
        let end = chrono::Utc::now();
        points
            .iter()
            .enumerate()
            .map(|(i, la)| {
                let at = end - chrono::Duration::minutes((points.len() - 1 - i) as i64);
                let mut s = bare(&at.to_rfc3339());
                // 0.5 and 1.0: well under core count on any machine these
                // tests use, so only index 2 can satisfy the ratio.
                s.load = Some([0.5, 1.0, *la]);
                s
            })
            .collect()
    }

    /// **THE incident, as a fixture, read through load average (#872).**
    ///
    /// 12 cores, load average 53, sustained 8.5 hours. That is 4.4
    /// runnable threads per core -- the machine was not busy, it was
    /// buried -- and `the_twelve_spinner_incident_is_surfaced` above
    /// shows what every OTHER rule in this module saw instead: 50%
    /// machine-wide CPU, comfortably under `AGGREGATE_PERCENT`'s 60.
    ///
    /// This is the one signal that was unambiguous at the time and the one
    /// nothing read. A rule set that stays quiet here is not finished.
    #[test]
    fn the_twelve_spinner_incident_is_oversubscribed() {
        // 8.5 hours of samples would be 510 entries; the rule measures a
        // run over CONSECUTIVE samples, so thirty of them (29 minutes) is
        // already far past `OVERSUBSCRIBED_MINUTES` and the arithmetic is
        // identical. The duration is asserted against the threshold, not
        // against 510.
        let s = load_series(&[53.0; 30]);

        let n = oversubscribed(&s, 12).expect("load 53 on 12 cores for half an hour must surface");

        match &n {
            Notice::Oversubscribed {
                load,
                cores,
                ratio,
                minutes,
            } => {
                assert!((load - 53.0).abs() < 0.01, "{load}");
                assert_eq!(*cores, 12);
                // 53/12 = 4.416..., which is what "catastrophically
                // oversubscribed" looks like as a number.
                assert!((ratio - 53.0 / 12.0).abs() < 0.01, "{ratio}");
                assert!(*minutes >= OVERSUBSCRIBED_MINUTES, "{minutes}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(n.key(), "cpu_oversubscribed");
        assert!(
            n.body().contains("53"),
            "the body says the load: {}",
            n.body()
        );
        assert!(
            n.body().contains("12 cores"),
            "and the core count: {}",
            n.body()
        );
    }

    /// **The false-positive test this rule exists to pass, from real
    /// measurement rather than reasoning.**
    ///
    /// `cargo build -j12` of THIS repository on a 12-core machine,
    /// 2026-09-12, `vm.loadavg` sampled every 5-6 seconds across a
    /// 68-second build. These are the fifteen-minute readings, one per
    /// minute, through the build and its link phase:
    ///
    /// ```text
    ///   1.83 2.31 3.16 3.27 3.25 3.22 3.19 3.13 3.38 3.53
    ///   5.88 7.57 7.71 7.86 8.00 8.06 7.90 7.71 7.45 7.23
    /// ```
    ///
    /// The peak is **8.06 on 12 cores: 0.67x**, and it never reached core
    /// count at all. Meanwhile the ONE-minute average in the same run
    /// peaked at 51.78 (4.31x) and the five-minute at 16.86 (1.40x) --
    /// which is the measurement that chose `OVERSUBSCRIBED_INDEX`, and the
    /// reason a rule reading index 0 or 1 would fire on every build on
    /// this machine.
    ///
    /// If this test fails the guard cries wolf on ordinary work, and a
    /// guard that cries wolf gets turned off -- #853 already paid ~40
    /// false positives for that lesson once.
    #[test]
    fn a_parallel_build_is_not_oversubscribed() {
        // The whole run, rise through decay, one reading per minute.
        let measured = [
            1.83, 2.31, 3.16, 3.27, 3.25, 3.22, 3.19, 3.13, 3.38, 3.53, 5.88, 7.57, 7.71, 7.86,
            8.00, 8.06, 7.90, 7.71, 7.45, 7.23,
        ];
        assert!(
            oversubscribed(&load_series(&measured), 12).is_none(),
            "a real `cargo build -j12` must stay silent"
        );

        // And again truncated at its WORST moment, which is the case that
        // actually tests the threshold. The rule measures backwards from
        // the newest sample, so a series ending on the decay tail breaks
        // its run early and would pass at almost any ratio -- sabotage
        // proved exactly that, with `OVERSUBSCRIBED_RATIO` lowered to 0.6
        // and the full series above still silent. Ending the series at the
        // 8.06 peak removes that accident: every sample in the run is at
        // or near the build's maximum, so the assertion is about the ratio
        // and nothing else.
        let peak = &measured[..16];
        assert!(
            (peak[15] - 8.06).abs() < 0.01,
            "the truncation ends on the measured peak"
        );
        assert!(
            oversubscribed(&load_series(peak), 12).is_none(),
            "the build's worst fifteen-minute reading is 8.06 on 12 cores -- 0.67x, and \
             OVERSUBSCRIBED_RATIO is {OVERSUBSCRIBED_RATIO}"
        );
        // Measured by sabotage, and worth stating because it is not what
        // the paragraph above implies: the ratio ALONE does not exclude
        // this build. Lowering `OVERSUBSCRIBED_RATIO` to 0.6 leaves this
        // test passing, because the run then breaks at the 5.88 sample
        // four minutes back and `OVERSUBSCRIBED_MINUTES` refuses it.
        // Lowering BOTH -- ratio to 0.6 and minutes to 3 -- fails it. The
        // two clauses exclude the build together, and a future change to
        // either one has this fixture standing behind it.

        // The reading the rule deliberately does NOT use, from the same
        // run, to show what choosing the fifteen-minute window bought: the
        // FIVE-minute average held above core count for three and a half
        // minutes during this very build, so a rule at index 1 and any
        // ratio at or under 1.4 would have fired on it.
        let five_minute = [
            12.26, 15.98, 16.12, 16.65, 16.86, 16.60, 16.35, 16.11, 15.86, 15.67, 15.26, 15.03,
            14.81,
        ];
        assert!(
            five_minute.iter().all(|la| la / 12.0 >= 1.0),
            "every one of these five-minute readings is above core count, which is why \
             OVERSUBSCRIBED_INDEX is 2 and not 1"
        );
    }

    /// **Windows: `load` is `None` by design, and the rule must stay
    /// SILENT rather than read it as zero.**
    ///
    /// `collect.rs` maps Windows' three zeroes to `None`, because Windows
    /// has no load average and three zeroes are indistinguishable from a
    /// genuinely idle machine. Absent is not zero -- the characteristic
    /// bug class of this module.
    ///
    /// # What the silence assertion can and cannot prove
    ///
    /// Recorded because sabotaging the code proved the obvious version of
    /// this test VACUOUS, which is the v5.14.0 lesson arriving again:
    /// replacing `let Some(load) = s.load else { break }` with
    /// `s.load.unwrap_or([0.0; 3])` -- absent-is-not-zero committed
    /// outright -- and the first two assertions below still PASSED. They
    /// have to: a defaulted 0.0 is below every ratio, so it breaks the run
    /// at the same place the `else` branch does, and the rule is silent
    /// either way. "Stays quiet on Windows" is the same observable for the
    /// right reason and the wrong one.
    ///
    /// So the third clause is the one that carries the guard, and it is
    /// here rather than in a test of its own for exactly that reason. It
    /// puts a `None` part-way through a high series: absent must END the
    /// run, as a missing `cpu_percent` does in `sustained_above`, leaving
    /// only the four samples after the hole. A `continue` that skipped the
    /// hole -- the other shape absent-is-not-zero takes, and the one that
    /// INVENTS duration rather than losing it -- fails it, reporting 29
    /// minutes of load 53 across a series that was never continuous.
    ///
    /// The two vacuous assertions are kept anyway: they are cheap, they
    /// document the platform contract, and they would catch a `Some([0.0;
    /// 3])` fallback substituted in `collect.rs` if the rule ever grew a
    /// clause that read a zero as information.
    #[test]
    fn windows_has_no_load_average_and_says_nothing() {
        // `bare` leaves `load: None`, which is exactly what a Windows
        // sample looks like.
        let end = chrono::Utc::now();
        let windows: Vec<Sample> = (0..30)
            .map(|i| bare(&(end - chrono::Duration::minutes(29 - i)).to_rfc3339()))
            .collect();
        assert!(
            windows.iter().all(|s| s.load.is_none()),
            "the fixture is a Windows series"
        );
        assert!(
            oversubscribed(&windows, 12).is_none(),
            "no load average is 'cannot say', never 'zero'"
        );

        // The same series with the incident's load present DOES fire, so
        // the silence above is attributable to the absent reading and not
        // to something else about the fixture.
        assert!(
            oversubscribed(&load_series(&[53.0; 30]), 12).is_some(),
            "the only difference is whether `load` was readable"
        );

        // And a `None` part-way through ENDS the run rather than being
        // skipped, the same as a missing `cpu_percent` in
        // `sustained_above`: a mixed series cannot be measured across the
        // hole.
        let mut mixed = load_series(&[53.0; 30]);
        mixed[25].load = None;
        let after_the_hole = oversubscribed(&mixed, 12);
        assert!(
            after_the_hole.is_none(),
            "only four samples sit after the missing reading: {after_the_hole:?}"
        );
    }

    /// An unknown core count is "cannot say", not a division.
    ///
    /// `Aggregate::cores` is 0 when `available_parallelism` failed, and
    /// `largest_share` already treats a 0 that way for exactly this
    /// reason: `load / 0` is `+inf` in IEEE 754, which clears every
    /// threshold at once. A single unreadable core count would otherwise
    /// turn this rule into "always fire".
    #[test]
    fn an_unknown_core_count_does_not_divide() {
        assert!(
            oversubscribed(&load_series(&[53.0; 30]), 0).is_none(),
            "zero cores is unknown, and an unknown cannot be evidence"
        );
    }

    /// The duration clause, asserted rather than assumed.
    ///
    /// A load spike at any level is silence until it has held, because the
    /// whole discriminator between a build and a runaway is persistence --
    /// the same weighting `WATCH_MINUTES` uses, for the same reason.
    #[test]
    fn a_load_spike_must_hold_before_it_is_said() {
        // Nine minutes of samples is nine minutes of measured span (ten
        // samples, one minute apart), just under the threshold.
        let brief = load_series(&[60.0; 10]);
        assert!(
            oversubscribed(&brief, 12).is_none(),
            "nine minutes of measured span is under OVERSUBSCRIBED_MINUTES"
        );
        // One more sample crosses it.
        let held = load_series(&[60.0; 12]);
        assert!(
            oversubscribed(&held, 12).is_some(),
            "eleven minutes is over it"
        );
    }

    /// The ratio boundary, and the normalisation that makes it mean
    /// anything.
    ///
    /// The SAME load average is a fault on one machine and ordinary on
    /// another: 18 on 12 cores is 1.5x and qualifies, while 18 on 64 cores
    /// is 0.28x and is a quiet machine. Comparing a raw load average to a
    /// fixed number -- the obvious shortcut -- would call every large
    /// machine broken and every small one healthy.
    #[test]
    fn the_ratio_is_normalised_by_core_count() {
        let eighteen = load_series(&[18.0; 30]);
        assert!(
            oversubscribed(&eighteen, 12).is_some(),
            "18 on 12 cores is 1.5x: oversubscribed"
        );
        assert!(
            oversubscribed(&eighteen, 64).is_none(),
            "the same 18 on 64 cores is 0.28x: a quiet machine"
        );
        // Just under the ratio on the 12-core machine, so the boundary is
        // asserted and not assumed.
        let under = load_series(&[12.0 * OVERSUBSCRIBED_RATIO - 0.1; 30]);
        assert!(
            oversubscribed(&under, 12).is_none(),
            "just under the ratio is silence"
        );
    }

    /// **Gap discipline, the same refusal as every other duration in this
    /// module.**
    ///
    /// The incident ran for 8.5 hours while nobody was watching. If the
    /// run could be stitched across a gap, the first sample after the app
    /// reopens would sit beside the last one before it and a condition
    /// nobody measured would be reported as sustained -- which is the
    /// failure `a_gap_is_not_sustained_burn` exists for, applied to load
    /// average.
    #[test]
    fn a_gap_is_not_sustained_oversubscription() {
        let end = chrono::Utc::now();
        let mut s = Vec::new();
        // Twenty high samples, then a six-hour hole, then three more.
        for i in 0..20 {
            let at = end - chrono::Duration::hours(6) - chrono::Duration::minutes(20 - i);
            let mut one = bare(&at.to_rfc3339());
            one.load = Some([0.5, 1.0, 53.0]);
            s.push(one);
        }
        for i in 0..3 {
            let at = end - chrono::Duration::minutes(2 - i);
            let mut one = bare(&at.to_rfc3339());
            one.load = Some([0.5, 1.0, 53.0]);
            s.push(one);
        }
        let out = oversubscribed(&s, 12);
        assert!(
            out.is_none(),
            "only two minutes of measured span sits after the gap: {out:?}"
        );
        assert_eq!(
            GAP_MS,
            crate::health::alerts::GAP_MS,
            "and it is the same gap rule as the rest of health, not a second one"
        );
    }

    /// A machine-wide notice has no process, and says so rather than
    /// inventing one.
    ///
    /// `name()` is `None`, `niced()` and `orphaned()` are `false`: it is a
    /// condition of the machine, so there is no priority and no parent to
    /// report. The key carries no figures either, so a load wandering
    /// between 19 and 21 stays ONE row rather than becoming a new one
    /// every poll -- the rule `Alert::key` states.
    #[test]
    fn a_machine_wide_notice_names_no_process() {
        let n = oversubscribed(&load_series(&[53.0; 30]), 12).expect("fires");
        assert_eq!(n.name(), None, "it is about no process in particular");
        assert!(!n.niced());
        assert!(!n.orphaned());
        assert_eq!(n.key(), "cpu_oversubscribed", "no figures in the key");
        assert!(!n.title().is_empty());
        assert!(n.minutes() >= OVERSUBSCRIBED_MINUTES);
    }

    /// An empty series is no information, not a quiet machine -- and one
    /// sample is a reading, not a duration.
    #[test]
    fn one_load_reading_is_not_a_duration() {
        assert!(oversubscribed(&[], 12).is_none(), "nothing measured");
        assert!(
            oversubscribed(&load_series(&[53.0]), 12).is_none(),
            "one sample spans no measured time at all"
        );
    }

    /// A NaN from the platform is not a reading.
    ///
    /// Left in, a NaN loses every comparison silently: `nan / 12.0 < 1.5`
    /// is `false`, so it would NOT break the run -- it would be summed
    /// into a mean that is NaN from then on, and the notice would report
    /// "load average NaN". `Table::read` refuses a NaN for the same reason
    /// one line over.
    #[test]
    fn a_nan_load_reading_is_not_a_reading() {
        let mut s = load_series(&[53.0; 30]);
        s[29].load = Some([0.5, 1.0, f64::NAN]);
        assert!(
            oversubscribed(&s, 12).is_none(),
            "the newest reading is unusable, so there is no run at all"
        );
    }

    // ---- Shadow logging ---------------------------------------------

    /// Nice 0, the common real value, so every pre-#865 test keeps
    /// asserting what it asserted. `proc_niced` is for the cases that
    /// are ABOUT the nice value.
    fn proc(pid: u32, name: &str, cpu: f64, parent: Option<u32>) -> ProcessObservation {
        ProcessObservation {
            pid,
            start_time: 1_700_000_000,
            name: name.to_string(),
            cpu_percent: cpu,
            parent,
            nice: Some(0),
        }
    }

    fn proc_niced(
        pid: u32,
        name: &str,
        cpu: f64,
        parent: Option<u32>,
        nice: Option<i32>,
    ) -> ProcessObservation {
        ProcessObservation {
            nice,
            ..proc(pid, name, cpu, parent)
        }
    }

    fn durations(entries: &[(&ProcessObservation, f64)]) -> HashMap<(u32, u64), f64> {
        entries
            .iter()
            .map(|(p, m)| ((p.pid, p.start_time), *m))
            .collect()
    }

    /// Tier 1's shape: high, orphaned, long, not a known daemon.
    #[test]
    fn an_orphaned_long_burner_is_shadow_logged_as_tier_one() {
        let p = proc(4242, "yes", 99.0, Some(1));
        let out = shadow(std::slice::from_ref(&p), &durations(&[(&p, 45.0)]));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].tier, "tier1");
        assert_eq!(out[0].severity, "alert");
        assert_eq!(out[0].name, "yes");
        assert!(out[0].orphaned);
        assert!(!out[0].allowlisted);
        assert!(!out[0].line().is_empty());
    }

    /// A known daemon is exonerated even when it matches everything
    /// else. `ollama` legitimately pins cores for hours and legitimately
    /// reparents to launchd.
    #[test]
    fn an_allowlisted_daemon_is_not_shadow_logged_as_tier_one() {
        let p = proc(900, "ollama", 140.0, Some(1));
        let out = shadow(std::slice::from_ref(&p), &durations(&[(&p, 90.0)]));
        assert!(out.is_empty(), "{out:?}");
    }

    /// A parented process is not tier 1 however hot it is -- that is
    /// what tier 2's much longer duration is the backstop for.
    #[test]
    fn a_parented_burner_is_not_tier_one() {
        let p = proc(5000, "node", 99.0, Some(412));
        let out = shadow(std::slice::from_ref(&p), &durations(&[(&p, 45.0)]));
        assert!(
            out.is_empty(),
            "forty-five minutes is not two hours: {out:?}"
        );
    }

    /// Tier 2 catches it once it has been two hours.
    #[test]
    fn a_parented_burner_past_two_hours_is_tier_two() {
        let p = proc(5000, "node", 99.0, Some(412));
        let out = shadow(std::slice::from_ref(&p), &durations(&[(&p, 130.0)]));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].tier, "tier2");
        assert_eq!(out[0].severity, "warn");
        assert!(!out[0].orphaned);
    }

    /// A process matching both tiers is reported ONCE, as tier 1.
    /// Two lines would double-count it in the distribution the shadow
    /// week exists to produce.
    #[test]
    fn a_process_matching_both_tiers_is_logged_once() {
        let p = proc(4242, "yes", 99.0, Some(1));
        let out = shadow(std::slice::from_ref(&p), &durations(&[(&p, 200.0)]));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].tier, "tier1");
    }

    /// A process the watcher has never seen has no duration, so it
    /// cannot satisfy a duration rule however hot it is right now.
    #[test]
    fn a_brand_new_process_has_no_duration() {
        let p = proc(4242, "yes", 400.0, Some(1));
        assert!(shadow(&[p], &HashMap::new()).is_empty());
    }

    /// A missing parent is NOT an orphan. "We could not read the
    /// parent" and "this process has no living parent" are opposite
    /// answers, and conflating them would make every unreadable
    /// process a tier-1 candidate.
    #[test]
    fn an_unreadable_parent_is_not_an_orphan() {
        let p = proc(4242, "yes", 99.0, None);
        assert!(!p.orphaned());
        assert!(shadow(std::slice::from_ref(&p), &durations(&[(&p, 45.0)])).is_empty());
    }

    // ---- The watcher -------------------------------------------------

    /// Minutes accumulate across passes while the process stays hot.
    #[test]
    fn the_watcher_accumulates_across_passes() {
        let mut w = Watcher::default();
        let p = proc(4242, "yes", 99.0, Some(1));
        for _ in 0..10 {
            w.observe(std::slice::from_ref(&p), 60_000);
        }
        let minutes = w.observe(std::slice::from_ref(&p), 60_000);
        assert!(
            (minutes[&(p.pid, p.start_time)] - 11.0).abs() < 0.001,
            "{minutes:?}"
        );
    }

    /// **The live half of the gap rule.**
    ///
    /// Across a closed lid the sampler's elapsed span is hours. If that
    /// were credited, every hot process on the machine would be handed
    /// half a day of "sustained" burn the moment the app reopens -- and
    /// tier 1 would shadow-log a fleet of firings that never happened.
    #[test]
    fn the_watcher_refuses_to_credit_a_gap() {
        let mut w = Watcher::default();
        let p = proc(4242, "yes", 99.0, Some(1));
        for _ in 0..5 {
            w.observe(std::slice::from_ref(&p), 60_000);
        }
        // Twelve hours asleep.
        let minutes = w.observe(std::slice::from_ref(&p), 12 * 60 * 60 * 1000);
        assert_eq!(
            minutes.get(&(p.pid, p.start_time)).copied(),
            Some(0.0),
            "a gap resets rather than extends: {minutes:?}"
        );
    }

    /// Dropping below the floor ends the run: the next hot spell starts
    /// from zero rather than resuming. A process busy for a minute an
    /// hour has not been busy for thirty minutes.
    #[test]
    fn the_watcher_resets_a_process_that_goes_quiet() {
        let mut w = Watcher::default();
        let hot = proc(4242, "yes", 99.0, Some(1));
        let cool = proc(4242, "yes", 3.0, Some(1));
        for _ in 0..20 {
            w.observe(std::slice::from_ref(&hot), 60_000);
        }
        w.observe(std::slice::from_ref(&cool), 60_000);
        let minutes = w.observe(std::slice::from_ref(&hot), 60_000);
        assert!(
            (minutes[&(hot.pid, hot.start_time)] - 1.0).abs() < 0.001,
            "the run restarted: {minutes:?}"
        );
    }

    /// A recycled PID does not inherit the dead process's minutes.
    /// That is the entire reason the key carries `start_time`.
    #[test]
    fn a_recycled_pid_starts_from_zero() {
        let mut w = Watcher::default();
        let old = proc(4242, "yes", 99.0, Some(1));
        for _ in 0..40 {
            w.observe(std::slice::from_ref(&old), 60_000);
        }
        let fresh = ProcessObservation {
            start_time: old.start_time + 9_999,
            ..old.clone()
        };
        let minutes = w.observe(std::slice::from_ref(&fresh), 60_000);
        assert!(
            (minutes[&(fresh.pid, fresh.start_time)] - 1.0).abs() < 0.001,
            "{minutes:?}"
        );
        assert!(
            !minutes.contains_key(&(old.pid, old.start_time)),
            "the dead process is forgotten: {minutes:?}"
        );
    }

    /// An exited process is forgotten rather than accumulating forever.
    /// The sampler loop runs for the life of the app, so a map that only
    /// ever grew would be an unbounded leak.
    #[test]
    fn the_watcher_forgets_processes_that_are_gone() {
        let mut w = Watcher::default();
        let p = proc(4242, "yes", 99.0, Some(1));
        w.observe(std::slice::from_ref(&p), 60_000);
        let minutes = w.observe(&[], 60_000);
        assert!(minutes.is_empty(), "{minutes:?}");
    }

    /// The gap rule here and the one in `alerts` are the SAME rule, not
    /// two that happen to agree. A series the charts draw as broken must
    /// not be one a duration is measured across.
    #[test]
    fn the_gap_rule_is_the_battery_modules_gap_rule() {
        assert_eq!(GAP_MS, crate::health::alerts::GAP_MS);
    }

    /// Shadow mode must have no path to a notification. If a `Shadow`
    /// ever becomes an `Alert`, a week of measurement becomes a week of
    /// interruptions -- so the absence of that conversion is asserted
    /// rather than left to review.
    ///
    /// Scanned LINE BY LINE rather than by searching for `"\n}\n"`.
    /// `include_str!` preserves whatever line endings the checkout has,
    /// and a Windows checkout with `core.autocrlf` has CRLF -- so a
    /// byte-pattern containing a bare `\n` finds nothing there and the
    /// test panics on its own `expect`. `str::lines` splits on both, so
    /// this reads the same on every platform. (Observed: this test, on
    /// the `platform (windows-latest)` job.)
    #[test]
    fn nothing_converts_a_shadow_into_an_alert() {
        let src = include_str!("runaway.rs");
        let body: Vec<&str> = src
            .lines()
            .take_while(|l| !l.starts_with("#[cfg(test)]"))
            .collect();
        assert!(!body.is_empty(), "the module body parsed to nothing");
        assert!(
            !body.iter().any(|l| l.contains("impl From<Shadow>")),
            "a Shadow must not be convertible into an Alert"
        );
        // And exactly one Alert variant ships: a second one would mean a
        // deferred tier had gone live.
        let start = body
            .iter()
            .position(|l| l.trim() == "pub enum Alert {")
            .expect("Alert exists");
        let len = body[start..]
            .iter()
            .position(|l| *l == "}")
            .expect("Alert closes");
        let variants = body[start..start + len]
            .iter()
            .filter(|l| l.trim_start().starts_with("DiffuseCpu"))
            .count();
        assert_eq!(variants, 1);
    }

    /// A `Notice` must never become an `Alert`, which is the #865
    /// equivalent of `nothing_converts_a_shadow_into_an_alert` above.
    ///
    /// The watch tier is an INDICATOR: the user asked for something that
    /// says "you might want to investigate", not for another thing that
    /// interrupts them. A `From<Notice> for Alert`, or a `notify_` call
    /// taking one, would silently promote half a core for five minutes
    /// into a notification -- and five minutes is short enough that the
    /// result would be a guard people turn off, which is the outcome
    /// #853 already paid for once.
    ///
    /// Source text rather than types, for the same reason the sibling
    /// test uses it: the thing being forbidden is a conversion that does
    /// not exist yet, and you cannot write a type assertion about an
    /// absent impl.
    #[test]
    fn nothing_converts_a_notice_into_an_alert() {
        let src = include_str!("runaway.rs");
        let body: Vec<&str> = src
            .lines()
            .take_while(|l| !l.starts_with("#[cfg(test)]"))
            .collect();
        assert!(!body.is_empty(), "the module body parsed to nothing");
        assert!(
            !body.iter().any(|l| l.contains("impl From<Notice>")),
            "a Notice must not be convertible into an Alert"
        );
        // And the module must not hand a Notice to anything that
        // notifies. `notify_runaway` takes `&Alert` by signature; this
        // catches a future overload or a generic that would accept both.
        assert!(
            !body
                .iter()
                .any(|l| l.contains("notify") && l.contains("Notice")),
            "a Notice must not reach a notification path"
        );
    }

    /// Wording exists, since a notification with an empty body says
    /// nothing.
    #[test]
    fn the_alert_can_say_what_it_means() {
        let a = Alert::DiffuseCpu {
            percent: 71.0,
            minutes: 19.0,
            process_count: 1436,
        };
        assert!(!a.title().is_empty());
        assert!(a.body().contains("1436"));
        assert!(a.body().contains("19"));
        assert_eq!(a.key(), "diffuse_cpu");
    }

    /// **The regression test for the zero-delta false positive.**
    ///
    /// `sysinfo` refuses to recompute a CPU delta inside
    /// `MINIMUM_CPU_UPDATE_INTERVAL`, so two back-to-back `read` calls
    /// report every process at 0%. A zero `top_cpu_percent` makes
    /// `one_process_explains_it` answer `Some(false)` -- the clause that
    /// lets `evaluate` fire -- so a caller without the interval would
    /// report "no single process explains it" during a build (#807).
    ///
    /// That is `a_legitimate_build_does_not_fire` reintroduced one layer
    /// down, where that test cannot see it, so it is asserted here.
    ///
    /// # What is asserted, and why it changed (#834)
    ///
    /// The promise `read_twice` makes is an INTERVAL: it leaves at least
    /// `MINIMUM_CPU_UPDATE_INTERVAL` between its two reads, which is the
    /// precondition `sysinfo` imposes for a delta to exist at all. That
    /// is what #807 violated, and it is deterministic -- so it is what is
    /// measured here, on elapsed time.
    ///
    /// This test used to compare two live CPU readings instead: a busy
    /// loop, a no-interval `read` pair, and an assertion that
    /// `read_twice`'s `top_cpu_percent` came out strictly higher. That
    /// was a PROXY, and a load-sensitive one, because
    /// `top_cpu_percent` is the whole machine's busiest process and not
    /// this test's own spin loop. Under CI's `--test-threads=8`
    /// (ci.yml, the race check) the comparison is therefore between two
    /// readings of a shared, contended process table, so the assertion
    /// was really a claim about runner load: the spin can be
    /// descheduled, or the eager read can pick up another tenant's
    /// burst, and it inverts. It took down `test-rust` on #831 -- a
    /// stats-only PR with zero files under `health/` -- where it read
    /// exactly like a real regression at a glance.
    ///
    /// Rejected alternatives, both considered:
    ///   * **Retry in-test.** Cheapest and the worst. #811 shipped an
    ///     install retry for a transient that was not the cause, and the
    ///     lesson stuck: a retry that can mask a real regression is
    ///     worse than the flake it hides.
    ///   * **`#[ignore]` behind the env convention** used by the live
    ///     tests in this repo. That would have kept the proxy and lost
    ///     the regression protection on CI -- the #807 bug is precisely
    ///     the kind that must fail a PR.
    ///
    /// The CPU delta is not left untested, it is tested where it is
    /// deterministic: `a_legitimate_build_does_not_fire` and the
    /// `one_process_explains_it` cases above drive synthetic aggregates,
    /// so the zero-delta inversion that made #807 dangerous is asserted
    /// on fixtures rather than on whatever the runner happened to be
    /// doing. `a_real_process_table_reads` keeps the live reader covered.
    #[test]
    fn read_twice_waits_long_enough_for_a_cpu_delta_to_exist() {
        let table = Table::new();
        let started = std::time::Instant::now();
        let (observations, _) = table.read_twice();
        let elapsed = started.elapsed();

        // The property, asserted directly. `>=` because that is the
        // contract -- at LEAST the interval -- and a scheduler may
        // always hand back more; only less would be the #807 bug.
        assert!(
            elapsed >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL,
            "read_twice must leave at least MINIMUM_CPU_UPDATE_INTERVAL \
             ({:?}) between its two reads, or sysinfo reports every \
             process at 0% and one_process_explains_it inverts (#807); \
             took {:?}",
            sysinfo::MINIMUM_CPU_UPDATE_INTERVAL,
            elapsed
        );

        // And it really did read a table across that interval, rather
        // than sleeping and returning nothing -- which would satisfy the
        // timing assertion above while being useless. No claim about the
        // VALUES: that is the machine's business, and asserting on it is
        // what made this test flaky.
        assert!(
            !observations.is_empty(),
            "a machine running this test has processes"
        );
    }

    /// The real process table, on whatever machine runs the tests. Not
    /// an assertion about any particular process -- it cannot be -- but
    /// it does prove the reader works, which a pure-arithmetic suite
    /// never would.
    ///
    /// # Deliberately NOT gated (#853)
    ///
    /// Listed in #853 with the host-dependent group; it does not belong
    /// there, and gating it would leave the live reader with no coverage
    /// on CI at all -- which matters more now that #834 removed the CPU
    /// comparison from the test above.
    ///
    /// Every assertion is an invariant of whatever table came back:
    /// non-empty, `observations.len() == aggregate.process_count`,
    /// `top_cpu_percent.is_finite()`, and our own PID present with a name
    /// and a start time. The last is the only one that names a specific
    /// process, and it is the process doing the asking -- true on every
    /// host, a PID namespace of one included, which is where
    /// `footprint.rs`'s `> 1` count assertion failed. Nothing here reads
    /// a clock or compares two live samples.
    #[test]
    fn a_real_process_table_reads() {
        // `read_twice`, because `sysinfo` reports CPU since the previous
        // refresh and will not recompute one inside
        // `MINIMUM_CPU_UPDATE_INTERVAL` -- so a back-to-back pair would
        // assert against an all-zero reading.
        let (observations, aggregate) = Table::new().read_twice();
        assert!(!observations.is_empty(), "no processes at all");
        assert_eq!(observations.len(), aggregate.process_count);
        assert!(aggregate.top_cpu_percent.is_finite());
        // Our own process must be in there, and it has a parent.
        let own = sysinfo::get_current_pid().expect("our own pid");
        let me = observations
            .iter()
            .find(|p| p.pid == own.as_u32())
            .expect("we are running");
        assert!(!me.name.is_empty());
        assert!(me.start_time > 0, "a start time is the identity half");
    }
}
