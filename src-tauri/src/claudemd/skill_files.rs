//! The skills a session under a repository can load: every `SKILL.md`, at
//! any depth, under the repository's `.claude/skills` and the user's
//! `~/.claude/skills`.
//!
//! Shared by the transcripts producer's "already written" corpus (#1370)
//! and the toolchain producer's "names" corpus (#1394), so the two agree
//! on which files are skills. Plugin skills are out of scope for both:
//! they are not the repository owner's to change (#1365).
//!
//! # Unreadable is not "no skills"
//!
//! A skills directory that does not exist holds nothing, an ordinary
//! answer. One that exists and could not be listed might hold the file a
//! caller is looking for, so it is recorded in [`Skills::unreadable`] with
//! the io error, and a caller stating a negative must say Unknown (#1351).
//! Reading each `SKILL.md` is the caller's, and so is a read that fails.

use std::path::{Path, PathBuf};

/// Every skill file found, and every skills directory that could not be
/// listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skills {
    /// `(path, name)`, the name being the directory holding the file, in
    /// walk order (repository first, then the user's, each sorted).
    pub files: Vec<(String, String)>,
    /// `(path, io error)` for a skills directory that exists and could
    /// not be listed.
    pub unreadable: Vec<(String, String)>,
}

/// How deep a skills walk goes, so a symlink cycle ends.
const SKILL_DEPTH: usize = 8;

/// The skills under `repo/.claude/skills` and, with a home directory,
/// `home/.claude/skills`.
pub fn read(repo: &Path, home: Option<&Path>) -> Skills {
    let mut out = Skills::default();
    for root in std::iter::once(repo).chain(home) {
        walk(&root.join(".claude").join("skills"), 0, &mut out);
    }
    out
}

/// Not-found, or a path component that is not a directory: nothing there.
fn is_gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn walk(dir: &Path, depth: usize, out: &mut Skills) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // Absent holds nothing.
        Err(e) if is_gone(&e) => return,
        Err(e) => {
            out.unreadable
                .push((dir.to_string_lossy().into_owned(), e.to_string()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => paths.push(e.path()),
            Err(e) => {
                out.unreadable
                    .push((dir.to_string_lossy().into_owned(), e.to_string()));
            }
        }
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < SKILL_DEPTH {
                walk(&path, depth + 1, out);
            }
        } else if path.file_name() == Some(std::ffi::OsStr::new("SKILL.md")) {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.files.push((path.to_string_lossy().into_owned(), name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Repository skills come first, nested ones count, a file not named
    /// `SKILL.md` does not, and an absent directory is nothing.
    #[test]
    fn skills_are_read_from_the_repository_and_the_home_directory() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let home = t.path().join("home");
        let a = repo.join(".claude").join("skills").join("a");
        let b = home.join(".claude").join("skills").join("group").join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("SKILL.md"), "a").unwrap();
        fs::write(a.join("notes.md"), "not a skill").unwrap();
        fs::write(b.join("SKILL.md"), "b").unwrap();

        let got = read(&repo, Some(&home));
        let names: Vec<&str> = got.files.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(got.unreadable.is_empty());

        assert_eq!(read(&t.path().join("none"), None), Skills::default());
    }
}
