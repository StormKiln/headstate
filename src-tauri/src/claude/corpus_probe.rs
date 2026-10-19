//! Synthetic, thread-local enumeration injection. Never selects a corpus root.
use super::transcript::PopulationControl;
use std::{cell::RefCell, time::Duration};
#[derive(Default)]
struct State {
    entries: usize,
    delay: Duration,
    cancel: Option<(usize, PopulationControl)>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
pub(crate) fn configure(delay: Duration, cancel: Option<(usize, PopulationControl)>) {
    STATE.with(|s| {
        *s.borrow_mut() = State {
            entries: 0,
            delay,
            cancel,
        }
    });
}
pub(crate) fn entries() -> usize {
    STATE.with(|s| s.borrow().entries)
}
pub(crate) fn entry() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.entries += 1;
        if !s.delay.is_zero() {
            std::thread::sleep(s.delay);
        }
        if let Some((at, control)) = &s.cancel {
            if s.entries >= *at {
                control.cancel();
            }
        }
    });
}
#[test]
#[ignore = "synthetic discovery baseline; retains generated fixture"]
fn discovery_scaling_measurement() {
    for (dirs, files, delay) in [(1, 2048, 0), (32, 64, 0), (1, 2048, 100)] {
        let temp = tempfile::tempdir().unwrap().keep();
        for d in 0..dirs {
            let dir = temp.join(format!("project-{d}"));
            std::fs::create_dir(&dir).unwrap();
            for f in 0..files {
                std::fs::write(dir.join(format!("session-{f}.jsonl")), b"").unwrap();
            }
        }
        configure(Duration::from_micros(delay), None);
        #[cfg(feature = "enterprise-harness")]
        crate::enterprise_harness::resources::take_read_capture();
        let start = std::time::Instant::now();
        let scan = super::transcript::corpus(&temp);
        let owned: usize = scan
            .source_fingerprints
            .iter()
            .map(|(p, _, s)| p.as_os_str().len() + s.len())
            .sum();
        eprintln!("DISCOVERY_BASELINE dirs={dirs} files_per_dir={files} delay_us={delay} entries={} elapsed_us={} owned_path_certificate_bytes={owned} sessions={} fixture={}",entries(), start.elapsed().as_micros(),scan.sessions.len(),temp.display());
        #[cfg(feature = "enterprise-harness")]
        assert_eq!(
            crate::enterprise_harness::resources::take_read_capture(),
            [0; 4]
        );
        assert_eq!(scan.sessions.len(), dirs * files);
        assert!(!scan.is_partial());
        configure(Duration::ZERO, None);
    }
}

// Some Unix filesystems (including APFS) reject invalid UTF-8 names. Tests can
// substitute one synthetic directory entry to exercise the same native-byte path.
thread_local! { static ENTRY_PATH: RefCell<Option<(std::path::PathBuf, std::path::PathBuf)>> = const { RefCell::new(None) }; }
#[cfg(unix)]
pub(crate) struct EntryPathGuard;
#[cfg(unix)]
impl Drop for EntryPathGuard {
    fn drop(&mut self) {
        ENTRY_PATH.with(|p| *p.borrow_mut() = None);
    }
}
#[cfg(unix)]
pub(crate) fn substitute_entry_path(
    from: std::path::PathBuf,
    to: std::path::PathBuf,
) -> EntryPathGuard {
    ENTRY_PATH.with(|p| *p.borrow_mut() = Some((from, to)));
    EntryPathGuard
}
pub(crate) fn entry_path(path: std::path::PathBuf) -> std::path::PathBuf {
    ENTRY_PATH.with(|p| match p.borrow().as_ref() {
        Some((from, to)) if from == &path => to.clone(),
        _ => path,
    })
}
