use std::path::{Path, PathBuf};

/// Locate a package-manager executable.
///
/// A GUI-launched `.app` does NOT inherit the shell's PATH -- `auth.rs`
/// carries the same search for `gh` and `claude`, with the comment "not
/// on PATH is the norm". Confirmed again while building this: `npm` was
/// resolvable in one shell and returned 127 in another.
///
/// The distinction that matters downstream is that `None` here means
/// "the tool is not installed", which must be reported as such. Rendering
/// it as an empty update list would say "you are up to date" about a
/// check that never ran.
pub fn find(program: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    let exe = format!("{program}{}", std::env::consts::EXE_SUFFIX);

    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for dir in fallbacks {
        let candidate = Path::new(dir).join(&exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // LAST: ask the user's own login shell.
    //
    // A fixed fallback list cannot find a version-managed runtime,
    // because the path contains the version. Measured on a real machine:
    // npm and yarn both live at
    // `~/.nvm/versions/node/v24.3.0/bin/` -- and TWO node versions are
    // installed, so guessing which is current would run the wrong
    // toolchain against a lockfile.
    //
    // A login shell runs the user's own profile, so nvm/fnm/asdf/volta
    // all resolve exactly as they do in their terminal. It is the only
    // approach that is right by construction rather than by enumeration.
    //
    // Costs ~1.3s measured, which is why the result is cached for the
    // process lifetime: a version manager's active version does not
    // change while the app is open, and paying it once per program is
    // acceptable where paying it per repository would not be.
    ask_login_shell(program)
}

/// Cached results of asking the login shell, per program name.
///
/// `None` is cached too. A tool that is genuinely absent should not cost
/// a 1.3s shell spawn on every repository the user clicks.
static SHELL_LOOKUPS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, Option<PathBuf>>>,
> = std::sync::OnceLock::new();

/// Where the user's own login shell says a program is.
///
/// `-l` reads the profile (which is where a version manager installs its
/// shims), `-i` makes it interactive (which some setups require to load
/// them at all), and `-c` runs one command.
fn ask_login_shell(program: &str) -> Option<PathBuf> {
    let cache =
        SHELL_LOOKUPS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(program) {
            return hit.clone();
        }
    }

    let found = run_login_shell(program);

    if let Ok(mut map) = cache.lock() {
        map.insert(program.to_string(), found.clone());
    }
    found
}

fn run_login_shell(program: &str) -> Option<PathBuf> {
    // Windows has no login-shell equivalent, and no version manager that
    // hides binaries the way nvm does on unix.
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").ok()?;
    // The program name is passed as an ARGUMENT to `command -v`, not
    // interpolated into the script, so a name containing shell
    // metacharacters cannot become a command. The names are ours
    // (`Ecosystem::program`), but that is a property of the caller
    // rather than of this function.
    // The answer is DELIMITED rather than inferred from line structure
    // (#774). Taking "the last line starting with /" looked safe and was
    // not: an interactive shell with iTerm2 shell integration emits OSC
    // escape sequences (ESC ] 1337 ; ... BEL) with NO trailing newline,
    // so `command -v`'s output is concatenated onto the end of a control
    // sequence. The resulting line begins with an escape byte, and on a
    // real machine `grep -c "^/"` over that output returns ZERO -- every
    // node tool read as missing while resolving perfectly in the same
    // shell.
    //
    // `-i` is what invites it: shell integration only announces itself
    // interactively. Dropping `-i` would fix this case and break the
    // setups the flag was added for, so the parsing is what changes.
    //
    // Markers a profile will not emit by accident, printed with no
    // newline of their own so nothing downstream can split them.
    let script = "printf '<<<headstate:%s>>>' \"$(command -v \"$1\")\"";
    let out = std::process::Command::new(shell)
        .args(["-lic", script, "--", program])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let path = between_markers(&stdout)?;
    // An empty result means `command -v` found nothing: the markers are
    // still printed, which is how "asked and it is absent" is told apart
    // from "the shell never ran".
    if path.is_empty() {
        return None;
    }
    let p = PathBuf::from(path);
    p.is_file().then_some(p)
}

/// The text between the sentinels, or `None` if they are not both there.
///
/// Split out so the parsing is testable without spawning a shell -- the
/// bug in #774 was entirely in this step, and a test that has to launch
/// the user's real profile could not have pinned it.
///
/// Takes the LAST opening marker: a profile that echoes its own commands
/// (`set -x`, or a verbose plugin) can print the script text before
/// running it, so the first occurrence may be the literal `printf` line
/// rather than its output.
fn between_markers(out: &str) -> Option<&str> {
    const OPEN: &str = "<<<headstate:";
    const CLOSE: &str = ">>>";
    let start = out.rfind(OPEN)? + OPEN.len();
    let rest = &out[start..];
    let end = rest.find(CLOSE)?;
    Some(rest[..end].trim())
}

/// Where package managers land when PATH does not carry them.
///
/// Node version managers and per-user Python installs put binaries under
/// the home directory, which is exactly what a GUI app's PATH omits.
pub fn fallback_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    // Through the one resolver (#1554), which a test build answers from a
    // fixture and which reads `USERPROFILE` on Windows, where `HOME` is
    // usually unset. Joined, not formatted, for the verbatim-path reason
    // the root CLAUDE.md gives.
    if let Some(home) = crate::auth::home_dir() {
        for rel in [
            [".local", "bin"],
            [".cargo", "bin"],
            [".dotnet", "tools"],
            [".volta", "bin"],
            [".bun", "bin"],
        ] {
            let dir: PathBuf = rel.iter().fold(home.clone(), |p, part| p.join(part));
            dirs.push(dir.to_string_lossy().into_owned());
        }
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/usr/local/share/dotnet",
        ]
        .iter()
        .map(|d| (*d).to_string()),
    );
    dirs
}

/// A `PATH` for running `bin`, with its own directory first.
///
/// Finding the tool is NOT enough. `npm` and `yarn` are JavaScript files
/// whose shebang is `#!/usr/bin/env node`, so running one starts a
/// SECOND lookup -- for `node` -- inside the child, against whatever
/// `PATH` the child inherited. A GUI-launched `.app` inherits a `PATH`
/// with no `node` on it, which is the same reason `find` needs its
/// fallbacks in the first place.
///
/// The result was `npm: env: node: No such file or directory` for every
/// project, reported as "no update data" once per repository.
///
/// Prepending the resolved binary's own directory fixes it because
/// nvm, Homebrew, Volta and fnm all put `node` and `npm` in the SAME
/// `bin`. Prepended rather than appended: a version manager's node must
/// win over any system one, or the tool runs under an interpreter its
/// installation did not choose.
///
/// Affects `npm` and `yarn` today. `uv` and `dotnet` are real binaries,
/// and `poetry` and `pod` resolve their interpreters absolutely -- but
/// this applies to every spawn regardless, because which tools are
/// scripts is an implementation detail of someone else's installer.
pub fn child_path(bin: &Path) -> std::ffi::OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let Some(dir) = bin.parent() else {
        return existing;
    };
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(std::env::split_paths(&existing));
    // `join_paths` fails only on a directory containing the separator,
    // which cannot happen for a path that just produced a real file.
    std::env::join_paths(dirs).unwrap_or(existing)
}

#[cfg(test)]
mod tests {

    /// #774: the exact shape that broke the old parser.
    ///
    /// Captured from a real machine. iTerm2 shell integration emits OSC
    /// sequences (ESC ] 1337 ; ... BEL) with NO trailing newline, so the
    /// answer is concatenated onto the end of a control sequence and no
    /// line in the output begins with `/`. The old `rfind(starts_with
    /// '/'))` found nothing and every node tool read as missing.
    #[test]
    fn a_path_survives_shell_integration_escape_sequences() {
        let out = "\u{1b}]1337;RemoteHost=@host\u{7}\u{1b}]1337;CurrentDir=/code/proj\u{7}\u{1b}]1337;ShellIntegrationVersion=14;shell=zsh\u{7}<<<headstate:/home/octocat/.nvm/versions/node/v24.3.0/bin/yarn>>>";
        // The premise: this is genuinely the broken shape. If any line
        // started with `/` the old parser would have coped and this test
        // would be pinning nothing.
        assert!(
            !out.lines().any(|l| l.starts_with('/')),
            "fixture must reproduce the no-line-starts-with-slash shape"
        );
        assert_eq!(
            super::between_markers(out),
            Some("/home/octocat/.nvm/versions/node/v24.3.0/bin/yarn")
        );
    }

    /// A profile that prints a banner BEFORE the answer, on its own
    /// lines. The common case the old parser was written for; it must
    /// keep working.
    #[test]
    fn a_banner_before_the_answer_is_ignored() {
        let out = "Welcome to your shell\nLast login: today\n<<<headstate:/usr/local/bin/npm>>>";
        assert_eq!(super::between_markers(out), Some("/usr/local/bin/npm"));
    }

    /// `command -v` found nothing: the markers are still printed with an
    /// empty body. That is "asked, and it is absent" -- distinct from
    /// the shell never having run, which yields no markers at all.
    #[test]
    fn an_empty_answer_is_distinguishable_from_no_answer() {
        assert_eq!(super::between_markers("<<<headstate:>>>"), Some(""));
        assert_eq!(super::between_markers("nothing here"), None);
        // A truncated marker is not an answer.
        assert_eq!(super::between_markers("<<<headstate:/usr/bin/npm"), None);
    }

    /// A profile with `set -x`, or a verbose plugin, echoes the script
    /// before running it -- so the literal `printf` text appears first
    /// and the real output second. The LAST opening marker is the answer.
    #[test]
    fn an_echoed_script_does_not_shadow_the_real_answer() {
        let out = "+ printf '<<<headstate:%s>>>' /wrong/from/trace\n<<<headstate:/usr/bin/yarn>>>";
        assert_eq!(super::between_markers(out), Some("/usr/bin/yarn"));
    }
    use super::*;

    /// The fallback branch is the whole point, so it is tested without
    /// depending on what happens to be installed on the machine.
    #[test]
    fn finds_a_program_in_a_fallback_directory() {
        let t = tempfile::TempDir::new().unwrap();
        let exe = t
            .path()
            .join(format!("faketool{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let dir = t.path().to_string_lossy().to_string();

        let found = find("faketool", &[dir.as_str()]);
        assert_eq!(found, Some(exe));
    }

    /// The per-user directories come from `auth::home_dir`, not from
    /// `$HOME` (#1554): under a fixture home they are the fixture's, and
    /// with no home there are none -- while the fixed system directories
    /// are there either way.
    #[test]
    fn the_home_fallbacks_follow_the_resolver() {
        let fixture = tempfile::TempDir::new().unwrap();
        let cargo_bin = fixture.path().join(".cargo").join("bin");
        let with_home = {
            let _home = crate::auth::test_home::set(fixture.path());
            fallback_dirs()
        };
        assert!(
            with_home.contains(&cargo_bin.to_string_lossy().into_owned()),
            "{with_home:?}"
        );
        let without = fallback_dirs();
        assert!(
            !without.iter().any(|d| d.contains(".cargo")),
            "no home, no per-user directories: {without:?}"
        );
        assert!(without.contains(&"/usr/bin".to_string()));
    }

    /// The login-shell fallback must be LAST.
    ///
    /// It costs ~1.3s measured, so a tool already on PATH or in a
    /// fallback directory must never pay for it. This asserts the
    /// ordering by giving `find` a fallback that WILL match: if the
    /// shell were consulted first the result would be the shell's answer
    /// (or None), not the fallback's.
    ///
    /// # Asserted on the cache, not on the clock (#853)
    ///
    /// The `< 500ms` bound was standing in for "no subprocess was
    /// spawned" -- a proxy, and the reason #853 lists this line. A
    /// 1.3s shell spawn is slow, but 500ms of scheduling delay on a
    /// contended runner is not evidence of one either way.
    ///
    /// `ask_login_shell` memoises into `SHELL_LOOKUPS` before returning,
    /// so "the shell was never consulted for this name" has a direct
    /// observable form: no entry. Asserted on that instead -- stronger
    /// (a fast spawn no longer passes), deterministic, and so needing no
    /// `#[ignore]` gate.
    #[test]
    fn a_fallback_hit_does_not_reach_the_login_shell() {
        let t = tempfile::TempDir::new().unwrap();
        // A name nothing real could resolve, so only the fallback can
        // produce a hit.
        let name = "headstate-fake-tool-xyz";
        let exe = t
            .path()
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let dir = t.path().to_string_lossy().to_string();

        assert_eq!(find(name, &[dir.as_str()]), Some(exe));

        // The shell is asked LAST and caches every answer it gives,
        // including `None`. An entry for this name would mean the
        // fallback hit did not short-circuit the search -- the ~1.3s
        // regression this test exists to catch.
        let consulted = SHELL_LOOKUPS
            .get()
            .map(|c| c.lock().unwrap().contains_key(name))
            .unwrap_or(false);
        assert!(
            !consulted,
            "a fallback hit must not reach the login shell, but {name} \
             was looked up and cached -- that is a ~1.3s spawn per repository"
        );
    }

    /// A missing tool is cached as missing.
    ///
    /// Without this, every repository the user clicks pays a ~1.3s shell
    /// spawn to re-learn that a tool they do not have is still not
    /// installed.
    ///
    /// # Asserted on the cache, not on the clock (#853)
    ///
    /// This compared the two lookups' ELAPSED TIMES, with a 50ms floor,
    /// which made it a claim about scheduling: the second call is fast
    /// because it is cached, but a descheduled 51ms on a contended runner
    /// reads identically to a cache miss. #853 groups it with the
    /// "wall-clock budget as proxy" tests for that reason.
    ///
    /// The cache is observable, so the real property is asserted
    /// directly: after one lookup the program has an entry in
    /// `SHELL_LOOKUPS`, and it is `None`. That is strictly STRONGER than
    /// the timing version -- a second spawn that happened to be fast
    /// passed before and fails now -- and it is deterministic, so it
    /// needs no gate. Gating it would have been the wrong trade: this
    /// guards a 1.3s-per-repository regression, which is exactly the kind
    /// of thing that must fail a PR rather than wait for someone to run
    /// the ignored tests.
    #[test]
    fn a_missing_tool_is_not_looked_up_twice() {
        let name = "headstate-definitely-absent-tool";
        assert!(find(name, &[]).is_none());

        // The negative result is MEMOISED, which is the whole point: an
        // absent tool must not cost a shell spawn per repository.
        let cached = SHELL_LOOKUPS
            .get()
            .expect("the lookup above initialises the cache")
            .lock()
            .unwrap()
            .get(name)
            .cloned();
        assert_eq!(
            cached,
            Some(None),
            "a missing tool must be cached AS missing -- an absent entry \
             means the next repository pays the ~1.3s shell spawn again"
        );

        // And the cached answer is still the right one.
        assert!(find(name, &[]).is_none());
    }

    /// The regression test for `env: node: No such file or directory`.
    ///
    /// Runs a REAL script whose shebang names an interpreter that exists
    /// only beside it, under a PATH that does not contain it. Without
    /// `child_path` the exec fails exactly as npm did; with it the
    /// script runs.
    ///
    /// Asserting on the child's behaviour rather than on the returned
    /// string is the point: `find` returning a path is what made this
    /// bug look fixed while every project still reported no data.
    #[cfg(unix)]
    #[test]
    fn a_script_finds_its_interpreter_beside_itself() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::TempDir::new().unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir(&bin).unwrap();

        // The "interpreter": a shell script that prints a marker. Stands
        // in for `node`, which is what npm's shebang looks for.
        let interp = bin.join("fake-node");
        std::fs::write(&interp, "#!/bin/sh\necho INTERPRETER_RAN\n").unwrap();
        std::fs::set_permissions(&interp, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The "tool": resolved by `find`, but useless unless the child
        // can also resolve `fake-node`. Exactly npm's shape.
        let tool = bin.join("faketool");
        std::fs::write(&tool, "#!/usr/bin/env fake-node\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run = |path: std::ffi::OsString| {
            std::process::Command::new(&tool)
                .env("PATH", path)
                .output()
                .expect("spawn should not fail; the exec inside might")
        };

        // A PATH without the interpreter: the failure being fixed.
        let bare = run(std::ffi::OsString::from("/usr/bin:/bin"));
        let bare_err = String::from_utf8_lossy(&bare.stderr);
        assert!(
            !bare.status.success(),
            "the fixture must fail without the interpreter on PATH, \
             or this test proves nothing"
        );
        assert!(
            bare_err.contains("fake-node"),
            "expected an interpreter-not-found error, got: {bare_err}"
        );

        // And with `child_path`, which puts the tool's own directory
        // first, the interpreter beside it is found.
        let fixed = run(child_path(&tool));
        assert!(
            fixed.status.success(),
            "child_path should make the interpreter resolvable: {}",
            String::from_utf8_lossy(&fixed.stderr)
        );
        assert!(
            String::from_utf8_lossy(&fixed.stdout).contains("INTERPRETER_RAN"),
            "the interpreter should have run"
        );
    }

    /// The tool's directory must come FIRST, so a version manager's
    /// interpreter beats a system one.
    #[test]
    fn the_tools_own_directory_is_first() {
        let p = child_path(Path::new("/opt/versions/node/bin/npm"));
        let first = std::env::split_paths(&p).next().unwrap();
        assert_eq!(first, Path::new("/opt/versions/node/bin"));
    }

    /// The existing PATH is kept, not replaced: a tool may need other
    /// things on it.
    #[test]
    fn the_existing_path_is_preserved() {
        let before: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        let after: Vec<_> =
            std::env::split_paths(&child_path(Path::new("/tmp/x/bin/tool"))).collect();
        for dir in &before {
            assert!(after.contains(dir), "{dir:?} was dropped from PATH");
        }
    }

    /// `None` means "not installed", which the caller MUST report rather
    /// than rendering as an empty list of updates.
    #[test]
    fn a_missing_program_is_none_not_a_guess() {
        assert!(find("headstate-nonexistent-tool", &[]).is_none());
    }

    #[test]
    fn a_directory_with_the_right_name_is_not_a_program() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(t.path().join("faketool")).unwrap();
        let dir = t.path().to_string_lossy().to_string();
        assert!(
            find("faketool", &[dir.as_str()]).is_none(),
            "a directory is not an executable"
        );
    }
}
