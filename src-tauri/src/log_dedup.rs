//! Log a condition when it starts, while it persists, and when it clears --
//! not on every tick it holds (#1966).
//!
//! The poll loop, the GitHub client and the snapshot writer all run on a
//! timer, and each logged its warning on EVERY pass while the condition
//! behind it held: offline, a low rate limit, a database that would not
//! open. A day offline was hundreds of identical `poll failed` lines, and
//! the line a bug report needs -- when it STARTED, and whether it ever
//! stopped -- was the one buried under them.
//!
//! So a site now logs:
//! - the first occurrence, in full, exactly as it did before;
//! - a reminder at most every [`REMINDER_EVERY`] while it recurs, with
//!   how many times it happened since it was first logged -- "still
//!   failing" is information, a silent log is not proof that it stopped;
//! - one line when it clears, so the log says it recovered rather than
//!   leaving the reader to infer it from the warnings stopping.
//!
//! Keyed by SITE, not by message. Messages carry counters and timestamps
//! ("12 remaining, resets at ..."), so keying on the text would treat
//! every tick as a new condition and dedupe nothing. The first line and
//! every reminder print the CURRENT message, so a changing cause is still
//! visible -- just not on every tick.
//!
//! A site with no natural "it worked" point simply never logs a recovery;
//! its reminders still bound the volume. Its reminder counts from the
//! FIRST occurrence, gaps included, which is why the wording says "since
//! this was first logged" and not "for the last N minutes".

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How often a condition that is still holding is mentioned again.
///
/// Ten minutes: five ticks at the default two-minute poll, so a reader of
/// the log can still see a condition was continuous without wading through
/// every tick of it.
pub const REMINDER_EVERY: Duration = Duration::from_secs(10 * 60);

/// What, if anything, a site should log for this occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// The condition has just started: log the message as it is.
    Started,
    /// Still holding, and the reminder interval has passed.
    Reminder { occurrences: u64, since: Duration },
    /// It held before and does not now.
    Recovered { occurrences: u64, since: Duration },
}

impl Line {
    /// The tail a reminder appends to the site's own message; empty for
    /// the first line, which is logged exactly as it always was.
    /// (`log_cleared!` words a recovery itself.)
    pub fn suffix(&self) -> String {
        match self {
            Line::Reminder { occurrences, since } => {
                format!(
                    " ({occurrences} times in the {} since this was first logged)",
                    minutes(*since)
                )
            }
            Line::Started | Line::Recovered { .. } => String::new(),
        }
    }
}

fn minutes(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 120 {
        format!("{secs}s")
    } else {
        format!("{} min", secs / 60)
    }
}

#[derive(Debug)]
struct Entry {
    started: Instant,
    last_logged: Instant,
    occurrences: u64,
}

/// The per-site state. A value, so tests drive it with their own clock;
/// the call sites share one through [`holds`] and [`clears`].
#[derive(Debug)]
pub struct Gate {
    entries: HashMap<Cow<'static, str>, Entry>,
    every: Duration,
}

impl Gate {
    pub fn new(every: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            every,
        }
    }

    /// The condition held at `now`. `Some` when this occurrence should be
    /// logged.
    pub fn holds(&mut self, key: impl Into<Cow<'static, str>>, now: Instant) -> Option<Line> {
        match self.entries.entry(key.into()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(Entry {
                    started: now,
                    last_logged: now,
                    occurrences: 1,
                });
                Some(Line::Started)
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let entry = slot.get_mut();
                entry.occurrences += 1;
                if now.saturating_duration_since(entry.last_logged) >= self.every {
                    entry.last_logged = now;
                    Some(Line::Reminder {
                        occurrences: entry.occurrences,
                        since: now.saturating_duration_since(entry.started),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// The condition did NOT hold at `now`. `Some` exactly once, on the
    /// first clear after it held; `None` while it was already clear.
    pub fn clears(&mut self, key: &str, now: Instant) -> Option<Line> {
        self.entries.remove(key).map(|entry| Line::Recovered {
            occurrences: entry.occurrences,
            since: now.saturating_duration_since(entry.started),
        })
    }
}

static GATE: LazyLock<Mutex<Gate>> = LazyLock::new(|| Mutex::new(Gate::new(REMINDER_EVERY)));

/// [`Gate::holds`] on the process-wide gate, now.
pub fn holds(key: impl Into<Cow<'static, str>>) -> Option<Line> {
    GATE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .holds(key, Instant::now())
}

/// [`Gate::clears`] on the process-wide gate, now.
pub fn clears(key: &str) -> Option<Line> {
    GATE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clears(key, Instant::now())
}

/// `log::<level>!` for a condition that may hold on every tick: the first
/// occurrence and periodic reminders only.
///
/// `log_while!(warn, "poll.failed", "poll failed: {e}")`
macro_rules! log_while {
    ($level:ident, $key:expr, $($arg:tt)+) => {
        if let Some(line) = $crate::log_dedup::holds($key) {
            log::$level!("{}{}", format_args!($($arg)+), line.suffix());
        }
    };
}
pub(crate) use log_while;

/// The condition a [`log_while!`] site reports is over: one `info` line if
/// it had been holding, nothing otherwise.
///
/// `log_cleared!("poll.failed", "poll succeeded again")`
macro_rules! log_cleared {
    ($key:expr, $what:expr) => {
        if let Some($crate::log_dedup::Line::Recovered { occurrences, since }) =
            $crate::log_dedup::clears($key)
        {
            log::info!(
                "{} (after {occurrences} occurrences over {}s)",
                $what,
                since.as_secs()
            );
        }
    };
}
pub(crate) use log_cleared;

#[cfg(test)]
mod tests {
    use super::*;

    /// The issue's own test: 100 identical failures, one tick a minute,
    /// then one success.
    #[test]
    fn a_hundred_failures_log_once_plus_reminders_then_one_recovery() {
        let mut gate = Gate::new(Duration::from_secs(600));
        let t0 = Instant::now();
        let tick = Duration::from_secs(60);
        let logged: Vec<Line> = (0..100u32)
            .filter_map(|i| gate.holds("poll.failed", t0 + tick * i))
            .collect();
        // Minute 0, then every ten minutes: 10, 20, ... 90.
        assert_eq!(logged.len(), 10, "{logged:?}");
        assert_eq!(logged[0], Line::Started);
        assert_eq!(
            logged[1],
            Line::Reminder {
                occurrences: 11,
                since: Duration::from_secs(600)
            }
        );
        assert_eq!(
            logged[9],
            Line::Reminder {
                occurrences: 91,
                since: Duration::from_secs(5400)
            }
        );

        let recovered = gate.clears("poll.failed", t0 + tick * 100);
        assert_eq!(
            recovered,
            Some(Line::Recovered {
                occurrences: 100,
                since: Duration::from_secs(6000)
            })
        );
        // Recovery is said once; a healthy tick after it says nothing.
        assert_eq!(gate.clears("poll.failed", t0 + tick * 101), None);
    }

    #[test]
    fn a_condition_that_returns_after_clearing_is_new_again() {
        let mut gate = Gate::new(Duration::from_secs(600));
        let t0 = Instant::now();
        assert_eq!(gate.holds("db.open", t0), Some(Line::Started));
        assert!(gate
            .clears("db.open", t0 + Duration::from_secs(1))
            .is_some());
        assert_eq!(
            gate.holds("db.open", t0 + Duration::from_secs(2)),
            Some(Line::Started)
        );
    }

    #[test]
    fn sites_are_independent() {
        let mut gate = Gate::new(Duration::from_secs(600));
        let t0 = Instant::now();
        assert_eq!(gate.holds("a", t0), Some(Line::Started));
        assert_eq!(gate.holds("b", t0), Some(Line::Started));
        assert_eq!(gate.holds("a", t0), None);
        assert!(gate.clears("a", t0).is_some());
        // Clearing `a` did not clear `b`.
        assert_eq!(gate.holds("b", t0), None);
    }

    #[test]
    fn clearing_a_condition_that_never_held_says_nothing() {
        let mut gate = Gate::new(Duration::from_secs(600));
        assert_eq!(gate.clears("never", Instant::now()), None);
    }

    #[test]
    fn a_reminder_says_how_often_and_how_long() {
        let line = Line::Reminder {
            occurrences: 31,
            since: Duration::from_secs(3600),
        };
        assert_eq!(
            line.suffix(),
            " (31 times in the 60 min since this was first logged)"
        );
        assert_eq!(Line::Started.suffix(), "");
    }
}
