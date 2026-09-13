use serde::{Deserialize, Serialize};

/// Which toolchain owns a project's dependencies.
// `Ord` so an ecosystem can be half of a map key: `registry::enrich`
// de-duplicates lookups by (ecosystem, name), because `aws` is both a
// Terraform provider and a crate and one must not answer for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Npm,
    Yarn,
    Poetry,
    Uv,
    Dotnet,
    /// CocoaPods, detected from a `Podfile`.
    Cocoapods,
    /// Terraform providers, from `.terraform.lock.hcl`.
    ///
    /// The only ecosystem here that needs NO tool installed. The lock
    /// file carries the resolved version in plain text, and the newest
    /// comes from the registry over HTTP -- so unlike every other one,
    /// a missing `terraform` binary cannot make this report nothing.
    Terraform,
    /// Swift packages, whether a `Package.swift` or Xcode-managed.
    ///
    /// Xcode-managed dependencies have NO CLI that reports outdated
    /// packages -- `xcodebuild -resolvePackageDependencies` resolves but
    /// does not diff. So this reports the pinned versions from
    /// `Package.resolved` and says plainly that it cannot check them,
    /// rather than rendering an empty list that reads as "up to date".
    Swift,
    /// Rust crates, from `Cargo.toml` plus `Cargo.lock`.
    ///
    /// The THIRD ecosystem needing no tool installed, for the same
    /// reason as Terraform. Cargo has no `npm outdated`, and the
    /// subcommands that come close (`cargo outdated`, `cargo upgrade`)
    /// are third-party installs -- so a missing one would silence the
    /// whole ecosystem, which is the inversion this module refuses.
    /// The lockfile carries the resolved version and the crates.io
    /// SPARSE INDEX answers "what is newest" over plain HTTPS.
    Cargo,
}

impl Ecosystem {
    /// The executable this ecosystem needs.
    pub fn program(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm",
            Ecosystem::Yarn => "yarn",
            Ecosystem::Poetry => "poetry",
            Ecosystem::Uv => "uv",
            Ecosystem::Dotnet => "dotnet",
            Ecosystem::Cocoapods => "pod",
            // Never spawned; see the variant's comment.
            Ecosystem::Terraform => "terraform",
            Ecosystem::Swift => "swift",
            // Never spawned; see the variant's comment. `cargo` is on
            // the machine if there is a Cargo.toml, but it answers a
            // different question -- `cargo update --dry-run` reports
            // what the RESOLVER would move to within the existing
            // constraints, not what the newest published version is.
            Ecosystem::Cargo => "cargo",
        }
    }

    /// How a user updates one package here, for the markdown handoff.
    pub fn update_hint(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm install <pkg>@<version>",
            Ecosystem::Yarn => "yarn up <pkg>@<version>",
            Ecosystem::Poetry => "poetry add <pkg>@<version>",
            Ecosystem::Uv => "uv add <pkg>==<version>",
            Ecosystem::Dotnet => "dotnet add package <pkg> --version <version>",
            Ecosystem::Cocoapods => "pod update <pkg>",
            Ecosystem::Terraform => "raise the version constraint, then terraform init -upgrade",
            Ecosystem::Swift => "update the version rule in Xcode, or Package.swift",
            Ecosystem::Cargo => "cargo add <pkg>@<version>",
        }
    }

    /// This ecosystem's name as it appears in a branch name (#797).
    ///
    /// Exactly the `serde(rename_all = "snake_case")` spelling of the
    /// variant, which is what `Ecosystem` already is on the wire and
    /// therefore what the TypeScript `Ecosystem` union holds. That is the
    /// point: `branch_name` and `derivedBranchName` must agree on the
    /// string, and the one thing both sides are already guaranteed to
    /// spell identically is the serialised form.
    ///
    /// NOT `program()`. That is the executable -- `pod` for CocoaPods --
    /// and a branch called `headstate/pod-deps-…` names the tool rather
    /// than the ecosystem. Every value here is ASCII lowercase letters,
    /// so nothing it produces needs sanitising for a ref.
    pub fn slug(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm",
            Ecosystem::Yarn => "yarn",
            Ecosystem::Poetry => "poetry",
            Ecosystem::Uv => "uv",
            Ecosystem::Dotnet => "dotnet",
            Ecosystem::Cocoapods => "cocoapods",
            Ecosystem::Terraform => "terraform",
            Ecosystem::Swift => "swift",
            Ecosystem::Cargo => "cargo",
        }
    }

    /// The manifest whose presence means this ecosystem is in use.
    pub fn manifest(self) -> &'static str {
        match self {
            Ecosystem::Npm | Ecosystem::Yarn => "package.json",
            Ecosystem::Poetry | Ecosystem::Uv => "pyproject.toml",
            Ecosystem::Cocoapods => "Podfile",
            Ecosystem::Terraform => ".terraform.lock.hcl",
            Ecosystem::Swift => "Package.resolved",
            Ecosystem::Cargo => "Cargo.toml",
            // .NET is a glob, handled by the detector rather than here.
            Ecosystem::Dotnet => "",
        }
    }
}

/// How large a version jump is.
///
/// `Unknown` is a first-class answer rather than a fallback to `Major`.
/// Version schemes here are NOT all semver: .NET routinely uses four
/// parts, and PEP 440 has epochs and local versions that a semver parser
/// rejects outright. A version we cannot compare must be shown as
/// uncomparable, because silently calling it major would hide it from a
/// "minors only" filter and silently calling it minor would offer it as
/// safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bump {
    Patch,
    Minor,
    Major,
    Unknown,
}

/// One package with an update available.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outdated {
    pub name: String,
    pub current: String,
    pub latest: String,
    pub bump: Bump,
    pub ecosystem: Ecosystem,
    /// The manifest to edit, relative to the repo. What a Claude session
    /// needs in order to act without rediscovering it.
    pub manifest: String,
}

/// What one ecosystem reported for one repository.
///
/// A result rather than a bare list, because "no updates" and "the tool
/// is not installed" are completely different answers and rendering both
/// as an empty list is the worst outcome available -- it looks like good
/// news.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcosystemReport {
    pub ecosystem: Ecosystem,
    pub outdated: Vec<Outdated>,
    /// Set when the check could not run. The UI shows this instead of an
    /// empty list.
    pub error: Option<String>,
}

/// What a FILE-reading ecosystem scan found, including what it could not
/// read.
///
/// The unreadable list is the point of the type, exactly as it is for
/// `claude::transcript::Scan`. Cargo, Swift and Terraform answer from
/// files rather than from a command, so there is no spawn failure to
/// report and `error` was left hardcoded `None` (#954) -- which made
/// every read failure beneath them unrepresentable. An unreadable
/// `Cargo.lock` dropped every crate and the page said "up to date": a
/// confident wrong answer where the truth is "we could not look".
///
/// A bare `Vec` cannot carry that, so this does. `run::check` turns a
/// non-empty `unreadable` into the `error` the UI already renders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileScan {
    /// What DID read. Never discarded because something else did not: a
    /// partial answer labelled partial beats both a silent truncation and
    /// an error page, the rule `claude::transcript::Scan::is_partial`
    /// states.
    pub outdated: Vec<Outdated>,
    /// Paths that could not be read, with why.
    ///
    /// Messages rather than a count, because a lock file hides an
    /// unknown number of packages and naming it is what makes the report
    /// actionable.
    pub unreadable: Vec<String>,
}

impl FileScan {
    /// Whether anything at all could not be read.
    ///
    /// Deliberately NOT a reason to discard `outdated`; see the field's
    /// own comment.
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }

    /// The sentence the UI shows instead of an empty list.
    ///
    /// `None` when nothing failed, so a caller can write
    /// `error: scan.error()` and have the no-failure case be exactly the
    /// `None` it was before.
    ///
    /// Says WHAT was skipped and that the rest is still real, because
    /// this text lands beside a list the user can see: a message that
    /// only said "failed" would read as though the whole list were
    /// suspect.
    pub fn error(&self) -> Option<String> {
        if self.unreadable.is_empty() {
            return None;
        }
        let count = self.unreadable.len();
        Some(format!(
            "{count} file{} could not be read, so this list is incomplete: {}",
            if count == 1 { "" } else { "s" },
            self.unreadable.join("; ")
        ))
    }

    /// Record an unreadable path.
    pub(super) fn failed(&mut self, path: &std::path::Path, why: &std::io::Error) {
        self.unreadable.push(format!("{} ({why})", path.display()));
    }

    /// Record an unreadable path from a message that is not an
    /// `io::Error` -- a malformed file, which is a read that SUCCEEDED
    /// and produced nothing usable.
    pub(super) fn failed_with(&mut self, path: &std::path::Path, why: &str) {
        self.unreadable.push(format!("{} ({why})", path.display()));
    }
}

/// One project's worth of reports.
///
/// The unit the UI groups by. A repository can hold several, and their
/// updates are separate pieces of work: different manifests, and
/// sometimes different ecosystems entirely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectReport {
    /// Absolute path to the project directory.
    pub path: String,
    /// Relative to the repository root. Empty at the root itself.
    pub label: String,
    pub reports: Vec<EcosystemReport>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ecosystem. A literal list is the only way to enumerate a
    /// Rust enum without a derive; what stops a stale one from passing
    /// quietly is that a new variant has to be spelled in
    /// `src/lib/branchName.ts` too, and `branchName.test.ts` asserts the
    /// two agree on the slug.
    const EVERY: &[Ecosystem] = &[
        Ecosystem::Npm,
        Ecosystem::Yarn,
        Ecosystem::Poetry,
        Ecosystem::Uv,
        Ecosystem::Dotnet,
        Ecosystem::Cocoapods,
        Ecosystem::Terraform,
        Ecosystem::Swift,
        Ecosystem::Cargo,
    ];

    /// `slug` must BE the serialised name, not merely resemble it.
    ///
    /// The slug goes into a branch name that `derivedBranchName` has to
    /// predict without a round trip (#797), and the only string the two
    /// sides are guaranteed to spell identically is the one that crosses
    /// the wire -- the TypeScript `Ecosystem` union IS this serde form.
    /// Derived from `serde_json` here rather than typed out, so a variant
    /// renamed on one side cannot pass by having a hand-written copy
    /// renamed to match.
    #[test]
    fn slug_matches_the_serialised_name() {
        for eco in EVERY {
            let json = serde_json::to_string(eco).expect("an ecosystem serialises");
            assert_eq!(
                format!("\"{}\"", eco.slug()),
                json,
                "{eco:?}: slug and serde form disagree"
            );
        }
    }

    /// A slug reaches `git worktree add -b`, so it must need no
    /// sanitising. Every value is ASCII lowercase today; this is what
    /// stops a future variant spelled with an underscore or a dot from
    /// quietly changing what a ref looks like.
    #[test]
    fn every_slug_is_safe_in_a_ref() {
        for eco in EVERY {
            let s = eco.slug();
            assert!(!s.is_empty(), "{eco:?} has an empty slug");
            assert!(
                s.chars().all(|c| c.is_ascii_lowercase()),
                "{eco:?} slug {s:?} is not plain lowercase ASCII"
            );
        }
    }
}
