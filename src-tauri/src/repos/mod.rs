//! Browsing a repository's files, the way GitHub's code view does
//! (#1031, #1032, #1033, epic #1011).
//!
//! Two questions, each answered by one bounded function: what is in this
//! directory, and what is in this file. Both take a repository root and a
//! repository-relative path, and both refuse a path that is not inside
//! that root.
//!
//! # Why the listing comes from the git index and not from `readdir`
//!
//! Measured in this repository's own checkout: **672 tracked files
//! against 623,488 on disk in the developer's main checkout -- 928x**.
//! Across that machine's whole scan root it is 30,349 tracked files
//! against roughly a million. Every one of the fifteen largest
//! directories under it is a build artifact directory git already
//! ignores; the biggest is `src-tauri/target/debug/deps` at 33,574
//! entries.
//!
//! The cost differs by the same order:
//!
//! ```text
//! git ls-files, whole repo                 0.01s
//! git ls-files -- <subdir>                 0.014s
//! scandir + stat over target/debug/deps    0.718s
//! ```
//!
//! So a filesystem walk would show the user 623,478 entries in Headstate's
//! own repository, of which 672 are the repository. That is not a code
//! view, it is a build directory -- and re-implementing `.gitignore` to
//! filter it back down is re-deriving what the index already answers, at
//! fifty times the cost, on a rule that would get `node_modules` right and
//! `target/debug/deps` wrong on some repository eventually. Git is
//! already the authority for "what is in this repository" and it answers
//! in 10ms.
//!
//! One directory LEVEL per call, not the whole tree. 30,349 files is
//! small in total, but the code view descends one level at a time and the
//! response should be the level being shown -- and `git ls-files --
//! <subdir>` is measured at the same 14ms as the whole-repo listing, so
//! there is nothing to win by fetching more.
//!
//! # Absent is not zero
//!
//! `caches/mod.rs:550` states the rule and #846 is where it was shipped
//! wrong. There are three outcomes here, not two:
//!
//! - git succeeded and the directory holds no tracked files -> an empty
//!   [`Tree`]. A real answer, and at a path the tree itself produced it is
//!   the "it vanished" signal.
//! - git failed -- not a repository, a permission wall, git missing from a
//!   GUI-launched app's PATH -> an `Err` carrying git's own message. It
//!   must never become an empty listing.
//! - the path was refused by the guard -> an `Err` naming which test
//!   failed, because the four remedies differ.
//!
//! `PartialScanNotice`'s comment names exactly those causes and says why a
//! message beats a boolean: they send the user to three different places,
//! and only the message distinguishes them.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// The most of one file that is ever read or returned (#1033).
///
/// 256 KB, the same window `claude::preview::TAIL_BYTES` chose for a
/// transcript, and the measurement supports it independently. Every
/// tracked file in the 38 repositories under the development machine's
/// scan root -- 30,349 files, 919.8 MB:
///
/// ```text
/// p50      3,553 bytes    (3.5 KB)
/// p90     20,779 bytes
/// p99    203,840 bytes    (199 KB)
/// p99.9    2.1 MB
/// max    275.2 MB         (a checked-in .zip)
///
/// over 256 KB   251 files  (0.8%)
/// over   1 MB    88 files  (0.3%)
/// over  10 MB     5 files
/// over 100 MB     1 file
/// ```
///
/// So the bound clears p99 and 99.2% of tracked files arrive whole. It is
/// never the common case, and when it does bind [`FileRead::truncated`]
/// says so -- a window shown as if it were the whole file is the #846
/// defect in its purest form.
///
/// The bound lives HERE, inside the command, and that is the property
/// that makes the `Class::Read` row safe: a phone's `remote_call` inherits
/// it rather than reimplementing it, the same rule `stats_board` is
/// classed by. A phone that asked for the 275 MB zip must not be handed
/// 275 MB, and never is, because this function never reads it.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// How much of a file's head decides whether it is text (#1033).
///
/// A NUL byte in the first 8 KB, which is what git itself uses and what
/// the corpus measurement was taken with: 29,542 text files against 794
/// binary ones, the binaries 2.6% of files and 59% of bytes.
///
/// Deliberately NOT the extension. The corpus has extensionless tracked
/// binaries (`adserver`, `migrate`, `registration-node`) and 9.3 MB `.py`
/// files that are text, so an extension rule is wrong in both directions.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The entry's own name, with no path in it -- `mod.rs`, `src`.
    pub name: String,
    /// The repository-relative path to it, which is what the caller sends
    /// back to descend or to read. Built with [`PathBuf`] joins rather
    /// than `format!("{}/{}")`: six Windows-only failures have cost this
    /// repository, and a hand-built separator is how each of them started.
    pub path: String,
    /// Whether this is a directory to descend into.
    pub dir: bool,
    /// Whether git has this tracked as a symlink (mode `120000`).
    ///
    /// Shown rather than descended into. Measured across the 38
    /// repositories: 22 tracked symlinks, 2 of them already broken, and 0
    /// resolving outside their own repository root -- so following them
    /// buys 20 working links and costs the guard its containment
    /// property. The GitHub code view shows them as links too.
    pub symlink: bool,
}

/// One directory level of a repository, from the git index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tree {
    /// The repository-relative path listed, `""` for the root. Echoed
    /// back so a response cannot be rendered against the wrong request.
    pub path: String,
    /// Directories first, then files, each group by name -- which is what
    /// the GitHub code view does.
    pub entries: Vec<Entry>,
}

/// One file's bounded contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileRead {
    /// The repository-relative path read.
    pub path: String,
    /// The file's real size on disk, read by `stat` BEFORE the file is.
    /// Asking for 275 MB and then discarding it is not a bound.
    pub size: u64,
    /// The text, or empty when [`FileRead::binary`] is true.
    pub content: String,
    /// Whether the bound cut the file short. STATED, never silent.
    pub truncated: bool,
    /// Whether a NUL byte was found in the head, so this is not text.
    ///
    /// Not an error -- the file was read perfectly well and simply is not
    /// text, which is exactly what the GitHub code view says about one.
    /// The three outcomes a caller must render differently are an `Err`
    /// (could not be READ), `binary: true` with no content, and
    /// `binary: false` with no content -- a genuinely empty file, of which
    /// this corpus has real ones (`.gitkeep`).
    pub binary: bool,
}

/// Resolve a repository-relative path inside `repo_root`, or say which
/// test it failed (#1032).
///
/// The security surface of the whole browser. `transcript_path_in`
/// (`commands.rs`) established the shape and states the reason: a
/// `Class::Read` command's path argument arrives over the pairing
/// transport from a paired device, and *"a paired device is trusted to
/// read Headstate's data; it is not a reason to turn a path parameter into
/// 'read any file on this machine and send it back'"*.
///
/// This must be STRONGER than that one, not weaker, because it has none
/// of that guard's three luxuries: the root is caller-chosen rather than
/// fixed at `~/.claude/projects`, there is no extension to check because
/// every extension is legitimate, and it must accept directories as well
/// as files. What replaces the extension check is the caller's
/// obligation to re-derive the root against the live scan first -- see
/// [`repo_path_in`]'s callers in `commands.rs`.
///
/// The four checks, in this order and for these reasons:
///
/// 1. **A relative path with no `..` component and no root.** Rejected
///    before anything touches the disk, so `../../.ssh/id_rsa` never
///    reaches a syscall at all. This is belt to the braces below, not a
///    substitute for them.
/// 2. **`symlink_metadata` on the target, BEFORE canonicalising.**
///    `caches/mod.rs:505` states why: canonicalising "resolves through
///    links and leaves nothing to detect". A symlink is refused outright
///    rather than followed -- see [`Entry::symlink`] for the measurement
///    that decided it.
/// 3. **Canonicalise BOTH the root and the target.** The root too, with a
///    measured reason `transcript_path_in` records: on macOS `/Users/...`
///    resolves through `/System/Volumes/Data`, so comparing a resolved
///    path against an unresolved root fails on every real machine.
/// 4. **`starts_with` on the RESOLVED paths, never a string prefix.**
///    `~/.claude/projects/../../.ssh/id_rsa` has the string prefix and is
///    not under the root.
///
/// # Errors
///
/// Names which test failed, because the remedies differ four ways: a path
/// outside the root is a caller asking for something this does not serve;
/// a symlink is a deliberate refusal the UI should explain; a missing path
/// is a file deleted since the tree was listed; and a root that is no
/// longer a scanned repository is a stale selection (that fourth one is
/// the caller's check, above this).
pub fn repo_path_in(repo_root: &Path, rel: &str) -> Result<PathBuf, String> {
    // 1. The shape of the argument, before any syscall. An absolute path
    //    or a `..` component is a caller asking for something else
    //    entirely, and saying so here gives a clearer message than a
    //    containment failure three steps later would.
    let candidate = Path::new(rel);
    if candidate.is_absolute() {
        return Err(format!(
            "{rel} is an absolute path; this takes a path relative to the repository"
        ));
    }
    for part in candidate.components() {
        match part {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => {
                return Err(format!(
                    "{rel} leaves the repository; only paths inside it can be browsed"
                ))
            }
        }
    }

    // `join` on an empty string yields the root itself, which is the
    // listing the browser opens on. `PathBuf::join`, never
    // `format!("{}/{rel}")` -- see `Entry::path`.
    let target = repo_root.join(candidate);

    // 2. Never a symlink, checked BEFORE canonicalising, per
    //    `caches/mod.rs`. After it there is nothing left to detect.
    let meta =
        std::fs::symlink_metadata(&target).map_err(|e| format!("{rel}: could not be read: {e}"))?;
    if meta.is_symlink() {
        return Err(format!(
            "{rel} is a symbolic link, which is shown but not followed"
        ));
    }

    // 3. Both sides resolved.
    let root = repo_root
        .canonicalize()
        .map_err(|e| format!("{}: could not be read: {e}", repo_root.display()))?;
    let resolved = target
        .canonicalize()
        .map_err(|e| format!("{rel}: could not be read: {e}"))?;

    // 4. On the resolved paths.
    if !resolved.starts_with(&root) {
        return Err(format!(
            "{rel} is not inside {}, so it cannot be browsed from here",
            root.display()
        ));
    }
    Ok(resolved)
}

/// One directory level of `repo_root`, from the git index (#1031).
///
/// `git ls-files -z --stage -- <prefix>` once, folded into immediate
/// children: an entry with no further `/` after the prefix is a file, and
/// the rest collapse to unique directory names. `--stage` is what carries
/// the mode, which is the only place the symlink bit is available without
/// a second syscall per entry.
///
/// The path is guarded by [`repo_path_in`] before git sees it, so a
/// caller cannot enumerate an arbitrary directory through the prefix.
pub fn tree(repo_root: &Path, rel: &str) -> Result<Tree, String> {
    let rel = normalise(rel);
    // The guard runs on the DIRECTORY being listed, so a prefix pointing
    // outside the repository is refused before git is spawned. The root
    // itself (`""`) resolves to the root, which passes trivially and is
    // the right answer.
    let dir = repo_path_in(repo_root, &rel)?;
    if !dir.is_dir() {
        return Err(format!("{rel} is not a directory"));
    }

    // git is run IN the directory being listed (`git -C <dir>`), so it
    // prints paths relative to that directory and the prefix is implicit
    // -- verified, and it is why one call serves the root and a
    // subdirectory alike.
    //
    // `--stage` for the mode, which is the only place the symlink bit is
    // available without an `lstat` per entry. `-z` so a path with a
    // newline in it cannot split one entry into two. `--` and then `.` so
    // a directory whose name looks like an option is still a path.
    //
    // A git FAILURE is an `Err` carrying git's own message -- never an
    // empty listing, which is #846's exact shape and the one thing this
    // must not do.
    let out = crate::worktrees::git(&dir, &["ls-files", "-z", "--stage", "--", "."])?;

    let mut dirs: std::collections::BTreeMap<String, ()> = std::collections::BTreeMap::new();
    let mut files: Vec<Entry> = Vec::new();
    for record in out.split('\0') {
        if record.is_empty() {
            continue;
        }
        // `<mode> <oid> <stage>\t<path>`. The path is after the first
        // tab, which is the only field separator that cannot appear in a
        // mode.
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mode = meta.split_whitespace().next().unwrap_or("");
        // git prints paths relative to the CWD it was run in, which is
        // the directory being listed -- so the first segment is already
        // the immediate child.
        let mut segments = path.split('/');
        let Some(name) = segments.next() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if segments.next().is_some() {
            dirs.insert(name.to_string(), ());
        } else {
            files.push(Entry {
                name: name.to_string(),
                path: child_path(&rel, name),
                dir: false,
                // Mode `120000` is git's symlink. Read from the index
                // rather than from a `lstat` per entry: 651 entries would
                // be 651 syscalls to learn something git already knows.
                symlink: mode == "120000",
            });
        }
    }

    // Directories first, then files, each already in name order -- the
    // `BTreeMap` sorts the directories and git's own output sorts the
    // files. This is what the GitHub code view does.
    let mut entries: Vec<Entry> = dirs
        .into_keys()
        .map(|name| Entry {
            path: child_path(&rel, &name),
            name,
            dir: true,
            symlink: false,
        })
        .collect();
    entries.extend(files);
    Ok(Tree { path: rel, entries })
}

/// One file's bounded contents (#1033).
///
/// Reads the SIZE first and then at most [`MAX_FILE_BYTES`], from the
/// HEAD rather than the tail: unlike a transcript, you read a source file
/// from the top.
pub fn file(repo_root: &Path, rel: &str) -> Result<FileRead, String> {
    let rel = normalise(rel);
    if rel.is_empty() {
        return Err("no file was named".to_string());
    }
    let path = repo_path_in(repo_root, &rel)?;
    // `stat` BEFORE the read. Asking for 275 MB and then discarding it is
    // not a bound.
    let meta = std::fs::metadata(&path).map_err(|e| format!("{rel}: could not be read: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{rel} is a directory, not a file"));
    }
    let size = meta.len();

    use std::io::Read;
    let mut handle =
        std::fs::File::open(&path).map_err(|e| format!("{rel}: could not be read: {e}"))?;
    let mut buf = Vec::new();
    // `take` so the bound is enforced by the reader itself rather than by
    // a length check after the fact -- a file that GREW between the
    // `stat` and the read still cannot hand back more than the window.
    handle
        .by_ref()
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{rel}: could not be read: {e}"))?;

    // A NUL byte in the head, which is what git uses. Over the window we
    // actually hold, capped at the sniff length: a NUL at byte 300,000 in
    // a file we only ever show the first 256 KB of is not something the
    // rendered text can contain.
    let sniff = &buf[..buf.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Ok(FileRead {
            path: rel,
            size,
            content: String::new(),
            truncated: false,
            binary: true,
        });
    }

    let truncated = size > MAX_FILE_BYTES;
    Ok(FileRead {
        path: rel,
        size,
        // `from_utf8_lossy`: a window cut at a byte boundary can land
        // mid-codepoint, and refusing a whole file because its 256 KB
        // boundary fell inside a `é` would be a worse answer than one
        // replacement character at the very end.
        content: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
        binary: false,
    })
}

/// A repository-relative path in the one spelling everything here uses:
/// no leading or trailing `/`, and `.` for the root spelled as `""`.
///
/// Normalising rather than rejecting, because the browser's own "up"
/// button naturally produces `""` and a caller that sends `"/"` or
/// `"src/"` means the same directory either way. It does NOT normalise
/// away a `..` -- that is [`repo_path_in`]'s refusal, and quietly
/// rewriting it here would hide the very thing the guard exists to name.
fn normalise(rel: &str) -> String {
    rel.trim_matches('/').to_string()
}

/// `<parent>/<name>`, or just `<name>` at the root.
///
/// The wire path is a repository-relative POSIX path -- git's own
/// spelling, which is what the caller sends back -- so this is a string
/// join by design. Every path that touches the FILESYSTEM goes through
/// `PathBuf::join` in [`repo_path_in`] instead.
fn child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic repository, never the developer's real one.
    ///
    /// `transcript_path_in` is split out from `claude_transcript_path` for
    /// precisely this reason -- *"a guard tested against the real home
    /// directory is a guard tested on one machine's accidents"* -- and
    /// `caches/mod.rs` makes the point harder: the containment rule "is
    /// the one rule that must never go untested because CI happens to
    /// lack Python tooling -- which is exactly what happened the first
    /// time this shipped".
    struct Fixture {
        dir: std::path::PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "headstate-repos-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("fixture root");
            Self { dir }
        }

        fn write(&self, rel: &str, body: &[u8]) {
            let p = self.dir.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("fixture parent");
            }
            std::fs::write(p, body).expect("fixture file");
        }

        fn git(&self, args: &[&str]) {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.dir)
                .args(args)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        /// A real git repository with an index, because the listing reads
        /// the INDEX and a fixture with no index would make every
        /// assertion about it vacuous.
        fn init(&self) {
            self.git(&["init", "-q"]);
            self.git(&["config", "user.email", "headstate@users.noreply.github.com"]);
            self.git(&["config", "user.name", "t"]);
        }

        fn add_all(&self) {
            self.git(&["add", "-A"]);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // ---- the containment guard (#1032) ------------------------------

    #[test]
    fn a_path_inside_the_repository_resolves() {
        let f = Fixture::new("inside");
        f.write("src/main.rs", b"fn main() {}");
        let got = repo_path_in(&f.dir, "src/main.rs").expect("inside the root");
        assert!(got.ends_with("src/main.rs"), "{}", got.display());
    }

    /// The traversal refusal, spelled through the RESOLVED root.
    ///
    /// Asserting "the error mentions traversal" would pass on macOS for
    /// the wrong reason: `/var` resolves to `/private/var`, so a guard
    /// comparing an unresolved root would reject this path with a
    /// containment error even while being broken for every path that DOES
    /// resolve. So the positive case above runs in the same fixture, and
    /// the check below is that the escape is refused while a real file
    /// inside the root is still reachable.
    #[test]
    fn a_dot_dot_escape_is_refused() {
        let f = Fixture::new("escape");
        f.write("src/main.rs", b"fn main() {}");
        // A real file OUTSIDE the root, one level up, so the refusal
        // cannot be "the target does not exist".
        let outside = f
            .dir
            .parent()
            .expect("temp dir has a parent")
            .join(format!("headstate-repos-secret-{}.txt", std::process::id()));
        std::fs::write(&outside, b"id_rsa").expect("outside file");
        let rel = format!(
            "../{}",
            outside.file_name().and_then(|n| n.to_str()).expect("name")
        );
        let err = repo_path_in(&f.dir, &rel).expect_err("must refuse a path outside the root");
        assert!(
            err.contains("leaves the repository"),
            "the error must name which test failed, got: {err}"
        );
        // And the guard is not simply refusing everything: the same
        // fixture still resolves a path that IS inside.
        repo_path_in(&f.dir, "src/main.rs").expect("a path inside the root still resolves");
        let _ = std::fs::remove_file(&outside);
    }

    /// The resolved-prefix half, which a string prefix test would pass.
    ///
    /// The root is spelled through its CANONICAL form here, because on
    /// macOS the temp directory is under `/var`, a symlink to
    /// `/private/var`. A guard that compared unresolved paths would
    /// "reject" this for the wrong reason and a guard that compared
    /// strings would accept a sibling whose name merely extends the
    /// root's.
    #[test]
    fn a_sibling_directory_sharing_the_roots_name_prefix_is_refused() {
        let f = Fixture::new("prefix");
        f.write("keep.txt", b"in");
        // `<root>-evil` has `<root>` as a STRING prefix and is not inside
        // it. Reached through `..`, so the component check catches it
        // first -- which is the point: the shape check and the resolved
        // comparison are belt and braces for the same escape.
        let evil = {
            let mut p = f.dir.clone().into_os_string();
            p.push("-evil");
            std::path::PathBuf::from(p)
        };
        std::fs::create_dir_all(&evil).expect("sibling");
        std::fs::write(evil.join("secret.txt"), b"out").expect("sibling file");
        let rel = format!(
            "../{}/secret.txt",
            evil.file_name().and_then(|n| n.to_str()).expect("name")
        );
        assert!(
            repo_path_in(&f.dir, &rel).is_err(),
            "a sibling sharing the root's string prefix must be refused"
        );
        // Prove the resolved comparison itself, not only the component
        // check: the canonical root must not be a prefix of the sibling.
        let root = f.dir.canonicalize().expect("canonical root");
        let sibling = evil.canonicalize().expect("canonical sibling");
        assert!(
            !sibling.starts_with(&root),
            "{} must not be inside {}",
            sibling.display(),
            root.display()
        );
        let _ = std::fs::remove_dir_all(&evil);
    }

    /// The escape that ONLY the resolved comparison can catch, and the
    /// reason step 4 is not redundant with step 1.
    ///
    /// A symlinked intermediate DIRECTORY. `link/secret.txt` has no `..`
    /// component and is not absolute, so the shape check passes it; and
    /// `symlink_metadata` follows every component except the LAST, so the
    /// symlink refusal sees a plain file and passes it too. The path is
    /// legal by every syntactic test and still resolves outside the root.
    ///
    /// Written after sabotaging step 4 and watching all twenty-three
    /// tests pass without it -- which is the only way to find an
    /// assertion that cannot fail, and the same method
    /// `emptyStateGuard.test.ts` records having used on `RepoPickerSidebar`.
    ///
    /// The refusal is spelled through the RESOLVED root deliberately: on
    /// macOS the temp directory is `/var/...`, a symlink to
    /// `/private/var/...`, so a guard comparing an unresolved root
    /// rejects every path on a real machine and a test asserting only
    /// "some error" would pass on that broken guard for the wrong reason.
    /// So this asserts the containment message AND that an ordinary file
    /// inside the same root still resolves.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_intermediate_directory_cannot_smuggle_a_path_out_of_the_root() {
        let f = Fixture::new("viadir");
        f.write("keep.txt", b"in");
        let outside = f.dir.parent().expect("parent").join(format!(
            "headstate-repos-outside-{}-{}",
            std::process::id(),
            "dir"
        ));
        std::fs::create_dir_all(&outside).expect("outside dir");
        std::fs::write(outside.join("secret.txt"), b"id_rsa").expect("outside file");
        // The LINK is a directory link inside the root; the path through
        // it names a plain file, so nothing but the resolved comparison
        // is left to notice.
        std::os::unix::fs::symlink(&outside, f.dir.join("link")).expect("dir symlink");

        let err = repo_path_in(&f.dir, "link/secret.txt")
            .expect_err("a symlinked intermediate directory must not smuggle a path out");
        let root = f.dir.canonicalize().expect("canonical root");
        assert!(
            err.contains("is not inside") && err.contains(&root.display().to_string()),
            "the refusal must be the CONTAINMENT one, naming the resolved root, got: {err}"
        );
        // And the guard has not simply become "refuse everything", which
        // is what comparing against an unresolved root does on macOS.
        repo_path_in(&f.dir, "keep.txt").expect("an ordinary file inside the root still resolves");

        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let f = Fixture::new("absolute");
        f.write("keep.txt", b"in");
        let err = repo_path_in(&f.dir, "/etc/passwd").expect_err("must refuse an absolute path");
        assert!(err.contains("absolute"), "got: {err}");
    }

    /// A symlink is refused BEFORE canonicalising, which is the only
    /// order in which it is detectable at all.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused_even_when_its_target_is_inside_the_root() {
        let f = Fixture::new("symlink");
        f.write("real.txt", b"inside");
        std::os::unix::fs::symlink(f.dir.join("real.txt"), f.dir.join("link.txt"))
            .expect("symlink");
        let err = repo_path_in(&f.dir, "link.txt").expect_err("must refuse a symlink");
        assert!(
            err.contains("symbolic link"),
            "the refusal must say it is a link, got: {err}"
        );
        // The target itself is still readable -- the refusal is about the
        // link, not about the contents.
        repo_path_in(&f.dir, "real.txt").expect("the target itself resolves");
    }

    /// A symlink pointing OUT of the repository is the case containment
    /// exists for, and it is refused at the same step.
    #[cfg(unix)]
    #[test]
    fn a_symlink_escaping_the_root_is_refused() {
        let f = Fixture::new("escapelink");
        f.write("keep.txt", b"in");
        let outside = f
            .dir
            .parent()
            .expect("parent")
            .join(format!("headstate-repos-target-{}.txt", std::process::id()));
        std::fs::write(&outside, b"secret").expect("outside file");
        std::os::unix::fs::symlink(&outside, f.dir.join("escape.txt")).expect("symlink");
        assert!(
            repo_path_in(&f.dir, "escape.txt").is_err(),
            "a symlink leaving the root must be refused"
        );
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn a_missing_path_says_it_could_not_be_read() {
        let f = Fixture::new("missing");
        f.write("keep.txt", b"in");
        let err = repo_path_in(&f.dir, "gone.txt").expect_err("must refuse a missing path");
        assert!(err.contains("could not be read"), "got: {err}");
    }

    // ---- the listing (#1031) ----------------------------------------

    #[test]
    fn the_root_listing_is_directories_first_then_files_by_name() {
        let f = Fixture::new("listing");
        f.init();
        f.write("zeta.txt", b"z");
        f.write("alpha.txt", b"a");
        f.write("src/main.rs", b"fn main() {}");
        f.write("docs/readme.md", b"# hi");
        f.add_all();
        let t = tree(&f.dir, "").expect("listing");
        let names: Vec<&str> = t.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["docs", "src", "alpha.txt", "zeta.txt"]);
        assert!(t.entries[0].dir);
        assert!(!t.entries[2].dir);
        assert_eq!(t.entries[2].path, "alpha.txt");
        assert_eq!(t.path, "");
    }

    #[test]
    fn a_subdirectory_lists_only_its_own_level() {
        let f = Fixture::new("descend");
        f.init();
        f.write("src/main.rs", b"fn main() {}");
        f.write("src/deep/inner.rs", b"//");
        f.write("top.txt", b"t");
        f.add_all();
        let t = tree(&f.dir, "src").expect("listing");
        let names: Vec<&str> = t.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["deep", "main.rs"]);
        // The paths are repository-relative, not directory-relative, so a
        // caller can descend or read with the value it was handed.
        assert_eq!(t.entries[0].path, "src/deep");
        assert_eq!(t.entries[1].path, "src/main.rs");
        assert_eq!(t.path, "src");
    }

    /// Untracked build output is not in the index and therefore not in
    /// the listing -- which IS the feature (#1031).
    #[test]
    fn untracked_files_are_absent_from_the_listing() {
        let f = Fixture::new("untracked");
        f.init();
        f.write("src/main.rs", b"fn main() {}");
        f.add_all();
        // Written AFTER the add, so it is on disk and not in the index --
        // which is what `target/debug/deps` is on a real machine.
        f.write("src/artifact.o", b"junk");
        let t = tree(&f.dir, "src").expect("listing");
        let names: Vec<&str> = t.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["main.rs"],
            "the listing comes from the index, so untracked output must not appear"
        );
    }

    /// Absent is not zero: a directory git listed and found nothing in is
    /// an empty `Tree`, and a git that FAILED is an `Err`.
    #[test]
    fn a_directory_with_no_tracked_files_lists_empty_rather_than_failing() {
        let f = Fixture::new("emptydir");
        f.init();
        f.write("src/main.rs", b"fn main() {}");
        f.add_all();
        std::fs::create_dir_all(f.dir.join("scratch")).expect("dir");
        let t = tree(&f.dir, "scratch").expect("an empty directory is an ANSWER, not a failure");
        assert!(t.entries.is_empty());
        assert_eq!(t.path, "scratch");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_an_error_not_an_empty_listing() {
        let f = Fixture::new("notarepo");
        // No `init`, so there is no index at all.
        f.write("src/main.rs", b"fn main() {}");
        let err = tree(&f.dir, "")
            .expect_err("git failing must be an Err, never an empty listing (#846)");
        assert!(
            !err.is_empty(),
            "the failure must carry git's own message, since the three causes send the user to three different places"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_tracked_symlink_is_listed_as_a_link() {
        let f = Fixture::new("linklisting");
        f.init();
        f.write("real.txt", b"hi");
        std::os::unix::fs::symlink("real.txt", f.dir.join("link.txt")).expect("symlink");
        f.add_all();
        let t = tree(&f.dir, "").expect("listing");
        let link = t
            .entries
            .iter()
            .find(|e| e.name == "link.txt")
            .expect("the link is SHOWN, not hidden");
        assert!(link.symlink, "a tracked symlink must be marked as one");
        assert!(!link.dir);
        // And reading it is still refused, which is the pair this
        // behaviour comes in: shown in the listing, not followed.
        assert!(file(&f.dir, "link.txt").is_err());
    }

    #[test]
    fn a_listing_path_outside_the_repository_is_refused() {
        let f = Fixture::new("listescape");
        f.init();
        f.write("src/main.rs", b"fn main() {}");
        f.add_all();
        assert!(
            tree(&f.dir, "../..").is_err(),
            "the guard must cover the LISTING too, or it enumerates arbitrary directories"
        );
    }

    // ---- the bounded read (#1033) -----------------------------------

    #[test]
    fn a_small_text_file_arrives_whole_and_untruncated() {
        let f = Fixture::new("small");
        f.write("hello.txt", b"hello\n");
        let r = file(&f.dir, "hello.txt").expect("read");
        assert_eq!(r.content, "hello\n");
        assert_eq!(r.size, 6);
        assert!(!r.truncated);
        assert!(!r.binary);
    }

    /// 300 KB against a 256 KB bound: cut, and SAID so.
    #[test]
    fn a_file_over_the_bound_is_truncated_and_says_so() {
        let f = Fixture::new("big");
        let body = vec![b'a'; 300 * 1024];
        f.write("big.txt", &body);
        let r = file(&f.dir, "big.txt").expect("read");
        assert_eq!(
            r.size,
            300 * 1024,
            "the REAL size is reported, not the window's"
        );
        assert_eq!(
            r.content.len() as u64,
            MAX_FILE_BYTES,
            "the window is the bound, enforced inside the command"
        );
        assert!(
            r.truncated,
            "a window shown as if it were the whole file is worse than a refusal"
        );
        assert!(!r.binary);
    }

    /// Detected by NUL byte, never by extension: this one is called
    /// `.txt` and the corpus has extensionless binaries and 9.3 MB `.py`
    /// files that are text.
    #[test]
    fn a_binary_file_is_refused_with_a_reason_rather_than_rendered() {
        let f = Fixture::new("binary");
        f.write("payload.txt", &[0x7f, 0x45, 0x4c, 0x46, 0x00, 0x01, 0x02]);
        let r = file(&f.dir, "payload.txt").expect("a binary file is READ, and is not an error");
        assert!(r.binary, "a NUL byte in the head makes it binary");
        assert!(r.content.is_empty(), "no bytes are returned for a binary");
        assert_eq!(
            r.size, 7,
            "the size is still reported, so the UI can name it"
        );
    }

    /// The third outcome, which must render as its own thing: read
    /// perfectly well, genuinely empty. There are real ones (`.gitkeep`).
    #[test]
    fn an_empty_file_is_read_and_is_neither_binary_nor_an_error() {
        let f = Fixture::new("emptyfile");
        f.write(".gitkeep", b"");
        let r = file(&f.dir, ".gitkeep").expect("an empty file READS");
        assert_eq!(r.size, 0);
        assert!(r.content.is_empty());
        assert!(!r.binary, "empty is not binary");
        assert!(!r.truncated);
    }

    /// And the first outcome: could not be READ, which is an `Err` and
    /// never an empty string.
    #[test]
    fn a_vanished_file_is_an_error_not_empty_content() {
        let f = Fixture::new("vanished");
        f.write("keep.txt", b"in");
        let err = file(&f.dir, "gone.txt").expect_err("a missing file is an Err");
        assert!(err.contains("could not be read"), "got: {err}");
    }

    #[test]
    fn a_directory_is_not_read_as_a_file() {
        let f = Fixture::new("readdir");
        f.write("src/main.rs", b"fn main() {}");
        let err = file(&f.dir, "src").expect_err("a directory is not a file");
        assert!(err.contains("directory"), "got: {err}");
    }

    #[test]
    fn a_read_outside_the_repository_is_refused() {
        let f = Fixture::new("readescape");
        f.write("keep.txt", b"in");
        assert!(file(&f.dir, "../../etc/passwd").is_err());
    }

    // ---- path spelling ----------------------------------------------

    #[test]
    fn a_path_is_normalised_but_a_dot_dot_is_never_normalised_away() {
        assert_eq!(normalise("/src/"), "src");
        assert_eq!(normalise(""), "");
        assert_eq!(normalise("/"), "");
        // NOT rewritten: the guard must be the one to name it.
        assert_eq!(normalise("../etc"), "../etc");
    }

    #[test]
    fn a_child_path_joins_without_a_leading_separator_at_the_root() {
        assert_eq!(child_path("", "src"), "src");
        assert_eq!(child_path("src", "main.rs"), "src/main.rs");
    }
}
