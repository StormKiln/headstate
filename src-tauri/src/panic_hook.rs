//! A panic in a background task, made visible.
//!
//! The app's value is a live badge while hidden, so most of its work
//! happens in spawned tasks nobody is watching. A panic in one kills
//! that task and nothing else: the poll loop stops, the tray badge
//! freezes on its last value, and the window still paints normally. The
//! user sees stale-but-plausible numbers with no indication anything
//! died.
//!
//! That is precisely the "it stopped updating" uninvestigability
//! `lib.rs:173` says the logging plugin was added to fix -- except a
//! panic is the one event that produced no log line at all, because
//! `tauri::async_runtime::spawn` swallows it into a `JoinHandle` most
//! call sites drop (`lib.rs:811`, `poll.rs:823`, `:1014`, `:1316`,
//! `:1520`, `:1923`).
//!
//! # Why a frozen badge needs its own state
//!
//! A badge that stopped updating and a badge with nothing new to say
//! render identically. Collapsing those is #1042's defect -- Pending and
//! Unknown are different states -- so this records a marker the UI can
//! branch on rather than only writing a log line nobody opens.
//!
//! # What this does NOT do
//!
//! It reports; it does not recover. No task supervision, no restart, no
//! crash telemetry and nothing leaves the machine. A panic message is
//! redacted before it is written, because it can quote a path or a
//! repository name and the log is built to be sent to someone (#1122).

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a panic has been seen in this process.
///
/// A plain flag rather than a count or a payload: the UI's question is
/// "has background work died", and the detail belongs in the log where
/// a backtrace can sit beside it. `Relaxed` for `poll.rs`'s reason --
/// nothing is ordered against this, and a read landing either side of
/// the write carries no meaning.
static PANICKED: AtomicBool = AtomicBool::new(false);

/// Whether any task has panicked since launch.
///
/// Read by the tray and the status bar so a frozen badge can say so.
pub fn panicked() -> bool {
    PANICKED.load(Ordering::Relaxed)
}

/// Install the hook. Idempotent; safe to call once at startup.
///
/// CHAINS to the previous hook rather than replacing it. Rust's default
/// hook is what prints a panic to stderr, and a developer running
/// `cargo run` still wants that -- this adds a durable record, it does
/// not take one away.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICKED.store(true, Ordering::Relaxed);

        // `payload().as_str()` covers `panic!("literal")` and
        // `panic!("{x}")`; anything else is a type we cannot render, and
        // saying so beats printing `Any { .. }`.
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic payload this build cannot render".to_string());

        let where_ = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "an unknown location".to_string());

        // REDACTED before writing. A panic message can carry a path --
        // `PathBuf` unwraps are the common shape -- and the log's own
        // promise is that it holds no local path (#1122).
        //
        // The location is NOT redacted: it is a source path inside this
        // repository, which is public, and it is the single most useful
        // field for diagnosis.
        log::error!(
            "PANIC in a background task at {where_}: {}",
            crate::redact::redact(&message)
        );
        log::error!(
            "background updates have stopped. The window may still show \
             its last values; they are not being refreshed."
        );

        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    /// The flag starts clear, or every consumer would report a dead app
    /// on a healthy launch.
    ///
    /// Deliberately NOT a test that panics to set it: the flag is
    /// process-global and a test that tripped it would make every other
    /// test in the binary see a panicked app, which is the
    /// `READ_PERMIT_TEST_LOCK` hazard one module over. The hook's
    /// behaviour under an actual panic is covered by the integration
    /// path, not from inside the same process.
    #[test]
    fn nothing_has_panicked_on_a_healthy_process() {
        assert!(
            !super::panicked(),
            "the panic flag must start clear, or a healthy launch reports background work dead"
        );
    }
}
