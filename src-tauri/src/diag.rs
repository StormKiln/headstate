//! The switch behind the verbose `[diag]` timing log.
//!
//! Added in v3.5.3 to diagnose a slow review query on one machine, and
//! kept as a setting rather than removed: the next "it is slow on my
//! machine" report wants exactly this log, and asking someone to
//! install a special build to produce it is far worse than a checkbox.

use std::sync::atomic::{AtomicBool, Ordering};

/// A process-global flag rather than a value threaded through.
///
/// The call sites span the poll loop, the Tauri commands, and
/// `github::client` -- and the client has no `AppHandle` and no
/// business acquiring one just to decide whether to log. A single
/// atomic read is also cheap enough to sit in a per-request path, which
/// a settings lookup would not be.
///
/// This is deliberately NOT the pattern used for the refused-field
/// count, which was a global counter and had to be replaced because it
/// raced across polls and across tests. The difference is that this is
/// a single write on a settings change and a read everywhere else, with
/// no accumulation to lose.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Apply the user's preference. Called at startup and whenever settings
/// are saved.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether `[diag]` lines should be written.
///
/// `Relaxed` is right: nothing else is ordered against this, and a log
/// line landing one request either side of a settings change carries no
/// consequence.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Log a `[diag]` line, but only when diagnostics are on.
///
/// A macro rather than a function so the arguments are not formatted
/// when logging is off -- these lines interpolate elapsed times and
/// counts on every request, and paying that cost for output nobody
/// asked for is the thing the switch exists to avoid.
#[macro_export]
macro_rules! diag {
    ($($arg:tt)*) => {
        if $crate::diag::enabled() {
            log::info!($($arg)*);
        }
    };
}

/// The test logger and the switch lock, shared with every test that
/// needs to see what a diag line SAID (#1488), not only that one was
/// written.
///
/// Lifted out of `tests` rather than copied: `log::set_boxed_logger` is
/// process-global and one-shot, so a second module installing its own
/// capturing logger would lose the race to this one (or win it and break
/// the counting test below). One logger, installed once, serves both.
#[cfg(test)]
pub(crate) mod capture {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Serialises the tests that drive the global diagnostics switch.
    ///
    /// `set_enabled` is process-global and several tests toggle it, so
    /// under `--test-threads=8` one flips the switch while another is
    /// asserting on it. The counter noise was already handled with
    /// deltas; the SWITCH itself was not.
    ///
    /// `std::sync::Mutex` is right here -- these are ordinary
    /// synchronous tests with no await in the guarded window. Recovers
    /// from poisoning so a panic in one test fails that test rather than
    /// cascading.
    pub(crate) fn switch_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the installed logger has seen: a count, and every formatted
    /// line, in arrival order.
    pub(crate) struct Captured {
        pub(crate) records: AtomicUsize,
        pub(crate) lines: Mutex<Vec<String>>,
    }

    impl Captured {
        /// Lines logged since `mark` (a previous `lines.len()`).
        ///
        /// Other tests log concurrently into the same logger, so a caller
        /// reads a window and asserts on what it must NOT contain -- which
        /// holds however much foreign noise the window also carries.
        pub(crate) fn since(&self, mark: usize) -> Vec<String> {
            let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
            lines.get(mark..).unwrap_or(&[]).to_vec()
        }

        pub(crate) fn mark(&self) -> usize {
            self.lines.lock().unwrap_or_else(|e| e.into_inner()).len()
        }
    }

    /// The logger, installed at most once for this whole test binary.
    ///
    /// # Why a shared static rather than a per-test install (#853)
    ///
    /// `log::set_boxed_logger` is process-global and ONE-SHOT. The test
    /// below used to call it and `return` early when it lost the race --
    /// reporting PASS while asserting nothing at all, so the test
    /// guarding the `[diag]` switch could silently check nothing. That is
    /// the failure mode this repo rejects everywhere else
    /// (`check-privacy.sh`: "Abort loudly instead"), and it is worse than
    /// an ordinary skip because the switch it guards is a privacy-adjacent
    /// one: `[diag]` lines are what the user opted out of.
    ///
    /// The race is avoidable rather than merely detectable, which is why
    /// this is a fix and not a loud skip. There is only ever ONE logger
    /// per process, so the test does not need to own the installation --
    /// it needs the installed logger to be a capturing one. Installing it
    /// from a `OnceLock` does that: whichever test arrives first installs
    /// it, every later caller gets the same capture back, and no call
    /// ever fails. `set_max_level` is set here too, since a logger that
    /// is installed but filtered out captures nothing.
    pub(crate) fn logger() -> &'static Captured {
        static CAPTURED: Captured = Captured {
            records: AtomicUsize::new(0),
            lines: Mutex::new(Vec::new()),
        };
        static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

        struct Capturing;
        impl log::Log for Capturing {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                CAPTURED.records.fetch_add(1, Ordering::Relaxed);
                CAPTURED
                    .lines
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(record.args().to_string());
            }
            fn flush(&self) {}
        }

        INSTALLED.get_or_init(|| {
            // An Err here means something OUTSIDE these tests installed a
            // logger first (a harness, a dependency). That cannot be
            // counted against, and silently passing is the bug being
            // fixed -- so it fails loudly instead of returning a capture
            // that will never move.
            log::set_boxed_logger(Box::new(Capturing))
                .expect("a foreign logger is already installed; this test cannot count records");
            log::set_max_level(log::LevelFilter::Info);
        });
        &CAPTURED
    }
}

#[cfg(test)]
mod tests {
    use super::capture::switch_lock;
    use super::*;

    /// Defaults OFF, so a user who never opens Settings never pays for
    /// a diagnosis they did not ask for.
    ///
    /// Asserted on `UiPrefs::default()` rather than on the static: the
    /// static is process-global and other tests in this binary flip it,
    /// so reading it here would be a race, not a guarantee. The
    /// preference is what actually decides the startup value, so it is
    /// the honest thing to pin.
    #[test]
    fn diagnostics_default_to_off() {
        assert!(!crate::poll::UiPrefs::default().diagnostic_logging);
    }

    /// The macro must actually consult the switch.
    ///
    /// Asserted by installing a REAL logger and counting records.
    /// Counting argument evaluation instead does not work: `log::info!`
    /// has its own level check and skips formatting when no logger is
    /// installed, so an ungated macro and a gated one both evaluate
    /// nothing in a bare unit test -- a version of this test that
    /// counted arguments passed even with the gate deleted.
    ///
    /// No longer returns early when it loses the logger race: see
    /// `capture::logger`, which removes the race instead (#853).
    #[test]
    fn the_macro_writes_nothing_while_off() {
        let records = &super::capture::logger().records;

        // DELTAS, not absolutes. The logger is process-global and this
        // whole binary shares it, so other tests logging concurrently
        // move the counter under us -- which is exactly how the first
        // version of this test passed alone and failed in the suite.
        // A delta is still a real assertion: what matters is whether
        // THIS macro call produced a record.
        let _guard = switch_lock();
        set_enabled(false);
        let before = records.load(Ordering::Relaxed);
        crate::diag!("[diag] must not be written");
        // The gate is synchronous, so any record from this call has
        // already landed by the time the next line runs.
        let after_off = records.load(Ordering::Relaxed);

        set_enabled(true);
        crate::diag!("[diag] must be written");
        let after_on = records.load(Ordering::Relaxed);
        set_enabled(false);

        // Other tests may have logged in between, so the delta is a
        // LOWER bound on their noise and an exact bound on ours only
        // when nothing else ran. Assert the direction, which holds
        // either way: the on-call must add at least one more than the
        // off-call did.
        assert!(
            after_on - after_off >= 1,
            "a diag line was not written while diagnostics were on"
        );
        assert_eq!(
            after_off - before,
            0,
            "a diag line was written while diagnostics were off"
        );
    }

    #[test]
    fn the_switch_round_trips() {
        let _guard = switch_lock();
        set_enabled(true);
        assert!(enabled());
        set_enabled(false);
        assert!(!enabled());
    }
}

/// Reading the log back, for the panel that shows it (#1147).
///
/// Until now `reveal_log` was the only log-facing command: it opened a
/// Finder window and nothing read a byte of the file. So a user who hit
/// a failure had to leave the app, find `~/Library/Logs`, and open the
/// file in another program -- and on the phone could not do even that,
/// because there is no Finder to reveal into.
pub mod tail {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::Path;

    use serde::{Deserialize, Serialize};

    /// The most this will return in one call, whatever is asked for.
    ///
    /// A ceiling on the CALLER, not a guess at the right size: the
    /// payload crosses the Tauri bridge and, for a paired phone, an HTTP
    /// connection. The log rotates at 4 MB and this machine's is
    /// routinely near that, so an unbounded read is a multi-megabyte
    /// string built in memory and serialized to JSON on request.
    pub const MAX_BYTES: u32 = 512 * 1024;

    /// The default when the caller does not care.
    pub const DEFAULT_BYTES: u32 = 64 * 1024;

    /// The end of the log, and the truth about what was left out.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LogTail {
        /// The last bytes of the file, redacted, as text.
        pub text: String,
        /// The byte this excerpt starts at. Zero means the whole file.
        ///
        /// Carried so the view can say "showing the last 64 KB of 4.2
        /// MB" rather than presenting an excerpt as the log. A panel
        /// that silently shows the tail is one where a user scrolls to
        /// the top, sees no error, and concludes there was none.
        pub offset: u64,
        /// The file's total size in bytes.
        pub total: u64,
        /// Whether anything was cut from the front.
        ///
        /// Derivable from `offset > 0`, and kept anyway: it is the fact
        /// the UI actually branches on, and re-deriving a claim at each
        /// call site is how two surfaces come to disagree about it.
        pub truncated: bool,
        /// The path, so the panel can say where this came from and the
        /// reveal button still has something to name.
        pub path: String,
    }

    /// Why the log could not be read.
    ///
    /// `NotFound` is deliberately distinct from an IO error: a log that
    /// has never been written is the normal state of a fresh install,
    /// and reporting it as a failure would send a user looking for a
    /// problem that is not there. "Absent is not zero" (#846) cuts both
    /// ways -- absent is also not broken.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum TailError {
        /// The file does not exist yet.
        NotFound { path: String },
        /// It exists and could not be read, with the reason.
        Unreadable { path: String, why: String },
    }

    impl std::fmt::Display for TailError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NotFound { path } => write!(
                    f,
                    "No log has been written yet ({path}). It appears once something is logged."
                ),
                Self::Unreadable { path, why } => {
                    write!(f, "Could not read {path}: {why}")
                }
            }
        }
    }

    /// Read the last `max_bytes` of `path`.
    ///
    /// Seeks rather than reading the whole file: the log rotates at 4 MB
    /// and this is reached from a panel a user may leave open.
    ///
    /// # The seek can land mid-character, and mid-line
    ///
    /// UTF-8 is multi-byte, so an offset chosen by arithmetic can split
    /// a character. `from_utf8_lossy` would turn that into U+FFFD --
    /// a replacement character at the start of the panel, which reads as
    /// corruption. Instead the first partial LINE is dropped, which also
    /// removes the partial character inside it and is what a reader
    /// expects: a log excerpt starts at a line.
    pub fn read(path: &Path, max_bytes: u32) -> Result<LogTail, TailError> {
        let shown = path.to_string_lossy().into_owned();
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TailError::NotFound { path: shown })
            }
            Err(e) => {
                return Err(TailError::Unreadable {
                    path: shown,
                    why: e.to_string(),
                })
            }
        };
        let total = meta.len();
        let want = u64::from(max_bytes.clamp(1, MAX_BYTES));
        let offset = total.saturating_sub(want);

        let mut f = File::open(path).map_err(|e| TailError::Unreadable {
            path: shown.clone(),
            why: e.to_string(),
        })?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset))
                .map_err(|e| TailError::Unreadable {
                    path: shown.clone(),
                    why: e.to_string(),
                })?;
        }
        let mut buf = Vec::with_capacity(want.min(total) as usize);
        f.take(want)
            .read_to_end(&mut buf)
            .map_err(|e| TailError::Unreadable {
                path: shown.clone(),
                why: e.to_string(),
            })?;

        // Drop the partial first line -- but only when there IS a
        // preceding part. At offset 0 the first line is the real first
        // line of the file, and dropping it would hide the log's
        // beginning on every small log.
        let body: &[u8] = if offset > 0 {
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => &buf[i + 1..],
                // No newline in the whole excerpt: one enormous line.
                // Keeping it lossily is better than returning nothing,
                // and the truncation is already declared.
                None => &buf[..],
            }
        } else {
            &buf[..]
        };

        Ok(LogTail {
            // Redacted on THIS side of the bridge (#1122). The panel is
            // reachable from a paired phone, so an unredacted tail would
            // put tokens and home paths on a network transport -- and
            // the copy button exists precisely so this text gets pasted
            // into a bug report.
            text: crate::redact::redact(&String::from_utf8_lossy(body)),
            offset,
            total,
            truncated: offset > 0,
            path: shown,
        })
    }
}

#[cfg(test)]
mod tail_tests {
    use super::tail::{self, TailError};

    /// A temp file holding `body`, removed when the guard drops.
    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(name: &str, body: &[u8]) -> Self {
            let p = std::env::temp_dir().join(format!("headstate-tail-{name}"));
            std::fs::write(&p, body).unwrap();
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_short_log_comes_back_whole_and_says_it_was_not_truncated() {
        let t = Tmp::new("short", b"line one\nline two\n");
        let got = tail::read(&t.0, 64 * 1024).unwrap();
        assert_eq!(got.text, "line one\nline two\n");
        assert_eq!(got.offset, 0);
        assert_eq!(got.total, 18);
        assert!(!got.truncated);
    }

    #[test]
    fn the_first_line_of_a_short_log_is_never_dropped() {
        // The partial-line trim must NOT run at offset 0: there is no
        // preceding part, so the first line is the file's real first
        // line and dropping it would hide the start of every small log.
        let t = Tmp::new("firstline", b"THE FIRST LINE\nsecond\n");
        let got = tail::read(&t.0, 64 * 1024).unwrap();
        assert!(got.text.starts_with("THE FIRST LINE"), "{:?}", got.text);
    }

    #[test]
    fn a_long_log_is_cut_at_a_line_boundary_and_declares_it() {
        let body: Vec<u8> = (0..2000)
            .flat_map(|i| format!("line {i}\n").into_bytes())
            .collect();
        let total = body.len() as u64;
        let t = Tmp::new("long", &body);
        let got = tail::read(&t.0, 100).unwrap();

        assert!(got.truncated);
        assert_eq!(got.total, total);
        assert!(got.offset > 0);
        // Starts at a line, never mid-line: an excerpt beginning
        // "ne 1993" reads as corruption.
        assert!(got.text.starts_with("line "), "{:?}", got.text);
        // And it really is the END of the file, not the start.
        assert!(got.text.ends_with("line 1999\n"), "{:?}", got.text);
    }

    #[test]
    fn a_split_multibyte_character_never_becomes_a_replacement_char() {
        // THE reason the partial line is dropped rather than decoded
        // lossily. An offset chosen by arithmetic splits a 4-byte emoji,
        // and `from_utf8_lossy` would put U+FFFD at the top of the panel.
        let mut body = Vec::new();
        for i in 0..200 {
            body.extend_from_slice(format!("padding {i} 🎉🎉🎉🎉🎉\n").as_bytes());
        }
        let t = Tmp::new("utf8", &body);
        for want in [50u32, 61, 73, 99, 128, 257] {
            let got = tail::read(&t.0, want).unwrap();
            assert!(
                !got.text.contains('\u{FFFD}'),
                "want={want} produced a replacement char: {:?}",
                got.text
            );
        }
    }

    #[test]
    fn one_enormous_line_is_returned_rather_than_nothing() {
        // No newline anywhere in the excerpt. Returning an empty panel
        // would look like an empty log, which is a different claim.
        let body = vec![b'x'; 10_000];
        let t = Tmp::new("oneline", &body);
        let got = tail::read(&t.0, 100).unwrap();
        assert!(!got.text.is_empty());
        assert!(got.truncated);
    }

    #[test]
    fn an_absent_log_is_not_found_rather_than_an_error() {
        // A log that was never written is the normal state of a fresh
        // install. Reporting it as a failure sends a user looking for a
        // problem that is not there.
        let p = std::env::temp_dir().join("headstate-tail-definitely-absent");
        let _ = std::fs::remove_file(&p);
        match tail::read(&p, 1024).unwrap_err() {
            TailError::NotFound { path } => assert!(path.contains("definitely-absent")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn the_two_failures_read_differently() {
        // Each names a different remedy: "nothing has been logged yet"
        // is not "something is wrong with your disk".
        let a = TailError::NotFound { path: "p".into() }.to_string();
        let b = TailError::Unreadable {
            path: "p".into(),
            why: "denied".into(),
        }
        .to_string();
        assert_ne!(a, b);
        assert!(a.contains("yet"), "{a}");
        assert!(b.contains("denied"), "{b}");
    }

    #[test]
    fn an_empty_log_is_empty_rather_than_missing() {
        // The file EXISTS. Reporting NotFound would claim logging has
        // never happened, which is a different fact.
        let t = Tmp::new("empty", b"");
        let got = tail::read(&t.0, 1024).unwrap();
        assert_eq!(got.text, "");
        assert_eq!(got.total, 0);
        assert!(!got.truncated);
    }

    #[test]
    fn the_request_is_capped_however_much_is_asked_for() {
        // The payload crosses the Tauri bridge and, for a phone, an
        // HTTP connection. A caller asking for everything must not get
        // a multi-megabyte string.
        let body = vec![b'x'; (tail::MAX_BYTES as usize) * 2];
        let t = Tmp::new("cap", &body);
        let got = tail::read(&t.0, u32::MAX).unwrap();
        assert!(
            got.text.len() <= tail::MAX_BYTES as usize,
            "returned {} bytes",
            got.text.len()
        );
        assert!(got.truncated);
    }

    #[test]
    fn a_token_in_the_log_never_reaches_the_caller() {
        // The panel is reachable from a paired phone and its copy button
        // exists so this text gets pasted into a bug report (#1122).
        let t = Tmp::new(
            "secret",
            b"line one\nauth failed for ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
        );
        let got = tail::read(&t.0, 64 * 1024).unwrap();
        assert!(!got.text.contains("ghp_AAAA"), "{:?}", got.text);
    }
}
