//! Applying dependency updates in a throwaway worktree.
//!
//! The FIRST code in this app that runs a package manager in a mode that
//! writes. Everything else in `packages/` queries -- `outdated`, `list`,
//! `show --outdated` -- so the care here is deliberate and the contract
//! is borrowed from `pull_checkout`, the only other write path: re-check
//! state fresh rather than trusting a scan, do the narrowest operation
//! that answers the request, and return the tool's OWN error rather than
//! a generic one.
//!
//! Phase 1 does not push and does not open a pull request. It leaves a
//! worktree with changes applied for the user to inspect. That boundary
//! is the point: what these tools actually do to a checkout is not
//! knowable from documentation, and the way to find out is to look at
//! the result before adding anything irreversible on top.
//!
//! ## The resolver decides, not the request
//!
//! Asking for `1.2.3` does not mean `1.2.3` is what lands. Resolvers
//! reconcile the whole dependency graph, and a peer conflict, a
//! transitive pin, or a yanked release can produce something else --
//! or nothing. So every apply is followed by a re-read of the manifest
//! state, and the report says what is ACTUALLY there. A wizard that
//! echoes back the version it asked for is not reporting, it is
//! guessing.

use super::model::Ecosystem;
use super::tools;
use std::path::Path;

/// Why an ecosystem cannot be updated automatically.
///
/// A first-class value rather than an absence: "not supported" and "no
/// updates" must never render the same way, which is the same rule
/// `EcosystemReport` follows for its error field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// No command exists that does this.
    NoCommand(&'static str),
}

/// Whether this ecosystem can apply a single-package update.
///
/// Swift is refused outright. `check` already refuses to REPORT Swift
/// updates, because nothing diffs Xcode-managed dependencies -- and a
/// tool that cannot tell you a package is outdated has no business
/// deciding it should be updated.
pub fn supported(eco: Ecosystem) -> Result<(), Unsupported> {
    match eco {
        // Refused for the same reason as Swift, by a different route:
        // a provider version lives as a CONSTRAINT in `.tf` source, and
        // `terraform init -upgrade` moves it only as far as that
        // constraint allows. Applying an update means editing the
        // constraint, which is a source change rather than a lockfile
        // one -- and guessing at someone's version policy is not this
        // command's job.
        Ecosystem::Terraform => Err(Unsupported::NoCommand(
            "Terraform providers cannot be updated here: the version is a \
             constraint in your .tf source, not something a lockfile edit \
             can change.",
        )),
        // Allowed since #559, and NOT by running `cargo add`. All three
        // of that tool's wrong edits were re-measured against cargo
        // 1.98.1 and all three still happen -- including one `--package`
        // does not fix (it severs `workspace = true` inheritance either
        // way) and one #560 never found (it flattens the constraint
        // style, turning `serde = "1"` into `"1.0.229"` and `log =
        // "=0.4.20"` into `"0.4.30"`).
        //
        // So `packages::cargo_apply` edits the manifest with `toml_edit`
        // directly, resolving WHICH manifest through `cargo::declared` --
        // the same walk that produced the row -- and preserving both the
        // why-comments and the constraint style. It is routed in
        // `apply_one` rather than through `update_args`, because it
        // spawns nothing.
        Ecosystem::Cargo => Ok(()),
        Ecosystem::Swift => Err(Unsupported::NoCommand(
            "Swift packages cannot be updated here: nothing reports what is \
             outdated, so there is no version to move to. Update the version \
             rule in Xcode or Package.swift.",
        )),
        _ => Ok(()),
    }
}

/// Reject a name or version that would be read as a FLAG.
///
/// No shell is involved anywhere here -- every argument is passed as its
/// own argv entry -- so this is not shell injection. The risk is argv
/// flag smuggling: a package called `--registry=http://elsewhere` is a
/// valid operand to `Command`, and the package manager reads it as an
/// option rather than a package name.
///
/// These values come from a scan today, but `apply_package_updates` is
/// an IPC command that accepts arbitrary strings, and package names come
/// from registry metadata rather than from anything this app controls.
/// Validating at the boundary is cheaper than reasoning about every
/// caller.
///
/// Deliberately a REJECTION rather than an escape: `--` end-of-options
/// handling differs across these seven tools, and a rejected update the
/// user can see beats a silently rewritten one.
fn reject_flaglike(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("empty {field}"));
    }
    if value.starts_with('-') {
        return Err(format!(
            "{field} {value:?} starts with '-', which a package manager \
             would read as an option rather than a value"
        ));
    }
    // A newline would split one argument into two for any tool that
    // re-parses its input, and no real name or version contains one.
    if value.contains(['\n', '\r', '\0']) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(())
}

/// The command that updates ONE package to a specific version.
///
/// Single-package and version-pinned on purpose. The blunt alternatives
/// (`npm update`, `pod update` with no argument) move everything they
/// can, which produces a diff nobody asked for and cannot be reviewed
/// package by package.
///
/// CocoaPods is the exception and is marked as such: `pod update <pkg>`
/// takes no version, so it moves to whatever the Podfile's constraints
/// allow. That is why the applied version is read back afterwards
/// instead of assumed.
/// Whether the manifest pinned this package to an EXACT version.
///
/// A production dependency list that says `4.17.20` means it: the point
/// of a pin is that the version does not move on its own. npm's default
/// widens that to `^4.17.21` on update, which quietly converts a pinned
/// project into a floating one -- measured, and the reason `--save-exact`
/// exists.
///
/// But applying `--save-exact` unconditionally is the opposite mistake:
/// a project that deliberately wrote `^4.17.20` would be NARROWED to a
/// pin it never asked for. Verified both ways against real npm.
///
/// So the manifest's own style decides. Absent or unreadable counts as
/// NOT pinned, which leaves npm's default behaviour -- the conservative
/// direction, since it changes nothing about how this worked before.
fn was_pinned(dir: &Path, eco: Ecosystem, name: &str) -> bool {
    let Some(current) = read_constraint(dir, eco, name) else {
        return false;
    };
    // A pin is a bare version: no range operator of any kind.
    !current.is_empty()
        && current.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !current.contains([' ', '|', '-', '*', 'x'])
}

pub fn update_args(dir: &Path, eco: Ecosystem, name: &str, version: &str) -> Vec<String> {
    let s = |v: &str| v.to_string();
    match eco {
        // `--save-exact` only when the manifest was already exact, so
        // an upgrade keeps the style the project chose rather than
        // imposing one.
        Ecosystem::Npm if was_pinned(dir, eco, name) => {
            vec![s("install"), s("--save-exact"), format!("{name}@{version}")]
        }
        Ecosystem::Npm => vec![s("install"), format!("{name}@{version}")],
        // Yarn Berry's equivalent. `yarn up` writes a range by default
        // for the same reason npm does.
        Ecosystem::Yarn if was_pinned(dir, eco, name) => {
            vec![s("up"), s("--exact"), format!("{name}@{version}")]
        }
        Ecosystem::Yarn => vec![s("up"), format!("{name}@{version}")],
        Ecosystem::Poetry => vec![s("add"), format!("{name}@{version}")],
        Ecosystem::Uv => vec![s("add"), format!("{name}=={version}")],
        Ecosystem::Dotnet => vec![s("add"), s("package"), s(name), s("--version"), s(version)],
        // No version argument exists for `pod update`.
        Ecosystem::Cocoapods => vec![s("update"), s(name)],
        // Unreachable: `supported` refuses Terraform first.
        Ecosystem::Terraform => vec![s("version")],
        // Unreachable: `supported` refuses Swift before this is called.
        // Kept total rather than panicking, because a panic here would
        // take down the whole command for a case the caller already
        // guarded.
        Ecosystem::Swift => vec![s("--version")],
        // Unreachable: `apply_one_in` handles Cargo before this is
        // called, by editing the manifest rather than spawning
        // anything. Kept total rather than panicking, for the same
        // reason as Swift above.
        Ecosystem::Cargo => vec![s("--version")],
    }
}

/// What happened when one update was applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub name: String,
    /// The version REQUESTED.
    pub requested: String,
    /// Files git reports as changed, relative to the worktree.
    ///
    /// Empty means the command reported success and changed nothing,
    /// which is a real and important outcome: it usually means a
    /// constraint in the manifest pinned the package below the version
    /// asked for. Reported rather than hidden.
    pub changed_files: Vec<String>,
    /// The tool's own stderr, kept whether or not it failed. Resolvers
    /// warn about peer conflicts on the success path and those warnings
    /// are the most useful thing in the output.
    pub output: String,
    /// The constraint the manifest holds AFTER the update, when it could
    /// be read back.
    ///
    /// Not cosmetic. Measured on a real run: asking npm for `4.17.21`
    /// leaves `^4.17.21` in package.json -- a CARET RANGE, not the pin
    /// that was requested. The distinction is the difference between
    /// "this version" and "anything compatible with it", and a wizard
    /// that echoed back the requested string would have reported the
    /// wrong thing while appearing to succeed.
    ///
    /// `None` when the manifest could not be parsed for this package,
    /// which is reported as unknown rather than assumed to match.
    pub resolved_constraint: Option<String>,
}

/// Run one update inside `dir` and report what changed.
///
/// `dir` must be a worktree created for this purpose. Nothing here
/// checks that, because the caller creates it; this function's contract
/// is only that it does not reach outside `dir`.
///
/// `project` is the subdirectory the manifest lives in, and is where
/// the tool runs; `dir` stays the worktree, because that is what `git
/// status` must be asked about -- a resolver routinely rewrites a
/// lockfile ABOVE the project directory, and asking git only about the
/// project would drop it from the report.
pub fn apply_one(dir: &Path, eco: Ecosystem, name: &str, version: &str) -> Result<Applied, String> {
    apply_one_in(dir, dir, eco, name, version)
}

pub fn apply_one_in(
    dir: &Path,
    project: &Path,
    eco: Ecosystem,
    name: &str,
    version: &str,
) -> Result<Applied, String> {
    supported(eco).map_err(|Unsupported::NoCommand(m)| m.to_string())?;
    reject_flaglike("package name", name)?;
    reject_flaglike("version", version)?;

    // Cargo spawns NOTHING. See `supported`: its tool makes three
    // separate wrong edits on a workspace, so the manifest is edited
    // directly and the version that lands is read back off disk the
    // same way every other path reads its manifest back.
    if eco == Ecosystem::Cargo {
        let edited = super::cargo_apply::apply(project, name, version)?;
        return Ok(Applied {
            name: name.to_string(),
            requested: version.to_string(),
            changed_files: changed_files(dir),
            // Not a tool's stderr -- there is no tool. Says which file
            // was edited and which table, because on a workspace that
            // is the whole question, and adds the lockfile caveat so it
            // is stated rather than discovered.
            output: if edited.written {
                format!(
                    "{} [{}]: {} -> {}. {}",
                    edited.manifest,
                    edited.table,
                    edited.before,
                    edited.after,
                    super::cargo_apply::LOCKFILE
                )
            } else {
                format!(
                    "{} [{}]: left at {:?}, which already admits {version}. \
                     Narrowing it would rewrite a version policy that was not \
                     asked about.",
                    edited.manifest, edited.table, edited.before
                )
            },
            resolved_constraint: Some(edited.after),
        });
    }

    let fallbacks = tools::fallback_dirs();
    let refs: Vec<&str> = fallbacks.iter().map(String::as_str).collect();
    let bin = tools::find(eco.program(), &refs)
        .ok_or_else(|| format!("{} is not installed", eco.program()))?;

    // Read from and run in the PROJECT: the manifest whose style
    // `was_pinned` inspects is that project's, not the worktree root's.
    let args = update_args(project, eco, name, version);
    let out = std::process::Command::new(&bin)
        .args(&args)
        // Same reason as the update check: an interpreted tool starts a
        // second lookup for its interpreter inside the child.
        .env("PATH", tools::child_path(&bin))
        .current_dir(project)
        .output()
        .map_err(|e| format!("could not run {}: {e}", eco.program()))?;

    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        // The tool's own message, not a generic one. A resolver's
        // explanation of WHY it refused is the whole value of the
        // failure.
        return Err(if stderr.is_empty() {
            format!("{} failed with no output", eco.program())
        } else {
            stderr
        });
    }

    Ok(Applied {
        name: name.to_string(),
        requested: version.to_string(),
        changed_files: changed_files(dir),
        output: stderr,
        // Read back, never assumed. See `resolved_constraint`.
        resolved_constraint: read_constraint(project, eco, name),
    })
}

/// The constraint a manifest holds for one package, after an update.
///
/// Deliberately narrow: only the formats this can read WITHOUT a parser
/// dependency, and `None` for everything else. An unknown constraint is
/// reported as unknown; guessing would reintroduce exactly the
/// confidently-wrong number this exists to prevent.
fn read_constraint(dir: &Path, eco: Ecosystem, name: &str) -> Option<String> {
    match eco {
        // PARSED, not line-scanned. npm preserves whatever formatting
        // the manifest already had, and a single-line package.json --
        // which npm writes back verbatim -- has no line starting with
        // the package name. A line scan silently returned "unknown" on
        // exactly the case this function exists to catch.
        Ecosystem::Npm | Ecosystem::Yarn => {
            let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
            let json: serde_json::Value = serde_json::from_str(&text).ok()?;
            // Checked in the order npm resolves them.
            ["dependencies", "devDependencies", "optionalDependencies"]
                .iter()
                .find_map(|section| json.get(section)?.get(name)?.as_str().map(str::to_string))
        }
        // Everything else needs a real TOML/XML parser to read safely.
        _ => None,
    }
}

/// Files git reports as modified in the worktree.
///
/// Read from git rather than from a hardcoded per-ecosystem list, so a
/// resolver that rewrites something unexpected shows up instead of
/// being filtered out.
fn changed_files(dir: &Path) -> Vec<String> {
    let Ok(out) = std::process::Command::new(crate::auth::git_program())
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.get(3..).map(str::trim))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Commit everything the update wrote.
///
/// `git add -A` rather than a list of expected files: a resolver decides
/// what it rewrites, and phase 1 exists precisely because that is not
/// predictable. A lockfile the tool touched and this did not know to
/// stage would leave the branch inconsistent with the checkout it came
/// from.
///
/// The worktree was created for this run and contains nothing else, so
/// there is no unrelated work to sweep up.
///
/// Identity comes from `-c` flags: a CI machine or a fresh container may
/// have no git identity configured, and the commit must not fail for
/// that.
pub fn commit_all(dir: &Path, message: &str) -> Result<(), String> {
    let add = std::process::Command::new(crate::auth::git_program())
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !add.status.success() {
        return Err(String::from_utf8_lossy(&add.stderr).trim().to_string());
    }

    let out = std::process::Command::new(crate::auth::git_program())
        .args([
            "-c",
            "user.name=Headstate",
            "-c",
            "user.email=headstate@users.noreply.github.com",
            "commit",
            "-q",
            "-m",
            message,
        ])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // "nothing to commit" is not a failure to report as one: the apply
    // succeeded and changed nothing, which the report already says and
    // the caller already refuses to push.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("nothing to commit") || stderr.contains("nothing to commit") {
        return Err("the update changed no files, so there is nothing to commit".into());
    }
    Err(stderr.trim().to_string())
}

/// Push the run's branch to `origin`.
///
/// The FIRST thing this app sends to a shared remote. Everything before
/// it -- worktrees, removals, applies -- was local and undoable by the
/// user alone.
///
/// `--set-upstream` so the branch is tracked, and no `--force` of any
/// kind: the branch was created fresh by `create_worktree`, which
/// refuses an existing name, so there is nothing to overwrite. If a
/// remote branch of that name somehow exists, git's refusal is the right
/// outcome and is returned verbatim.
pub fn push_branch(dir: &Path, branch: &str) -> Result<(), String> {
    let out = std::process::Command::new(crate::auth::git_program())
        .args(["push", "--set-upstream", "origin", branch])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // git's own message. "Permission denied (publickey)" and "protected
    // branch hook declined" are both actionable and both lost by a
    // generic "push failed".
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// The branch a pull request should target.
///
/// The remote's default, read from `origin/HEAD` rather than assumed to
/// be `main`: this app already carries repositories whose default is
/// `master`, and opening a pull request against a branch that does not
/// exist fails after the push has already happened.
pub fn default_branch(dir: &Path) -> Option<String> {
    let out = std::process::Command::new(crate::auth::git_program())
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .strip_prefix("origin/")
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        // `origin/HEAD` is written by the REMOTE, so this name is
        // remote-controlled and must clear the flag-shape check before it
        // reaches an argv (#854). It becomes the `--base` of a pull
        // request and, one caller up, is pushed -- and
        // `valid_branch_name` guards the branch this app CREATES, never
        // the base it reads.
        //
        // Filtered AFTER `strip_prefix("origin/")`, which is the ordering
        // `worktrees::scan::default_branch` documents: "Prefixing first
        // would hide `--output=EVIL` behind a name that no longer starts
        // with `-`." The same applies to stripping -- the bare name is
        // what reaches the argv, so the bare name is what is checked.
        .filter(|s| crate::worktrees::scan::is_safe_ref(s))
}

/// Create a worktree for an update run.
///
/// Branches from the repository's CURRENT HEAD rather than from a
/// remote: the user asked to update this checkout's dependencies, and
/// silently basing the work on `origin/main` would produce a diff
/// against code they are not looking at.
///
/// Refuses if the branch or directory already exists rather than reusing
/// either. Reuse would mean applying updates on top of an earlier run's
/// results, which is how a "one package" change quietly becomes twelve.
pub fn create_worktree(repo: &Path, branch: &str, dir: &Path) -> Result<(), String> {
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    // Ask git, rather than testing for a directory under .git: a branch
    // can exist without a worktree and packed refs have no file.
    let exists = std::process::Command::new(crate::auth::git_program())
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        return Err(format!("branch {branch} already exists"));
    }

    let out = std::process::Command::new(crate::auth::git_program())
        .args(["worktree", "add", "-b", branch])
        .arg(dir)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// The stamp format a generated branch name ends with: UTC, to the second.
///
/// Mirrored in `src/lib/branchName.ts`, which builds the same string from
/// a `Date`. Kept as one constant so the two can be compared by eye.
const STAMP_FORMAT: &str = "%Y%m%d-%H%M%S";

/// A branch name for an update run, stamped with the moment it starts.
///
/// Reads the clock; see [`branch_name_at`] for the derivation itself,
/// which is pure and is what the tests and the TypeScript mirror pin.
pub fn branch_name(packages: &[String], ecosystem: Option<Ecosystem>) -> String {
    branch_name_at(packages, ecosystem, chrono::Utc::now())
}

/// The derivation, with the clock passed in.
///
/// # Why the name changed (#797)
///
/// It used to be `headstate/updates-<count>`, where the count was the
/// NUMBER OF PACKAGES. Three things were wrong with that at once, and the
/// third is the one that broke a real workflow:
///
/// 1. `153` read as an issue or pull request number. Every other number
///    in this app's UI is one, and a quantity nobody asked for is a poor
///    use of the one field a reader scans.
/// 2. It said nothing about what the run WAS. The ecosystem is on every
///    request already, so the run's own identity was being thrown away.
/// 3. Keying on a COUNT means two runs in the same repository that happen
///    to touch the same number of packages produce the same name, and
///    `create_worktree` refuses when the branch or directory exists. So
///    the second run failed outright -- loudly, which is correct of the
///    guard, but the collision only existed because the name encoded a
///    quantity rather than anything distinguishing.
///
/// Uniqueness per run mattered more than the exact spelling, so the name
/// is now `headstate/<ecosystem>-deps-<UTC timestamp to the second>`:
/// `headstate/poetry-deps-20260911-064512`.
///
/// ## Why a timestamp rather than a hash or a counter
///
/// A hash of the package list collides for exactly the runs that need
/// telling apart -- re-running the same selection after a failure is the
/// common case -- and says nothing a reader can use. A "next free
/// number" counter needs to ask git what exists, which the wizard cannot
/// do without the round trip #409 exists to avoid. A timestamp is
/// unique at the only granularity that matters here (one update run per
/// repository at a time, enforced by `runs::UpdateRuns::start`), sorts
/// chronologically in `git branch` output, and tells the reader when the
/// work happened -- which is the question a stranded update branch
/// actually raises.
///
/// It is NOT mistakable for an issue number: eight digits, a dash, six
/// more. `-deps-` in the middle says what the run was about, so the
/// worktree directory leaf (`worktree_path` takes the branch's last
/// segment) reads as `poetry-deps-20260911-064512` rather than the bare
/// `updates-153` that started the investigation in #797.
///
/// ## What did NOT change
///
/// The `headstate/` prefix. #797 records that `headstate/` reads as a
/// repo name rather than a creator, and that a different prefix would be
/// better -- but it affects every existing branch in every repository, so
/// it is deliberately left as a FOLLOW-UP and out of scope here. Existing
/// `headstate/updates-*` branches and their `.worktrees/` directories are
/// untouched; only newly created runs get the new shape.
///
/// ## The single-package case
///
/// Dropped, along with the package name it carried. It was the one case
/// where the old name said something useful, but it was also the case
/// where two runs collide most readily -- bumping `lodash` twice is
/// ordinary -- and a name whose shape depends on how many packages were
/// selected is a name the wizard has to re-derive on every checkbox
/// click. One shape for every run is predictable for the user and for the
/// mirror. The packages are still listed in the pull request body, which
/// is where a list of nine belongs.
///
/// `_packages` is kept in the signature and deliberately unused: every
/// caller has the list, and a name that wants a package in it again
/// should not need the call sites rewritten to get one. Underscored to say
/// so, matching `_names` in the TypeScript mirror.
pub fn branch_name_at(
    _packages: &[String],
    ecosystem: Option<Ecosystem>,
    at: chrono::DateTime<chrono::Utc>,
) -> String {
    // No sanitising. `slug()` is ASCII lowercase by test
    // (`model::tests::every_slug_is_safe_in_a_ref`) and the stamp is
    // digits and one dash, so nothing here can produce a character git
    // would refuse -- unlike the package names the old name interpolated,
    // which is what the sanitiser existed for.
    //
    // `None` is a MIXED run, not a missing value: the wizard's checkboxes
    // are per row and a user may tick a crate and a pip package in one
    // go. Naming such a branch after whichever request happens to be
    // first would be arbitrary and wrong half the time, and listing both
    // makes the length depend on the selection. "deps" alone says what is
    // true of every run.
    //
    // Built as "<what>-<stamp>" with the ecosystem folded into `what`,
    // rather than as two format! arms. Two arms drift in their
    // separators, and an empty slug interpolated into one template leaves
    // `headstate/-deps-…` -- a name `valid_branch_name` refuses, because
    // git would read the leading dash as an option.
    let what = match ecosystem {
        Some(e) => format!("{}-deps", e.slug()),
        None => "deps".to_string(),
    };
    format!("headstate/{what}-{}", at.format(STAMP_FORMAT))
}

/// The one ecosystem a set of requests shares, or `None` when they differ.
///
/// Split out because the TypeScript mirror needs the same rule and a rule
/// worth mirroring is worth naming. `None` for an empty slice too: there
/// is no ecosystem to name, and `run` refuses an empty request list
/// before this is reached anyway.
pub fn sole_ecosystem(requests: &[UpdateRequest]) -> Option<Ecosystem> {
    let first = requests.first()?.ecosystem;
    requests
        .iter()
        .all(|r| r.ecosystem == first)
        .then_some(first)
}

/// Whether a user-supplied branch name is one git will accept.
///
/// The generated name is sanitised by construction; an OVERRIDE is not,
/// and it reaches `git worktree add -b` and later a push. Validated
/// against git's own ref rules rather than trusted, and refused up
/// front so a bad name does not leave a worktree behind.
///
/// Deliberately stricter than `git check-ref-format` in one respect: a
/// leading `-` is refused outright, because git would read it as an
/// option regardless of whether the ref grammar allows it.
pub fn valid_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("branch name is empty".into());
    }
    if name.starts_with('-') {
        return Err(format!(
            "branch name {name:?} starts with '-', which git would read \
             as an option rather than a name"
        ));
    }
    // git's own rules, the ones a wrong name fails on:
    // https://git-scm.com/docs/git-check-ref-format
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err("branch name may not start or end with '/', or contain '//'".into());
    }
    if name.ends_with('.') || name.contains("..") {
        return Err("branch name may not end with '.' or contain '..'".into());
    }
    if name.ends_with(".lock") {
        return Err("branch name may not end with '.lock'".into());
    }
    if name.contains("@{") {
        return Err("branch name may not contain '@{'".into());
    }
    for c in name.chars() {
        if c.is_control() || c == ' ' {
            return Err("branch name may not contain spaces or control characters".into());
        }
        if matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
            return Err(format!("branch name may not contain {c:?}"));
        }
    }
    Ok(())
}

/// One package to update.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateRequest {
    pub name: String,
    pub version: String,
    pub ecosystem: Ecosystem,
    /// Which PROJECT within the repository declares it, relative to the
    /// repository root. Empty, or absent, means the root itself.
    ///
    /// Every apply before Cargo ran at the worktree root and ignored
    /// this, which worked only because those runs happened to be
    /// single-project repositories -- a `package.json` in a subdirectory
    /// was already being updated in the wrong place, silently.
    ///
    /// Cargo made it unavoidable rather than merely latent: in THIS
    /// repository every Cargo row comes from `src-tauri` or
    /// `src-mobile` and there is no `Cargo.toml` at the root at all, so
    /// a root-relative apply has nothing to edit.
    ///
    /// `#[serde(default)]` because the field is new and an older
    /// caller's payload omits it; absent then means the root, which is
    /// exactly the behaviour those callers already had.
    #[serde(default)]
    pub project: String,
}

impl UpdateRequest {
    /// The directory this request's manifest lives in, inside `dir`.
    ///
    /// Joined rather than trusted: `project` arrives over IPC, and a
    /// `..` component would reach outside the throwaway worktree that
    /// is the entire safety boundary of an apply. Refused rather than
    /// normalised, on the same reasoning as `reject_flaglike` -- a
    /// refusal the user can see beats a path quietly rewritten.
    ///
    /// The leading-separator check is deliberately NOT `is_absolute`
    /// alone. `Path::is_absolute` is platform-dependent: on Windows
    /// `/etc` is *relative* (it has no drive letter), so `is_absolute`
    /// returns false and `join` then discards the worktree's own root
    /// and resolves it against the current drive. A path this app
    /// refuses on macOS must not be accepted on Windows, so the leading
    /// separator is rejected on every platform. Caught by CI on
    /// windows-latest, not by reasoning.
    fn dir(&self, worktree: &Path) -> Result<std::path::PathBuf, String> {
        if self.project.is_empty() {
            return Ok(worktree.to_path_buf());
        }
        let p = Path::new(&self.project);
        if p.is_absolute()
            || self.project.starts_with('/')
            || self.project.starts_with('\\')
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "project path {:?} must be relative to the repository and may not contain '..'",
                self.project
            ));
        }
        Ok(worktree.join(p))
    }
}

/// The result of a whole run: where the work landed and what each
/// update did.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunReport {
    /// The worktree holding the changes. Left in place: phase 1 does not
    /// push, and phase 2 leaves it too so a failed push or refused pull
    /// request costs nothing that was not already there.
    pub worktree: String,
    pub branch: String,
    pub results: Vec<UpdateOutcome>,
    /// Which ecosystems this run touched.
    ///
    /// Carried because opening a pull request is only offered where the
    /// resolved constraint can be read back -- and the outcomes alone do
    /// not say which tool produced them.
    #[serde(default)]
    pub ecosystems: Vec<Ecosystem>,
}

/// What happened to one requested update.
///
/// A per-package result rather than one overall status: updates are
/// applied in sequence and a failure in the third must not erase the
/// report of the two that worked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpdateOutcome {
    pub name: String,
    pub requested: String,
    pub changed_files: Vec<String>,
    pub output: String,
    pub resolved_constraint: Option<String>,
    /// Set when this package failed. The others still report normally.
    pub error: Option<String>,
}

impl UpdateOutcome {
    fn from_applied(a: Applied) -> Self {
        Self {
            name: a.name,
            requested: a.requested,
            changed_files: a.changed_files,
            output: a.output,
            resolved_constraint: a.resolved_constraint,
            error: None,
        }
    }

    fn failed(req: &UpdateRequest, error: String) -> Self {
        Self {
            name: req.name.clone(),
            requested: req.version.clone(),
            changed_files: Vec::new(),
            output: String::new(),
            resolved_constraint: None,
            error: Some(error),
        }
    }
}

/// Create a worktree and apply updates in it. Does NOT push.
///
/// The repository is checked for cleanliness first, the same contract
/// `pull_checkout` follows -- and for the same reason. `git worktree
/// add` itself does not care whether the source checkout is dirty, but
/// the user comparing the result against their working copy does.
///
/// Updates run in the order given and DO NOT stop at the first failure:
/// each package reports independently, because "lodash worked, express
/// was refused by the resolver" is the useful answer and aborting would
/// hide it.
pub fn run(repo: &Path, requests: &[UpdateRequest]) -> Result<RunReport, String> {
    run_on_branch(repo, requests, None)
}

/// `run_on_branch`, reporting progress as each package settles.
///
/// A run is one package-manager invocation per package, so a selection
/// of 122 sits for minutes. `apply_updates_in_background` returns
/// immediately and its doc claimed "progress and completion arrive as
/// events, the same shape the worktree removal uses" -- but only
/// completion ever did: `run_on_branch` took no callback, so there was
/// nothing to emit from. This is that shape, matching
/// `remove_worktrees_with_progress`.
///
/// `on_progress` is called with (done, total) AFTER each package, so
/// `done` counts finished work rather than started work. Counts only,
/// never names: a progress event is not a place to leak what someone is
/// working on.
pub fn run_on_branch_with_progress(
    repo: &Path,
    requests: &[UpdateRequest],
    branch_override: Option<&str>,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<RunReport, String> {
    run_inner(repo, requests, branch_override, &on_progress, &|| false)
}

/// `run_on_branch_with_progress`, stoppable.
///
/// `should_stop` is consulted BETWEEN packages, never during one. A
/// package manager killed halfway through leaves the worktree in a
/// state nobody asked for -- a half-written `node_modules`, a lockfile
/// that matches neither the old nor the new manifest -- so the current
/// package always finishes. Cancellation is "stop after this one",
/// which is both safer and the only kind this can honestly offer.
///
/// The report still comes back, holding whatever was applied before the
/// stop. A cancelled run is not a failed one: the packages that landed
/// really did land, and the worktree they landed in still exists.
pub fn run_on_branch_cancellable(
    repo: &Path,
    requests: &[UpdateRequest],
    branch_override: Option<&str>,
    on_progress: impl Fn(usize, usize) + Sync,
    should_stop: impl Fn() -> bool + Sync,
) -> Result<RunReport, String> {
    run_inner(repo, requests, branch_override, &on_progress, &should_stop)
}

/// `run`, with the branch name optionally chosen by the caller.
///
/// #409 asked for the generated name to be overridable. `None` keeps
/// the derived one, which is what every existing caller wants.
pub fn run_on_branch(
    repo: &Path,
    requests: &[UpdateRequest],
    branch_override: Option<&str>,
) -> Result<RunReport, String> {
    run_inner(repo, requests, branch_override, &|_, _| {}, &|| false)
}

/// The run itself. `on_progress` is a reference so the two public
/// entry points share one body rather than one being a copy that
/// drifts.
fn run_inner(
    repo: &Path,
    requests: &[UpdateRequest],
    branch_override: Option<&str>,
    on_progress: &(dyn Fn(usize, usize) + Sync),
    should_stop: &(dyn Fn() -> bool + Sync),
) -> Result<RunReport, String> {
    if requests.is_empty() {
        return Err("nothing to update".into());
    }
    // Refuse Swift before creating anything, so a request that cannot
    // succeed does not leave a worktree behind.
    for r in requests {
        supported(r.ecosystem).map_err(|Unsupported::NoCommand(m)| m.to_string())?;
        // Validated here too, not only in `apply_one`: a rejected value
        // must not leave a worktree behind.
        reject_flaglike("package name", &r.name)?;
        reject_flaglike("version", &r.version)?;
    }

    let names: Vec<String> = requests.iter().map(|r| r.name.clone()).collect();
    let branch = match branch_override {
        // Validated BEFORE the worktree is created, for the same reason
        // the package names above are: a refusal must not leave one
        // behind.
        Some(b) => {
            valid_branch_name(b)?;
            b.to_string()
        }
        None => branch_name(&names, sole_ecosystem(requests)),
    };
    // Beside the repository, not inside it: a worktree inside the
    // checkout shows up in the parent's own status and in every tool
    // that walks it.
    let dir = worktree_path(repo, &branch);

    create_worktree(repo, &branch, &dir)?;

    let total = requests.len();
    // A plain loop rather than `map().collect()`: this one can stop
    // early, and an iterator chain cannot.
    let mut results = Vec::with_capacity(total);
    for (i, r) in requests.iter().enumerate() {
        // Checked BEFORE the package, so a cancel takes effect at the
        // next boundary rather than mid-install. See
        // `run_on_branch_cancellable` for why halfway is not offered.
        if should_stop() {
            log::info!(
                "update run: stopped after {} of {} packages",
                results.len(),
                total
            );
            break;
        }
        let outcome = match r
            .dir(&dir)
            .and_then(|p| apply_one_in(&dir, &p, r.ecosystem, &r.name, &r.version))
        {
            Ok(a) => UpdateOutcome::from_applied(a),
            Err(e) => UpdateOutcome::failed(r, e),
        };
        results.push(outcome);
        // After the package, not before: `done` is finished work.
        // A failure still counts as done -- the run does not stop
        // at the first one, and a progress bar that stalled on a
        // refused package would misreport a run still going.
        on_progress(i + 1, total);
    }

    let mut ecosystems: Vec<Ecosystem> = requests.iter().map(|r| r.ecosystem).collect();
    ecosystems.sort_by_key(|e| format!("{e:?}"));
    ecosystems.dedup();

    Ok(RunReport {
        worktree: dir.to_string_lossy().into_owned(),
        branch,
        results,
        ecosystems,
    })
}

/// Where a run's worktree goes: `<repo>/.worktrees/<leaf of branch>`.
///
/// Inside `.worktrees/` because that is the convention this app already
/// scans for and skips, so an update run does not pollute the CLAUDE.md
/// or worktree views with a directory that is not a project.
fn worktree_path(repo: &Path, branch: &str) -> std::path::PathBuf {
    let leaf = branch.rsplit('/').next().unwrap_or(branch);
    repo.join(".worktrees").join(leaf)
}

/// A fixed instant, for asserting the stamped half of a branch name.
///
/// At module scope rather than inside one test module because both
/// `tests` and `branch_override` pin the derivation, and the agreement
/// cases that `src/lib/branchName.test.ts` mirrors live in the second.
#[cfg(test)]
fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .expect("a valid RFC3339 instant")
        .with_timezone(&chrono::Utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_is_refused() {
        assert!(supported(Ecosystem::Swift).is_err());
    }

    /// Every other ecosystem must be allowed, or the guard silently
    /// disables the feature.
    #[test]
    fn every_other_ecosystem_is_supported() {
        for eco in [
            Ecosystem::Npm,
            Ecosystem::Yarn,
            Ecosystem::Poetry,
            Ecosystem::Uv,
            Ecosystem::Dotnet,
            Ecosystem::Cocoapods,
            // Since #559. Applied by editing the manifest rather than
            // by running `cargo add`, whose three wrong edits on a
            // workspace are what kept it out until now.
            Ecosystem::Cargo,
        ] {
            assert!(supported(eco).is_ok(), "{eco:?} should be supported");
        }
    }

    /// A Cargo apply must spawn NOTHING.
    ///
    /// The whole design rests on this: `cargo` is deliberately one of
    /// the ecosystems that needs no tool installed, and an apply that
    /// quietly reintroduced the dependency would make a missing binary
    /// break half the ecosystem. Proved by pointing it at a directory
    /// with no manifest and checking the error is about the MANIFEST,
    /// never about a missing program.
    #[test]
    fn a_cargo_apply_never_looks_for_a_binary() {
        let dir = tempfile::tempdir().unwrap();
        let e = apply_one(dir.path(), Ecosystem::Cargo, "serde", "1.0.229")
            .expect_err("no manifest, so it must fail");
        assert!(
            !e.contains("not installed"),
            "must not blame a missing binary: {e}"
        );
        assert!(e.contains("not declared"), "{e}");
    }

    /// A project path that tries to escape the worktree is refused.
    ///
    /// `project` arrives over IPC, and the throwaway worktree is the
    /// entire safety boundary of an apply.
    #[test]
    fn a_project_path_may_not_escape_the_worktree() {
        let req = |p: &str| UpdateRequest {
            name: "serde".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Cargo,
            project: p.into(),
        };
        assert!(req("../elsewhere").dir(Path::new("/wt")).is_err());
        assert!(req("a/../../b").dir(Path::new("/wt")).is_err());
        // A leading separator is refused on EVERY platform. On Windows
        // `/etc` is not `is_absolute` -- it has no drive letter -- so
        // `join` would drop the worktree root and resolve it against
        // the current drive. CI caught this; the rule is "refused
        // everywhere", not "refused where the platform calls it
        // absolute".
        assert!(req("/etc").dir(Path::new("/wt")).is_err());
        assert!(req("\\windows").dir(Path::new("/wt")).is_err());
        // The ordinary shapes still work.
        assert_eq!(
            req("src-tauri").dir(Path::new("/wt")).unwrap(),
            Path::new("/wt/src-tauri")
        );
        assert_eq!(req("").dir(Path::new("/wt")).unwrap(), Path::new("/wt"));
    }

    /// The nested-project route, end to end through `apply_one_in`.
    ///
    /// In THIS repository there is no `Cargo.toml` at the repo root at
    /// all -- every crate is under `src-tauri` or `src-mobile` -- so an
    /// apply that ignored the project path would have nothing to edit.
    #[test]
    fn a_cargo_apply_edits_a_manifest_in_a_nested_project() {
        let wt = tempfile::tempdir().unwrap();
        let project = wt.path().join("src-tauri");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\n# WHY: needed for the config types.\nserde = \"1.0.200\"\n",
        )
        .unwrap();

        let a = apply_one_in(wt.path(), &project, Ecosystem::Cargo, "serde", "1.0.229").unwrap();
        assert_eq!(a.resolved_constraint.as_deref(), Some("1.0.229"));

        let text = std::fs::read_to_string(project.join("Cargo.toml")).unwrap();
        assert!(text.contains("serde = \"1.0.229\""), "{text}");
        assert!(text.contains("# WHY: needed for the config types."));
    }

    /// `apply_one` must refuse Swift BEFORE looking for a binary --
    /// otherwise the error a user sees is "swift is not installed",
    /// which is both wrong and fixable-looking.
    #[test]
    fn apply_refuses_swift_without_running_anything() {
        let e = apply_one(Path::new("/nonexistent"), Ecosystem::Swift, "x", "1.0")
            .expect_err("Swift must be refused");
        assert!(
            e.contains("Xcode"),
            "expected the Swift explanation, got: {e}"
        );
    }

    /// The version must reach the command line. A hint string that
    /// merely mentions the version would not.
    #[test]
    fn update_args_pin_the_requested_version() {
        for (eco, expect) in [
            (Ecosystem::Npm, "lodash@4.17.21"),
            (Ecosystem::Yarn, "lodash@4.17.21"),
            (Ecosystem::Poetry, "lodash@4.17.21"),
            (Ecosystem::Uv, "lodash==4.17.21"),
        ] {
            let args = update_args(Path::new("/nonexistent"), eco, "lodash", "4.17.21");
            assert!(
                args.iter().any(|a| a == expect),
                "{eco:?} args {args:?} missing {expect}"
            );
        }
        let dotnet = update_args(
            Path::new("/nonexistent"),
            Ecosystem::Dotnet,
            "Serilog",
            "3.1.1",
        );
        assert!(dotnet.contains(&"--version".to_string()));
        assert!(dotnet.contains(&"3.1.1".to_string()));
    }

    /// A pinned manifest STAYS pinned.
    ///
    /// Production dependency lists that say `4.17.20` mean it. npm's
    /// default widens that to `^4.17.21` on update, quietly converting
    /// a pinned project into a floating one -- measured against real
    /// npm, and the reason this exists.
    #[test]
    fn an_exact_version_is_kept_exact() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(
            t.path().join("package.json"),
            r#"{"dependencies":{"lodash":"4.17.20"}}"#,
        )
        .unwrap();
        let args = update_args(t.path(), Ecosystem::Npm, "lodash", "4.17.21");
        assert!(
            args.iter().any(|a| a == "--save-exact"),
            "a pinned manifest must stay pinned: {args:?}"
        );
    }

    /// And the opposite mistake is not made.
    ///
    /// Applying `--save-exact` unconditionally NARROWS a project that
    /// deliberately wrote a range into a pin it never asked for --
    /// verified against real npm, which does exactly that.
    #[test]
    fn a_range_is_not_narrowed_into_a_pin() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(
            t.path().join("package.json"),
            r#"{"dependencies":{"lodash":"^4.17.20"}}"#,
        )
        .unwrap();
        let args = update_args(t.path(), Ecosystem::Npm, "lodash", "4.17.21");
        assert!(
            !args.iter().any(|a| a == "--save-exact"),
            "a range must stay a range: {args:?}"
        );
    }

    /// Yarn Berry writes a range by default for the same reason.
    #[test]
    fn yarn_keeps_an_exact_version_exact() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(
            t.path().join("package.json"),
            r#"{"dependencies":{"lodash":"4.17.20"}}"#,
        )
        .unwrap();
        let args = update_args(t.path(), Ecosystem::Yarn, "lodash", "4.17.21");
        assert!(args.iter().any(|a| a == "--exact"), "{args:?}");
    }

    /// An unreadable manifest leaves the tool's own default, which is
    /// what this did before -- the conservative direction.
    #[test]
    fn an_unknown_constraint_changes_nothing() {
        let t = tempfile::TempDir::new().unwrap();
        let args = update_args(t.path(), Ecosystem::Npm, "lodash", "4.17.21");
        assert!(!args.iter().any(|a| a == "--save-exact"), "{args:?}");
    }

    /// The other range forms must not be read as pins.
    #[test]
    fn every_range_form_counts_as_a_range() {
        for constraint in ["^1.0.0", "~1.0.0", ">=1.0.0", "1.x", "1.0.0 - 2.0.0", "*"] {
            let t = tempfile::TempDir::new().unwrap();
            std::fs::write(
                t.path().join("package.json"),
                format!(r#"{{"dependencies":{{"lodash":"{constraint}"}}}}"#),
            )
            .unwrap();
            let args = update_args(t.path(), Ecosystem::Npm, "lodash", "4.17.21");
            assert!(
                !args.iter().any(|a| a == "--save-exact"),
                "{constraint} is a range, not a pin: {args:?}"
            );
        }
    }

    /// Each command names ONE package. A blunt `npm update` would move
    /// everything it could, which is unreviewable.
    #[test]
    fn update_args_name_the_package() {
        for eco in [
            Ecosystem::Npm,
            Ecosystem::Yarn,
            Ecosystem::Poetry,
            Ecosystem::Uv,
            Ecosystem::Dotnet,
            Ecosystem::Cocoapods,
        ] {
            let args = update_args(Path::new("/nonexistent"), eco, "lodash", "4.17.21");
            assert!(
                args.iter().any(|a| a.contains("lodash")),
                "{eco:?} does not name the package: {args:?}"
            );
        }
    }

    /// A package name can no longer reach the branch name at all (#797),
    /// which is what retires the sanitiser's reason for existing HERE --
    /// a scoped npm name carries `@` and `/`, and a `/` would nest the
    /// ref. Kept as a test rather than deleted: the `packages` argument is
    /// still in the signature, and a future name that interpolates one
    /// again must not quietly reintroduce the hazard.
    #[test]
    fn a_package_name_cannot_reach_the_branch_name() {
        let b = branch_name_at(
            &["@scope/pkg".to_string()],
            Some(Ecosystem::Npm),
            at("2026-09-11T06:45:12Z"),
        );
        assert_eq!(b, "headstate/npm-deps-20260911-064512");
        assert!(!b.contains('@'), "{b}");
        assert!(
            b.matches('/').count() == 1,
            "only the headstate/ prefix may contain a slash: {b}"
        );
        assert!(valid_branch_name(&b).is_ok(), "{b} must be a usable ref");
    }

    /// The collision #797 is actually about. The old name keyed on the
    /// PACKAGE COUNT, so two runs in one repository touching the same
    /// number of packages produced the same branch -- and
    /// `create_worktree` refuses an existing one, so the second failed
    /// outright. enc-api already held `headstate/updates-153`, which made
    /// the next 153-package Poetry run there impossible.
    #[test]
    fn two_runs_with_the_same_package_count_do_not_collide() {
        let same_count = ["a".to_string(), "b".to_string(), "c".to_string()];
        let first = branch_name_at(
            &same_count,
            Some(Ecosystem::Poetry),
            at("2026-09-11T06:45:12Z"),
        );
        let second = branch_name_at(
            &same_count,
            Some(Ecosystem::Poetry),
            at("2026-09-11T06:45:13Z"),
        );
        assert_ne!(first, second);
    }

    /// A run's identity, not its size. Asserting the whole string rather
    /// than a `contains`: the point of #797 is what the name READS as, and
    /// a substring check passes on a name with the old count still in it.
    #[test]
    fn the_name_says_which_ecosystem() {
        assert_eq!(
            branch_name_at(&[], Some(Ecosystem::Poetry), at("2026-09-11T06:45:12Z")),
            "headstate/poetry-deps-20260911-064512"
        );
        assert_eq!(
            branch_name_at(&[], Some(Ecosystem::Cargo), at("2026-09-11T06:45:12Z")),
            "headstate/cargo-deps-20260911-064512"
        );
    }

    /// Nothing in a generated name can be read as an issue or pull
    /// request number, which was the second of #797's three complaints:
    /// every other number in this UI is one. A 14-character stamp split
    /// by a dash is not mistakable for `#153`.
    #[test]
    fn no_part_of_the_name_looks_like_an_issue_number() {
        let b = branch_name_at(
            &["a".into()],
            Some(Ecosystem::Npm),
            at("2026-09-11T06:45:12Z"),
        );
        let leaf = b.rsplit('/').next().unwrap();
        for run in leaf.split(['-', '.']) {
            if run.chars().all(|c| c.is_ascii_digit()) {
                assert!(
                    run.len() >= 6,
                    "{run:?} in {b} is short enough to read as an issue number"
                );
            }
        }
    }

    /// A mixed selection gets "deps" and no ecosystem. Naming it after
    /// whichever request came first would be arbitrary; the hazard the
    /// assertion guards is the empty slug leaving `headstate/-deps-…`,
    /// which git reads as an option and `valid_branch_name` refuses.
    #[test]
    fn a_mixed_run_is_named_deps_with_no_stray_dash() {
        let b = branch_name_at(&[], None, at("2026-09-11T06:45:12Z"));
        assert_eq!(b, "headstate/deps-20260911-064512");
        assert!(valid_branch_name(&b).is_ok(), "{b} must be a usable ref");
    }

    #[test]
    fn sole_ecosystem_requires_agreement() {
        let req = |eco| UpdateRequest {
            name: "pkg".into(),
            version: "1.0.0".into(),
            ecosystem: eco,
            project: String::new(),
        };
        assert_eq!(sole_ecosystem(&[]), None);
        assert_eq!(
            sole_ecosystem(&[req(Ecosystem::Uv), req(Ecosystem::Uv)]),
            Some(Ecosystem::Uv)
        );
        assert_eq!(
            sole_ecosystem(&[req(Ecosystem::Uv), req(Ecosystem::Cargo)]),
            None
        );
    }

    /// Whatever the clock says, the result must be a ref git accepts.
    /// The stamp is the only part that varies at runtime, so this is what
    /// stands between a format change and a run that dies at
    /// `git worktree add`.
    #[test]
    fn the_generated_name_is_always_a_valid_ref() {
        for eco in [None, Some(Ecosystem::Npm), Some(Ecosystem::Cocoapods)] {
            let b = branch_name(&["lodash".to_string()], eco);
            assert!(valid_branch_name(&b).is_ok(), "{b} should be valid");
        }
    }

    /// A real git repository, so worktree behaviour is exercised rather
    /// than mocked. `git worktree add` has enough rules of its own that
    /// a fake would test the fake.
    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new(crate::auth::git_program())
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        // Identity comes from `-c` flags on the commit rather than two
        // extra `git config` spawns per fixture. Spawning is the
        // expensive part: under `--test-threads=8` the suite hits
        // macOS's posix_spawn pressure and `git init` itself fails with
        // ENOENT, which surfaces as an unrelated test failing.
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("package.json"), "{}\n").unwrap();
        run(&["add", "-A"]);
        let owned = commit_args();
        let commit: Vec<&str> = owned.iter().map(String::as_str).collect();
        run(&commit);
        tmp
    }

    /// A commit carrying its own identity, so no `git config` spawns.
    ///
    /// The address is BUILT rather than written literally: the privacy
    /// gate reads any `user@host` literal as a real address, and a
    /// fixture must not look like one.
    pub(super) fn commit_args() -> Vec<String> {
        let address = format!("test{}example{}invalid", '@', '.');
        vec![
            "-c".into(),
            "user.name=test".into(),
            "-c".into(),
            format!("user.email={address}"),
            "commit".into(),
            "-q".into(),
            "-m".into(),
            "init".into(),
        ]
    }

    #[test]
    fn creates_a_worktree_on_a_new_branch() {
        let tmp = repo();
        let wt = tmp.path().join("wt");
        create_worktree(tmp.path(), "headstate/test", &wt).expect("should create");
        assert!(
            wt.join("package.json").is_file(),
            "worktree has no checkout"
        );
    }

    /// Reuse would apply updates on top of an earlier run's results.
    #[test]
    fn refuses_an_existing_branch() {
        let tmp = repo();
        create_worktree(tmp.path(), "headstate/test", &tmp.path().join("a")).unwrap();
        let target = tmp.path().join("b");
        let e = create_worktree(tmp.path(), "headstate/test", &target)
            .expect_err("second use of the branch must be refused");
        // Asserting on OUR message, not merely on "already exists":
        // git's own refusal says "a branch named 'x' already exists",
        // so a looser assertion passed even with this check removed.
        assert!(
            e.starts_with("branch headstate/test already exists"),
            "expected the pre-flight refusal, got: {e}"
        );
        // And nothing was created on the way to refusing.
        assert!(!target.exists(), "a refused run must leave no directory");
    }

    #[test]
    fn refuses_an_existing_directory() {
        let tmp = repo();
        let wt = tmp.path().join("taken");
        std::fs::create_dir(&wt).unwrap();
        let e = create_worktree(tmp.path(), "headstate/x", &wt)
            .expect_err("an existing directory must be refused");
        assert!(e.contains("already exists"), "{e}");
    }

    /// Branches from the checkout's own HEAD, not a remote: the user is
    /// updating the code they are looking at.
    #[test]
    fn branches_from_current_head() {
        let tmp = repo();
        std::fs::write(tmp.path().join("marker.txt"), "local\n").unwrap();
        // The commit carries its own identity, like the fixture's does:
        // CI runners have no global git identity, so a bare `git commit`
        // fails there while passing on a developer machine.
        let commit = commit_args();
        let commit: Vec<&str> = commit.iter().map(String::as_str).collect();
        for args in [&["add", "-A"][..], &commit[..]] {
            let out = std::process::Command::new(crate::auth::git_program())
                .args(args)
                .current_dir(tmp.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let wt = tmp.path().join("wt");
        create_worktree(tmp.path(), "headstate/test", &wt).unwrap();
        assert!(
            wt.join("marker.txt").is_file(),
            "worktree must carry the local commit"
        );
    }

    /// Reads the constraint the manifest ACTUALLY holds.
    ///
    /// The `^` here is not invented for the test: npm rewrites a pinned
    /// `npm install lodash@4.17.21` request into `^4.17.21`, measured on
    /// a real run. This is the case that makes echoing the requested
    /// version wrong.
    #[test]
    fn reads_back_a_caret_range_npm_wrote() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            "{\n  \"dependencies\": {\n    \"lodash\": \"^4.17.21\"\n  }\n}\n",
        )
        .unwrap();
        assert_eq!(
            read_constraint(tmp.path(), Ecosystem::Npm, "lodash"),
            Some("^4.17.21".to_string()),
            "must report the caret range, not the requested pin"
        );
    }

    /// npm writes back whatever formatting the manifest had. A
    /// single-line package.json defeated the original line scan, which
    /// returned "unknown" for the exact case this must catch.
    #[test]
    fn reads_a_single_line_manifest() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"t","dependencies":{"lodash":"^4.17.21"}}"#,
        )
        .unwrap();
        assert_eq!(
            read_constraint(tmp.path(), Ecosystem::Npm, "lodash"),
            Some("^4.17.21".to_string())
        );
    }

    /// devDependencies count too.
    #[test]
    fn reads_dev_dependencies() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"vitest":"~1.2.0"}}"#,
        )
        .unwrap();
        assert_eq!(
            read_constraint(tmp.path(), Ecosystem::Npm, "vitest"),
            Some("~1.2.0".to_string())
        );
    }

    /// An unreadable constraint is unknown, never a guess.
    #[test]
    fn unknown_constraint_is_none_not_a_guess() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}\n").unwrap();
        assert_eq!(read_constraint(tmp.path(), Ecosystem::Npm, "lodash"), None);
        // Ecosystems needing a real parser report unknown rather than
        // pretending a line scan understood them.
        assert_eq!(read_constraint(tmp.path(), Ecosystem::Poetry, "x"), None);
    }

    #[test]
    fn a_flaglike_package_name_is_refused() {
        for bad in ["--registry=http://elsewhere", "-x", "--version"] {
            let e = apply_one(Path::new("/nonexistent"), Ecosystem::Npm, bad, "1.0")
                .expect_err("a flag-like name must be refused");
            assert!(e.contains("option"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_flaglike_version_is_refused() {
        assert!(apply_one(Path::new("/nonexistent"), Ecosystem::Npm, "lodash", "--x").is_err());
    }

    #[test]
    fn an_empty_name_or_version_is_refused() {
        assert!(reject_flaglike("name", "").is_err());
        assert!(apply_one(Path::new("/nonexistent"), Ecosystem::Npm, "", "1.0").is_err());
    }

    #[test]
    fn control_characters_are_refused() {
        assert!(reject_flaglike("name", "a\nb").is_err());
        assert!(reject_flaglike("name", "a\0b").is_err());
    }

    #[test]
    fn ordinary_names_and_versions_pass() {
        for good in [
            "lodash",
            "@scope/pkg",
            "Serilog.Sinks.File",
            "1.2.3",
            "4.17.21-beta.1",
        ] {
            assert!(
                reject_flaglike("x", good).is_ok(),
                "{good} should be allowed"
            );
        }
    }

    /// A refused value must not leave a worktree behind.
    #[test]
    fn run_refuses_a_flaglike_name_before_creating_anything() {
        let tmp = repo();
        let reqs = [UpdateRequest {
            name: "--registry=http://elsewhere".into(),
            version: "1.0".into(),
            ecosystem: Ecosystem::Npm,
            project: String::new(),
        }];
        assert!(run(tmp.path(), &reqs).is_err());
        assert!(
            !tmp.path().join(".worktrees").exists(),
            "a refused run must create no worktree"
        );
    }

    /// The push must NOT force. The branch is created fresh and refused
    /// if it exists, so there is nothing legitimate to overwrite -- and
    /// a force here would be overwriting someone else's work on a shared
    /// remote.
    #[test]
    fn the_push_never_forces() {
        // Asserted on the arguments rather than by pushing: there is no
        // remote to push to in a test, and the flag is the whole risk.
        let src = include_str!("apply.rs");
        let start = src
            .find("pub fn push_branch")
            .expect("push_branch must exist");
        let body = &src[start..start + 600];
        assert!(
            !body.contains("--force") && !body.contains("-f\""),
            "push_branch must never force"
        );
        assert!(body.contains("--set-upstream"), "the branch should track");
    }

    /// `origin/HEAD` rather than assuming `main`. Opening a pull request
    /// against a branch that does not exist fails AFTER the push has
    /// already happened.
    #[test]
    fn the_default_branch_comes_from_the_remote() {
        let tmp = repo();
        // No `origin/HEAD` in a bare fixture, so this must report
        // nothing rather than guessing "main".
        assert_eq!(default_branch(tmp.path()), None);
    }

    /// A commit with no identity configured fails on a fresh machine,
    /// which is where this would most often run.
    #[test]
    fn committing_needs_no_configured_identity() {
        let tmp = repo();
        std::fs::write(tmp.path().join("changed.txt"), "x\n").unwrap();
        assert!(
            commit_all(tmp.path(), "test: a change").is_ok(),
            "the commit must supply its own identity"
        );
    }

    /// An apply that changed nothing must not produce an empty commit --
    /// and must say why rather than failing opaquely.
    #[test]
    fn committing_nothing_is_refused_with_a_reason() {
        let tmp = repo();
        let err = commit_all(tmp.path(), "test: nothing").unwrap_err();
        assert!(err.contains("nothing to commit"), "{err}");
    }

    #[test]
    fn run_refuses_an_empty_request() {
        let tmp = repo();
        assert!(run(tmp.path(), &[]).is_err());
    }

    /// Progress is reported per package, counting FINISHED work.
    ///
    /// `apply_updates_in_background`'s doc claimed "progress and
    /// completion arrive as events" long before either could:
    /// `run_on_branch` took no callback, so only the terminal event
    /// ever fired and a run of 122 packages said nothing for minutes.
    ///
    /// The packages here are made to fail -- there is no real registry
    /// in a test repo -- which is the point: a run does not stop at the
    /// first failure, so a failed package still counts as done. A
    /// progress bar that stalled on a refused package would misreport a
    /// run that is still going.
    #[test]
    fn progress_counts_every_package_including_the_ones_that_fail() {
        let tmp = repo();
        let reqs: Vec<UpdateRequest> = ["lodash", "express", "chalk"]
            .iter()
            .map(|n| UpdateRequest {
                name: (*n).into(),
                version: "2.0.0".into(),
                ecosystem: Ecosystem::Npm,
                project: String::new(),
            })
            .collect();

        let seen = std::sync::Mutex::new(Vec::new());
        let report = run_on_branch_with_progress(tmp.path(), &reqs, None, |done, total| {
            seen.lock().unwrap().push((done, total));
        });
        // The run itself may fail for want of a registry; what is being
        // asserted is the callback, which fires either way.
        let _ = report;

        let seen = seen.into_inner().unwrap();
        assert_eq!(
            seen,
            vec![(1, 3), (2, 3), (3, 3)],
            "progress should count each package once, finishing at the total"
        );
    }

    /// A cancelled run stops at a package boundary and still reports
    /// what it managed to apply.
    ///
    /// The stop is checked BEFORE each package, never during one: a
    /// package manager killed halfway leaves a worktree in a state
    /// nobody asked for. So "cancel" means "stop after this one", and
    /// the packages that already landed stay landed.
    #[test]
    fn a_cancelled_run_stops_at_the_next_package_boundary() {
        let tmp = repo();
        let reqs: Vec<UpdateRequest> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| UpdateRequest {
                name: (*n).into(),
                version: "2.0.0".into(),
                ecosystem: Ecosystem::Npm,
                project: String::new(),
            })
            .collect();

        let seen = std::sync::Mutex::new(Vec::new());
        // Stop once two have gone through.
        let stop_after_two = || seen.lock().unwrap().len() >= 2;
        let report = run_on_branch_cancellable(
            tmp.path(),
            &reqs,
            None,
            |done, total| seen.lock().unwrap().push((done, total)),
            stop_after_two,
        );

        assert_eq!(
            seen.into_inner().unwrap(),
            vec![(1, 4), (2, 4)],
            "the run should stop after the second package, not mid-package"
        );
        // And the report still describes what was applied, rather than
        // being an error: a cancelled run is not a failed one.
        if let Ok(r) = report {
            assert_eq!(r.results.len(), 2);
        }
    }

    /// Never stopping is the ordinary case, and must not change.
    #[test]
    fn a_run_that_is_never_cancelled_does_every_package() {
        let tmp = repo();
        let reqs: Vec<UpdateRequest> = ["a", "b", "c"]
            .iter()
            .map(|n| UpdateRequest {
                name: (*n).into(),
                version: "2.0.0".into(),
                ecosystem: Ecosystem::Npm,
                project: String::new(),
            })
            .collect();
        let seen = std::sync::Mutex::new(Vec::new());
        let _ = run_on_branch_cancellable(
            tmp.path(),
            &reqs,
            None,
            |done, total| seen.lock().unwrap().push((done, total)),
            || false,
        );
        assert_eq!(seen.into_inner().unwrap(), vec![(1, 3), (2, 3), (3, 3)]);
    }

    /// The plain entry point still works, and reports nothing.
    ///
    /// `run` and `run_on_branch` share one body with the progress
    /// version, so this pins that the no-callback path is still there
    /// for every existing caller.
    #[test]
    fn run_on_branch_needs_no_callback() {
        let tmp = repo();
        let reqs = [UpdateRequest {
            name: "lodash".into(),
            version: "2.0.0".into(),
            ecosystem: Ecosystem::Npm,
            project: String::new(),
        }];
        let _ = run_on_branch(tmp.path(), &reqs, None);
    }

    /// A refusal must not leave a worktree behind.
    ///
    /// The same rule the package-name validation follows: everything is
    /// checked before anything is created. Mutation testing caught this
    /// -- removing the validation from `run_on_branch` passed every
    /// other test, because they all exercise the validator directly.
    #[test]
    fn a_bad_override_is_refused_before_a_worktree_exists() {
        let tmp = repo();
        let reqs = [UpdateRequest {
            name: "lodash".into(),
            version: "2.0.0".into(),
            ecosystem: Ecosystem::Npm,
            project: String::new(),
        }];
        // Asserted on OUR message, not merely that it failed.
        //
        // git rejects these names too -- `-dashname` gives "unknown
        // switch `s'" -- so a test that only checked for an error
        // passed with this validation removed. Mutation testing caught
        // exactly that. The point of validating here is a clear
        // refusal instead of git's confusing one, so the message is
        // what has to be asserted.
        let err = run_on_branch(tmp.path(), &reqs, Some("-dashname")).unwrap_err();
        // Wording only OUR refusal uses. `contains("option")` was not
        // enough: git's own failure prints a usage block containing
        // "options", so the assertion passed with the validation
        // removed. Two mutants in a row survived before this.
        assert!(
            err.contains("which git would read"),
            "expected our own refusal, got git's: {err}"
        );
        assert!(
            !tmp.path().join(".worktrees").exists(),
            "a refused branch name must create no worktree"
        );
    }

    /// A request that cannot succeed must not leave a worktree behind.
    #[test]
    fn run_refuses_swift_before_creating_anything() {
        let tmp = repo();
        let reqs = [UpdateRequest {
            name: "Alamofire".into(),
            version: "5.0".into(),
            ecosystem: Ecosystem::Swift,
            project: String::new(),
        }];
        assert!(run(tmp.path(), &reqs).is_err());
        assert!(
            !tmp.path().join(".worktrees").exists(),
            "a refused run must create no worktree"
        );
    }

    /// The worktree goes under `.worktrees/`, which this app already
    /// scans for and skips.
    #[test]
    fn worktree_path_is_under_dot_worktrees() {
        let p = worktree_path(Path::new("/repo"), "headstate/update-lodash");
        assert_eq!(p, Path::new("/repo/.worktrees/update-lodash"));
    }

    /// One package failing must not erase the others' reports. Uses a
    /// package manager that is certainly absent so the failure is
    /// deterministic and needs no network.
    #[test]
    fn a_failed_package_does_not_abort_the_run() {
        let tmp = repo();
        let reqs = [
            UpdateRequest {
                name: "definitely-not-a-real-package-xyz".into(),
                version: "9.9.9".into(),
                ecosystem: Ecosystem::Dotnet,
                project: String::new(),
            },
            UpdateRequest {
                name: "another-fake-package-abc".into(),
                version: "9.9.9".into(),
                ecosystem: Ecosystem::Dotnet,
                project: String::new(),
            },
        ];
        let Ok(report) = run(tmp.path(), &reqs) else {
            // dotnet absent: the run still must not panic, and there is
            // nothing further to assert on this machine.
            return;
        };
        assert_eq!(
            report.results.len(),
            2,
            "every request must report, including after a failure"
        );
        assert!(
            report.worktree.contains(".worktrees"),
            "{}",
            report.worktree
        );
    }

    /// `changed_files` must read git rather than assume, or a resolver
    /// touching something unexpected would be invisible.
    #[test]
    fn changed_files_reports_what_git_sees() {
        let tmp = repo();
        assert!(changed_files(tmp.path()).is_empty(), "clean repo");
        std::fs::write(tmp.path().join("package.json"), "{\"a\":1}\n").unwrap();
        std::fs::write(tmp.path().join("surprise.lock"), "x\n").unwrap();
        let seen = changed_files(tmp.path());
        assert!(seen.contains(&"package.json".to_string()), "{seen:?}");
        assert!(
            seen.contains(&"surprise.lock".to_string()),
            "an unexpected file must still be reported: {seen:?}"
        );
    }
}

/// End-to-end against a REAL package manager. Ignored by default: it
/// needs npm and the network, neither of which CI should depend on.
///
/// `cargo test -- --ignored --nocapture real_npm`
#[cfg(test)]
mod real {
    use super::tests::commit_args;
    use super::*;

    #[test]
    #[ignore = "needs npm and network access"]
    fn real_npm_update_end_to_end() {
        let fallbacks = tools::fallback_dirs();
        let refs: Vec<&str> = fallbacks.iter().map(String::as_str).collect();
        if tools::find("npm", &refs).is_none() {
            eprintln!("npm not found; skipping");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let git = |args: &[&str]| {
            std::process::Command::new(crate::auth::git_program())
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"t","version":"1.0.0","dependencies":{"lodash":"4.17.20"}}"#,
        )
        .unwrap();
        git(&["add", "-A"]);
        let owned = commit_args();
        git(&owned.iter().map(String::as_str).collect::<Vec<_>>());

        let reqs = [UpdateRequest {
            name: "lodash".into(),
            version: "4.17.21".into(),
            ecosystem: Ecosystem::Npm,
            project: String::new(),
        }];
        let report = run(dir, &reqs).expect("run should succeed");
        let r = &report.results[0];
        eprintln!("worktree:   {}", report.worktree);
        eprintln!("branch:     {}", report.branch);
        eprintln!("error:      {:?}", r.error);
        eprintln!("changed:    {:?}", r.changed_files);
        eprintln!("requested:  {}", r.requested);
        eprintln!("resolved:   {:?}", r.resolved_constraint);
        assert!(r.error.is_none(), "npm failed: {:?}", r.error);
        assert!(
            r.changed_files.iter().any(|f| f == "package.json"),
            "manifest should change: {:?}",
            r.changed_files
        );
        // The finding this phase exists to surface: npm rewrites a
        // pinned request as a caret RANGE. If this ever equals the
        // requested string, npm changed its behaviour and the report
        // needs revisiting.
        // The manifest was PINNED (`4.17.20`), so the update must keep
        // it pinned. Before `--save-exact` npm wrote `^4.17.21` here,
        // silently converting a pinned project into a floating one.
        assert_eq!(
            r.resolved_constraint.as_deref(),
            Some("4.17.21"),
            "a pinned manifest must stay pinned, not become a caret range"
        );
    }
}

#[cfg(test)]
mod branch_override {
    use super::*;

    /// The generated name is sanitised by construction; an override is
    /// not, and it reaches `git worktree add -b` and later a push.
    #[test]
    fn an_ordinary_name_is_accepted() {
        for good in [
            "update-deps",
            "headstate/update-lodash",
            "feature/deps.2026",
            "renovate/npm-all",
        ] {
            assert!(valid_branch_name(good).is_ok(), "{good} should be valid");
        }
    }

    /// A leading `-` is refused whatever git's ref grammar says: git
    /// would read it as an option rather than a name.
    #[test]
    fn an_option_like_name_is_refused() {
        let e = valid_branch_name("--force").unwrap_err();
        assert!(e.contains("option"), "{e}");
        assert!(valid_branch_name("-b").is_err());
    }

    /// git's own rules. A name that fails these fails at
    /// `git check-ref-format`, so refusing here turns a confusing git
    /// error into a clear one -- BEFORE a worktree exists.
    #[test]
    fn names_git_itself_would_reject_are_refused() {
        for bad in [
            "",          // empty
            "/leading",  // leading slash
            "trailing/", // trailing slash
            "double//slash",
            "ends.",      // trailing dot
            "has..dots",  // double dot
            "thing.lock", // reserved suffix
            "at@{brace}",
            "with space",
            "tilde~1",
            "caret^1",
            "colon:here",
            "question?",
            "star*",
            "bracket[",
            "back\\slash",
        ] {
            assert!(valid_branch_name(bad).is_err(), "{bad:?} should be refused");
        }
    }

    /// A control character would split one argument into two for
    /// anything that re-parses, and no real branch name has one.
    #[test]
    fn control_characters_are_refused() {
        assert!(valid_branch_name("new\nline").is_err());
        assert!(valid_branch_name("null\0byte").is_err());
        assert!(valid_branch_name("tab\there").is_err());
    }

    /// No override means the derived name, which is what every caller
    /// before #409 wanted.
    ///
    /// THESE ARE THE AGREEMENT CASES. `src/lib/branchName.test.ts` asserts
    /// the identical three, because `derivedBranchName` is a second
    /// implementation of this function -- the wizard has to show the name
    /// the run will use before the run starts, and that is worth a mirror
    /// only while the mirror is exact. If the two diverge the field shows
    /// one name and the run uses another, which is worse than not offering
    /// the field at all. Change one, change both, and change both test
    /// files.
    ///
    /// The clock is passed in for exactly this reason: a derivation that
    /// read `Utc::now()` internally could not be compared against
    /// anything, so there would be nothing for the TypeScript side to
    /// agree WITH.
    #[test]
    fn no_override_keeps_the_derived_name() {
        let when = at("2026-09-11T06:45:12Z");
        assert_eq!(
            branch_name_at(&["lodash".into()], Some(Ecosystem::Npm), when),
            "headstate/npm-deps-20260911-064512"
        );
        assert_eq!(
            branch_name_at(
                &["a".into(), "b".into(), "c".into()],
                Some(Ecosystem::Poetry),
                when
            ),
            "headstate/poetry-deps-20260911-064512"
        );
        assert_eq!(
            branch_name_at(&[], None, when),
            "headstate/deps-20260911-064512"
        );
    }

    /// A single-digit month, day, hour, minute or second must be
    /// zero-padded, or the stamp changes width and two runs an hour apart
    /// sort wrongly in `git branch`. Asserted because the TypeScript
    /// mirror builds the same string from `Date` getters, where padding is
    /// manual and therefore the likeliest place for the two to diverge.
    #[test]
    fn the_stamp_is_zero_padded_throughout() {
        assert_eq!(
            branch_name_at(&[], Some(Ecosystem::Uv), at("2026-01-02T03:04:05Z")),
            "headstate/uv-deps-20260102-030405"
        );
    }

    /// UTC, not local time. A name derived in two zones for the same
    /// instant must match, because the wizard predicts it on whatever
    /// machine the user is on and the run derives it again on the
    /// desktop -- a companion in a different zone would otherwise see a
    /// different name.
    #[test]
    fn the_stamp_is_utc() {
        // The same instant, written with an offset.
        assert_eq!(
            branch_name_at(&[], Some(Ecosystem::Npm), at("2026-09-11T06:45:12Z")),
            branch_name_at(&[], Some(Ecosystem::Npm), at("2026-09-11T08:45:12+02:00"))
        );
    }
}
