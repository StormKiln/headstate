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

// Discovery's slice deadline is wall-clock. A test that asserts what one slice
// reaches must not depend on runner speed (#1977), so it can pin the budget on
// its own thread: `None` removes the time cap and leaves only the work limit;
// `Some(Duration::ZERO)` makes every slice's deadline already expired.
thread_local! { static SLICE_BUDGET: std::cell::Cell<Option<Option<Duration>>> = const { std::cell::Cell::new(None) }; }
pub(crate) struct SliceBudgetGuard(Option<Option<Duration>>);
impl Drop for SliceBudgetGuard {
    fn drop(&mut self) {
        SLICE_BUDGET.with(|b| b.set(self.0));
    }
}
pub(crate) fn slice_budget(budget: Option<Duration>) -> SliceBudgetGuard {
    SliceBudgetGuard(SLICE_BUDGET.with(|b| b.replace(Some(budget))))
}
pub(crate) fn slice_budget_override() -> Option<Option<Duration>> {
    SLICE_BUDGET.with(std::cell::Cell::get)
}

/// Every open file descriptor of this process that resolves inside `dir`.
/// Other tests' fixtures live in their own temporary directories, so the
/// answer for a test's own fixture does not depend on what runs beside it.
#[cfg(target_os = "linux")]
pub(crate) fn open_handles_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let dir = dir.canonicalize().expect("fixture directory exists");
    std::fs::read_dir("/proc/self/fd")
        .expect("procfs is mounted")
        .filter_map(|fd| std::fs::read_link(fd.ok()?.path()).ok())
        .filter(|target| target.starts_with(&dir))
        .collect()
}
