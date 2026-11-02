//! WebKitGTK rendering mitigations for hosts whose GPU path cannot work.
//!
//! On Linux, Tauri renders through WebKitGTK. Under WSL2's WSLg there is
//! no DRI3, so WebKit's DMA-BUF renderer and GPU compositing have nothing
//! to talk to. A user report of the 10.12.0 AppImage under WSL2 showed
//! the result: an all-black window, then
//!
//! ```text
//! libEGL warning: DRI3 error: Could not get DRI3 device
//! Couldn't open libGLESv2.so.2: ... No such file or directory
//! Aborted (core dumped)
//! ```
//!
//! The last two lines are libepoxy's: it `dlopen`s `libGLESv2.so.2` when
//! WebKit first asks for a GLES entry point and calls `abort()` if the
//! host does not have it. That library is a host GL-dispatch library
//! (Debian/Ubuntu `libgles2`), loaded at runtime rather than linked, so
//! linuxdeploy never sees it and the AppImage does not -- and should
//! not -- carry it: GL dispatch must match the host's driver stack.
//!
//! So two things together: tell WebKit not to use the GPU paths that fail
//! on WSL (here), and tell the user which host package supplies the GLES
//! library (README troubleshooting).
//!
//! # Scope
//!
//! Applied only when WSL is detected. Disabling compositing everywhere
//! would cost every working Linux desktop its accelerated rendering to
//! fix a host class we can recognise. Other no-DRI3 hosts (some VMs, some
//! NVIDIA setups) can set the same variables themselves; the README says
//! how.
//!
//! # Never override the user
//!
//! A variable that is present in the environment -- even set to an empty
//! string, even set to `0` -- is the user's decision and is left alone.
//! Someone who explicitly re-enabled compositing under WSL gets what they
//! asked for.

use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;

/// The variables this module may set, and the value it sets them to.
pub const MITIGATIONS: [(&str, &str); 2] = [
    ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
];

/// What was observed about the host, separated from how it was read so
/// the decision is testable without a WSL machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WslMarkers {
    /// `/proc/sys/fs/binfmt_misc/WSLInterop` exists.
    pub interop_file: bool,
    /// `WSL_DISTRO_NAME` is present in the environment.
    pub distro_env: bool,
    /// `/proc/version` mentions Microsoft's kernel build.
    pub kernel_is_microsoft: bool,
}

impl WslMarkers {
    pub fn is_wsl(self) -> bool {
        self.interop_file || self.distro_env || self.kernel_is_microsoft
    }
}

/// Reads the WSL markers under `root` (`/` in production) and from `env`.
pub fn detect(root: &Path, env: impl Fn(&str) -> Option<OsString>) -> WslMarkers {
    let proc_dir = root.join("proc");
    let interop = proc_dir
        .join("sys")
        .join("fs")
        .join("binfmt_misc")
        .join("WSLInterop");
    let kernel_is_microsoft = std::fs::read_to_string(proc_dir.join("version"))
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false);
    WslMarkers {
        interop_file: interop.exists(),
        distro_env: env("WSL_DISTRO_NAME").is_some(),
        kernel_is_microsoft,
    }
}

/// Which variables to set: every mitigation the user has not already set,
/// and none at all off WSL. Pure, so it is the thing the tests pin.
pub fn decide(
    markers: WslMarkers,
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, &'static str)> {
    if !markers.is_wsl() {
        return Vec::new();
    }
    MITIGATIONS
        .iter()
        .copied()
        .filter(|(name, _)| env(name).is_none())
        .collect()
}

static APPLIED: OnceLock<Vec<&'static str>> = OnceLock::new();

/// Sets the mitigations for this process. Must run before WebKit or GTK
/// initialise and before any thread is spawned (`set_var` is not safe
/// against concurrent environment reads) -- i.e. first thing in `run()`.
///
/// The logger does not exist yet at that point, so the decision is kept
/// and written once by [`log_applied`] after the log plugin is up.
pub fn apply() {
    let env = |name: &str| std::env::var_os(name);
    let markers = detect(Path::new("/"), env);
    let chosen = decide(markers, env);
    for (name, value) in &chosen {
        std::env::set_var(name, value);
    }
    let _ = APPLIED.set(chosen.iter().map(|(name, _)| *name).collect());
}

/// Logs, once per process, which mitigations [`apply`] set. Silent when
/// none were set or `apply` never ran.
pub fn log_applied() {
    static LOGGED: OnceLock<()> = OnceLock::new();
    let Some(applied) = APPLIED.get() else {
        return;
    };
    if applied.is_empty() || LOGGED.set(()).is_err() {
        return;
    }
    log::info!(
        "WSL detected: set {} to avoid WebKitGTK's GPU paths, which fail without DRI3. \
         Set either variable yourself to override.",
        applied.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    const WSL: WslMarkers = WslMarkers {
        interop_file: true,
        distro_env: false,
        kernel_is_microsoft: false,
    };

    fn names(chosen: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        chosen.iter().map(|(n, _)| *n).collect()
    }

    #[test]
    fn wsl_with_nothing_set_gets_both_mitigations() {
        let chosen = decide(WSL, env_of(&[]));
        assert_eq!(chosen, MITIGATIONS.to_vec());
        assert!(chosen.iter().all(|(_, v)| *v == "1"));
    }

    #[test]
    fn not_wsl_gets_nothing() {
        assert!(decide(WslMarkers::default(), env_of(&[])).is_empty());
    }

    #[test]
    fn each_marker_alone_counts_as_wsl() {
        for markers in [
            WslMarkers {
                interop_file: true,
                ..Default::default()
            },
            WslMarkers {
                distro_env: true,
                ..Default::default()
            },
            WslMarkers {
                kernel_is_microsoft: true,
                ..Default::default()
            },
        ] {
            assert_eq!(decide(markers, env_of(&[])).len(), 2, "{markers:?}");
        }
    }

    #[test]
    fn a_user_value_is_never_overridden() {
        let chosen = decide(WSL, env_of(&[("WEBKIT_DISABLE_COMPOSITING_MODE", "0")]));
        assert_eq!(names(&chosen), vec!["WEBKIT_DISABLE_DMABUF_RENDERER"]);
    }

    #[test]
    fn an_empty_user_value_still_counts_as_set() {
        let chosen = decide(WSL, env_of(&[("WEBKIT_DISABLE_DMABUF_RENDERER", "")]));
        assert_eq!(names(&chosen), vec!["WEBKIT_DISABLE_COMPOSITING_MODE"]);
    }

    #[test]
    fn both_user_set_means_nothing_to_do() {
        let chosen = decide(
            WSL,
            env_of(&[
                ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
                ("WEBKIT_DISABLE_COMPOSITING_MODE", "0"),
            ]),
        );
        assert!(chosen.is_empty());
    }

    fn fake_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("proc")).unwrap();
        root
    }

    #[test]
    fn detect_reads_the_interop_file() {
        let root = fake_root();
        let dir = root
            .path()
            .join("proc")
            .join("sys")
            .join("fs")
            .join("binfmt_misc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("WSLInterop"), "enabled\n").unwrap();
        let markers = detect(root.path(), env_of(&[]));
        assert!(markers.interop_file && markers.is_wsl());
    }

    #[test]
    fn detect_reads_a_microsoft_kernel_in_proc_version() {
        let root = fake_root();
        std::fs::write(
            root.path().join("proc").join("version"),
            "Linux version 5.15.0-microsoft-standard-WSL2 (gcc) #1 SMP\n",
        )
        .unwrap();
        assert!(detect(root.path(), env_of(&[])).kernel_is_microsoft);
    }

    #[test]
    fn detect_reads_the_distro_env_var() {
        let root = fake_root();
        let markers = detect(root.path(), env_of(&[("WSL_DISTRO_NAME", "TestDistro")]));
        assert!(markers.distro_env && markers.is_wsl());
    }

    #[test]
    fn detect_on_an_ordinary_linux_host_is_not_wsl() {
        let root = fake_root();
        std::fs::write(
            root.path().join("proc").join("version"),
            "Linux version 6.8.0-45-generic (gcc) #45 SMP\n",
        )
        .unwrap();
        assert!(!detect(root.path(), env_of(&[])).is_wsl());
    }

    #[test]
    fn detect_with_no_proc_at_all_is_not_wsl() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path(), env_of(&[])), WslMarkers::default());
    }
}
