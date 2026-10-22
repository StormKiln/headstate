//! Whether `chmod 000` actually denies this process, for the tests that
//! depend on it (#1999).
//!
//! About fifty tests make a file or directory unreadable with mode `000`
//! and assert what the code does with the refusal: `Unknown` rather than
//! a zero, a gap rather than an omission. That only works when the mode
//! is enforced against the test process. It is not for root, nor for a
//! process holding `CAP_DAC_OVERRIDE` or `CAP_DAC_READ_SEARCH`, nor on a
//! filesystem that ignores modes -- and cloud sessions and many CI images
//! run as root. There every one of those tests failed, every run, and a
//! real failure had to be found by diffing fifty expected ones by name.
//!
//! [`chmod_denies_reads`] asks the question the tests depend on directly,
//! by trying it, rather than inferring it from the uid: a root process
//! without the capability, or a non-root one with it, gets the right
//! answer. Each such test starts with [`skip_unless_chmod_denies!`],
//! which returns early when denial is not enforced -- and says so on the
//! real stderr, past the test harness's capture, naming the test. A test
//! that passed there would be claiming to have checked a refusal that
//! never happened; a test that failed there would be testing the
//! filesystem. Skipping, visibly, is the only honest outcome.
//!
//! `invariants.rs` requires the call in every test that sets mode `000`,
//! so a new permission test cannot bring the noise back.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;

/// Whether mode `000` stops this process reading a file and listing a
/// directory. Probed once per process; both must be denied, because the
/// tests use both.
///
/// A probe that cannot even set up answers `true`: the tests then run
/// and fail on whatever broke, rather than skipping on a guess.
pub(crate) fn chmod_denies_reads() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(probe)
}

fn probe() -> bool {
    let Ok(tmp) = tempfile::tempdir() else {
        return true;
    };
    let file = tmp.path().join("probe.txt");
    let dir = tmp.path().join("probe-dir");
    if std::fs::write(&file, b"probe").is_err() || std::fs::create_dir(&dir).is_err() {
        return true;
    }
    let shut = std::fs::Permissions::from_mode(0o000);
    if std::fs::set_permissions(&file, shut.clone()).is_err()
        || std::fs::set_permissions(&dir, shut).is_err()
    {
        return true;
    }
    let file_denied = std::fs::read(&file).is_err();
    let dir_denied = std::fs::read_dir(&dir).is_err();
    // Restored so the temp dir's drop can remove them.
    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644));
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
    file_denied && dir_denied
}

/// Say which test skipped and why, on the process's stderr. `part` names
/// what was skipped when the rest of the test still ran.
///
/// Written to the stream directly, not with `eprintln!`: the test harness
/// captures `eprintln!` from a passing test and never shows it, which
/// would make the skip as silent as a pass.
pub(crate) fn report_skip(part: Option<&str>) {
    let name = std::thread::current()
        .name()
        .unwrap_or("<unnamed test>")
        .to_string();
    let what = match part {
        Some(part) => format!("skipped {part} of {name}"),
        None => format!("skipped {name}"),
    };
    let _ = writeln!(
        std::io::stderr().lock(),
        "{what}: chmod 000 does not deny reads to this process \
         (running as root, or with CAP_DAC_OVERRIDE or CAP_DAC_READ_SEARCH), \
         so there is no refusal here for it to test"
    );
}

/// Return early from a test, saying why, when mode `000` does not deny
/// this process. The first statement of every test that sets mode `000`.
macro_rules! skip_unless_chmod_denies {
    () => {
        if !$crate::test_permissions::chmod_denies_reads() {
            $crate::test_permissions::report_skip(None);
            return;
        }
    };
}
pub(crate) use skip_unless_chmod_denies;

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe agrees with what a test would see. Whatever this process
    /// is, a `000` file's readability must match the probe's answer:
    /// readable means not enforced, refused means enforced.
    #[test]
    fn the_probe_matches_a_real_chmod() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refused = std::fs::read(&file).is_err();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(chmod_denies_reads(), refused);
    }
}
