//! GitHub's record of a merged pull request, as a POSITIVE merge signal
//! for the worktree view (#1440).
//!
//! # Why the offline checks are not enough
//!
//! `scan::merged_into` looks for a branch's CONTENT on the default
//! branch. Ancestry misses every squash merge, so it falls through to two
//! squash signals, and both decay as the default branch moves on:
//!
//! - the aggregate patch-id hashes the diff WITH context lines, so a
//!   neighbouring change landing first gives the squash commit different
//!   context and a different id (measured: 0 of 3 matched);
//! - `content_landed` needs every file the branch touched to still read
//!   the way the branch left it, which any later edit to one of those
//!   files breaks (measured: 0/2, 9/18 and 6/18 files identical).
//!
//! So on an active repository a squash-merged branch stops reading as
//! merged within days, and the rows that most need reclaiming are the
//! ones that lose the label. GitHub's record of the merge does not decay.
//!
//! # The scan stays offline
//!
//! Nothing here runs inside `scan::classify`. That pass is offline by
//! design -- a per-worktree network call was rejected because it hangs on
//! an unreachable remote -- and this module does not reopen that. The
//! lookup is ONE batched GraphQL request per repository per
//! [`crate::github::query::MERGED_HEADS_CHUNK`] branch names, run AFTER
//! the offline verdicts exist, under a deadline and the process budget.
//!
//! # The rule is strict, and only ever upgrades
//!
//! [`qualifying_pr`] is the whole rule, and every part of it is a refusal
//! by default:
//!
//! - only a row the offline pass called `Unmerged` or `Unpushed` is a
//!   candidate ([`upgradeable`]); dirty, in-progress and locked rows are
//!   decided before either of those and are never asked about;
//! - the pull request must be `MERGED` (the query filters to it) and its
//!   base must be the repository's default branch as GitHub names it;
//! - the worktree's HEAD must BE the PR's `headRefOid`, or be an ancestor
//!   of it: the PR carried every commit the worktree has. A local commit
//!   past the PR's head is work GitHub never saw, and does not qualify.
//!
//! And every failure -- not signed in, no GitHub remote, budget reserved,
//! timed out, refused, repository missing -- leaves the offline verdict
//! exactly as it was. The row then says what the offline pass found,
//! never that GitHub said "not merged": we did not ask is not they did
//! not answer (#1050).

use super::model::{Safety, Worktree};
use crate::github::client::GitHubClient;
use crate::github::query::MERGED_HEADS_CHUNK;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long the whole lookup may take, across every chunk.
///
/// Short, because the worktree view waits on it: the offline verdicts
/// have already streamed in row by row, and this only decides whether a
/// few of them turn green. On a failure the rows simply stay as they are,
/// so a slow GitHub costs at most this long and never an answer.
pub const LOOKUP_DEADLINE: Duration = Duration::from_secs(10);

/// What one chunk costs. MEASURED live 2026-09-25: 36 aliases at
/// `first: 10` cost 1 point, as did 2.
const COST_PER_CHUNK: u64 = 1;

/// One merged pull request GitHub reports for a head branch name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPr {
    pub number: u64,
    /// The exact commit GitHub merged.
    pub head_oid: String,
    /// The branch it merged into.
    pub base: String,
}

/// What GitHub said about one repository's branches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Merges {
    /// The repository's default branch as GITHUB names it. `None` means
    /// it could not be read, and then nothing qualifies.
    pub default_branch: Option<String>,
    /// Merged pull requests per head branch name, for the branches GitHub
    /// ANSWERED about. A branch absent here was not answered, which is
    /// not the same as "has no merged pull request" -- and neither one
    /// moves a verdict.
    pub by_branch: HashMap<String, Vec<MergedPr>>,
}

/// A delete-time question to GitHub about one branch: `Ok` is what it
/// said, `Err` is why it could not be asked or did not answer.
///
/// A closure rather than a client, so the removal gate in `scan.rs` stays
/// synchronous and testable without a network: production hands it a
/// [`blocking_ask`], tests hand it a literal answer.
pub type Ask<'a> = &'a dyn Fn(&str) -> Result<Merges, String>;

/// The offline verdicts GitHub's record may upgrade.
///
/// `Unmerged` is the issue itself. `Unpushed` is the stale-tracking-ref
/// case `worktree_safety` describes: a squash-merged branch whose
/// `origin/<branch>` lingers reads as ahead of it, and the offline merge
/// check that would have rescued it is the one that decays. Both are
/// only reached for a clean, unlocked tree with no operation in progress,
/// because `worktree_safety` returns those first -- which is what keeps a
/// dirty worktree from ever becoming removable here.
pub fn upgradeable(s: &Safety) -> bool {
    matches!(s, Safety::Unmerged | Safety::Unpushed(_))
}

/// A full commit id -- 40 or 64 hex digits -- and nothing that git could read
/// as a flag or a revision expression.
///
/// `headRefOid` arrives from the network and is passed to git as an
/// argument, so it is checked at that boundary the way `is_safe_ref`
/// checks remote-controlled ref names.
fn is_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The strict rule: the number of a merged pull request that provably
/// contains this worktree's HEAD, or `None`.
///
/// Qualifies only when the PR's base is the default branch GitHub
/// reports AND `head` is its `headRefOid` exactly or an ancestor of it.
/// The ancestor test is local (`merge-base --is-ancestor`) and needs the
/// PR's head commit to be in this clone; when it is not, git fails and
/// the answer is `None` -- unproven is not merged.
///
/// Several can qualify when a branch name was reused; the highest number
/// is reported, deterministically.
pub fn qualifying_pr(dir: &Path, head: &str, branch: &str, merges: &Merges) -> Option<u64> {
    let default = merges.default_branch.as_deref()?;
    if !is_oid(head) {
        return None;
    }
    let prs = merges.by_branch.get(branch)?;
    prs.iter()
        .filter(|pr| pr.base == default && is_oid(&pr.head_oid))
        .filter(|pr| {
            pr.head_oid.eq_ignore_ascii_case(head)
                || super::scan::git(dir, &["merge-base", "--is-ancestor", head, &pr.head_oid])
                    .is_ok()
        })
        .map(|pr| pr.number)
        .max()
}

/// Upgrade every row the rule vouches for, returning which ones moved.
///
/// Touches only [`upgradeable`] rows, and only ever sets
/// `Safety::MergedAsPr` -- there is no path from here to a lesser
/// verdict, so a wrong or partial answer can at worst leave a row as the
/// offline pass left it.
pub fn apply(rows: &mut [Worktree], merges: &Merges) -> Vec<usize> {
    let mut changed = Vec::new();
    for (i, w) in rows.iter_mut().enumerate() {
        if w.is_main || w.branch.is_empty() || !upgradeable(&w.safety) {
            continue;
        }
        if let Some(n) = qualifying_pr(Path::new(&w.path), &w.head, &w.branch, merges) {
            w.safety = Safety::MergedAsPr(n);
            changed.push(i);
        }
    }
    changed
}

/// The distinct branch names worth asking GitHub about, in row order.
pub fn candidates(rows: &[Worktree]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .filter(|w| !w.is_main && !w.branch.is_empty() && upgradeable(&w.safety))
        .filter(|w| seen.insert(w.branch.clone()))
        .map(|w| w.branch.clone())
        .collect()
}

/// Read one chunk's answer into `into`.
///
/// Per alias: an array of nodes is an ANSWER (possibly empty) and is
/// recorded; a missing or null alias is no answer and is left out, so a
/// partially refused response still upgrades what it did answer. A node
/// missing any of the three fields is skipped rather than defaulted -- a
/// default `headRefOid` could only ever fail the rule, but a default base
/// or number would be a guess dressed as GitHub's word.
pub fn map_merged_heads(
    v: &serde_json::Value,
    chunk: &[String],
    into: &mut Merges,
) -> Result<(), String> {
    let repo = &v["repository"];
    if !repo.is_object() {
        return Err("GitHub returned no repository for this remote".into());
    }
    if into.default_branch.is_none() {
        into.default_branch = repo["defaultBranchRef"]["name"]
            .as_str()
            .map(str::to_string);
    }
    for (i, branch) in chunk.iter().enumerate() {
        let Some(nodes) = repo[format!("h{i}")]["nodes"].as_array() else {
            continue;
        };
        let prs = nodes
            .iter()
            .filter_map(|n| {
                Some(MergedPr {
                    number: n["number"].as_u64()?,
                    head_oid: n["headRefOid"].as_str()?.to_string(),
                    base: n["baseRefName"].as_str()?.to_string(),
                })
            })
            .collect();
        into.by_branch.insert(branch.clone(), prs);
    }
    Ok(())
}

/// The `owner/name` of a checkout's `origin` on github.com, or `None`.
///
/// Stricter than `scan::repo_identity`, which takes the last two path
/// segments of any host: a GitLab remote named `team/tool` must not be
/// looked up as the unrelated GitHub repository of the same name. The
/// strict rule would still refuse a stranger's pull request -- its head
/// commit is not in this clone -- but asking about it at all spends
/// budget on a question with no meaningful answer.
pub fn github_identity(repo_path: &str) -> Option<String> {
    let url = super::scan::git(Path::new(repo_path), &["remote", "get-url", "origin"]).ok()?;
    if !url.contains("github.com") {
        return None;
    }
    super::scan::parse_owner_repo(&url)
}

/// Ask GitHub about `branches` in `identity`, in chunks, under
/// [`LOOKUP_DEADLINE`] and the process budget.
///
/// # Partial is not nothing
///
/// The deadline is applied PER CHUNK against a shared clock, and each
/// answer is folded into `merges` as it lands, so a chunk that times out
/// costs its own branches and nothing already answered. A single outer
/// `timeout` would drop the accumulator with the future (#1044).
///
/// `Err` only when nothing at all was answered. A lookup that answered
/// some branches returns them; the rest are simply absent, which no rule
/// reads as "not merged".
pub async fn lookup(
    client: &GitHubClient,
    identity: &str,
    branches: &[String],
) -> Result<Merges, String> {
    let (owner, name) = identity
        .split_once('/')
        .ok_or_else(|| "the repository's GitHub name could not be read".to_string())?;
    let chunks: Vec<&[String]> = branches.chunks(MERGED_HEADS_CHUNK).collect();
    let budget = crate::github::stats::Budget::new();
    // The poll loop is what the reserve protects. Refusing here is "we
    // did not ask", and says so -- it is not a GitHub failure.
    if !budget.permits(chunks.len() as u64 * COST_PER_CHUNK) {
        return Err("not asked: the GitHub rate-limit reserve is held for the poll loop".into());
    }

    let deadline = Instant::now() + LOOKUP_DEADLINE;
    let mut merges = Merges::default();
    let mut answered = false;
    let mut last_err = None;
    for chunk in chunks {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            last_err = Some(format!(
                "GitHub did not answer within {}s",
                LOOKUP_DEADLINE.as_secs()
            ));
            break;
        };
        match tokio::time::timeout(left, client.merged_heads(owner, name, chunk, &budget)).await {
            Ok(Ok(v)) => match map_merged_heads(&v, chunk, &mut merges) {
                Ok(()) => answered = true,
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            },
            Ok(Err(e)) => {
                last_err = Some(e.to_string());
                // Stop rather than repeat a refusal: a rejected token or a
                // rate limit answers every later chunk the same way.
                break;
            }
            Err(_) => {
                last_err = Some(format!(
                    "GitHub did not answer within {}s",
                    LOOKUP_DEADLINE.as_secs()
                ));
                break;
            }
        }
    }
    match (answered, last_err) {
        (false, Some(e)) => Err(e),
        (_, e) => {
            if let Some(e) = e {
                log::info!("worktree merge lookup: partial answer ({e})");
            }
            Ok(merges)
        }
    }
}

/// Run the lookup for a classified repository and upgrade what qualifies.
///
/// Returns the rows and the indices that changed. Every way this can
/// fail returns the rows untouched: no client (not signed in), no GitHub
/// remote, no candidates, a refused or failed lookup. Logged by COUNT
/// only -- repository and branch names are not ours to put in a log a
/// user pastes into an issue.
pub async fn enrich(
    client: Option<std::sync::Arc<GitHubClient>>,
    repo_path: &str,
    rows: Vec<Worktree>,
) -> (Vec<Worktree>, Vec<usize>) {
    let Some(client) = client else {
        return (rows, Vec::new());
    };
    let branches = candidates(&rows);
    if branches.is_empty() {
        return (rows, Vec::new());
    }
    let repo = repo_path.to_string();
    let identity = match tauri::async_runtime::spawn_blocking(move || github_identity(&repo)).await
    {
        Ok(Some(id)) => id,
        _ => return (rows, Vec::new()),
    };
    let merges = match lookup(&client, &identity, &branches).await {
        Ok(m) => m,
        Err(e) => {
            log::info!(
                "worktree merge lookup: GitHub not consulted for {} branch(es); \
                 offline verdicts stand ({e})",
                branches.len()
            );
            return (rows, Vec::new());
        }
    };
    // The ancestor test is a git call per candidate PR, so it runs off the
    // async workers. A copy is kept so a failed join returns the offline
    // rows rather than none.
    let fallback = rows.clone();
    let mut rows = rows;
    match tauri::async_runtime::spawn_blocking(move || {
        let changed = apply(&mut rows, &merges);
        (rows, changed)
    })
    .await
    {
        Ok((rows, changed)) => {
            log::info!(
                "worktree merge lookup: {} of {} candidate(s) merged per GitHub",
                changed.len(),
                branches.len()
            );
            (rows, changed)
        }
        Err(_) => (fallback, Vec::new()),
    }
}

/// A production [`Ask`]: one bounded lookup for one branch, run to
/// completion on the calling (blocking) thread.
///
/// For the removal gate, which runs inside `spawn_blocking`. Only ever
/// invoked for a row the offline gate has already refused as unmerged or
/// unpushed, so a removal the offline gate allows spends nothing -- not
/// even the remote lookup, which is why the identity is read in here.
pub fn blocking_ask(
    client: std::sync::Arc<GitHubClient>,
    repo_path: String,
) -> impl Fn(&str) -> Result<Merges, String> {
    move |branch: &str| {
        let identity = github_identity(&repo_path)
            .ok_or_else(|| "this repository has no GitHub remote".to_string())?;
        tauri::async_runtime::block_on(lookup(&client, &identity, &[branch.to_string()]))
    }
}

/// The removal gate's second route. `Ok` lets the removal proceed; `Err`
/// carries the refusal to show.
///
/// Called only after the offline gate has refused with `offline`. Every
/// refusal other than `Unmerged`/`Unpushed` is returned unchanged without
/// asking anything. When GitHub cannot be asked the message SAYS so,
/// rather than presenting the offline refusal as GitHub's answer.
pub fn gate(wt: &Worktree, offline: &Safety, github: Option<Ask<'_>>) -> Result<(), String> {
    let refuse = || Err(format!("not safe to remove: {}", offline.reason()));
    let Some(ask) = github else {
        return refuse();
    };
    if !upgradeable(offline) || wt.branch.is_empty() {
        return refuse();
    }
    match ask(&wt.branch) {
        Ok(merges) => match qualifying_pr(Path::new(&wt.path), &wt.head, &wt.branch, &merges) {
            Some(n) => {
                log::info!("removal gate: pull request #{n} is merged per GitHub");
                Ok(())
            }
            None => refuse(),
        },
        Err(why) => Err(format!(
            "not safe to remove: {} — GitHub could not be asked to confirm a merge ({why})",
            offline.reason()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Real git, because the rule's second half is a real `merge-base`.
    /// Identity and names are synthetic per `CONTRIBUTING.md`.
    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs([
                ("GIT_AUTHOR_NAME", "octocat"),
                ("GIT_COMMITTER_NAME", "octocat"),
                ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo whose `feature` worktree was pushed and then SQUASH-merged,
    /// after which `main` edited the same file again -- the exact shape
    /// #1440 reports, where every offline signal has decayed.
    ///
    /// Returns (tempdir, repo, worktree, the pushed head sha).
    fn decayed_squash() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("app.txt"), "one\ntwo\nthree\n").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "base"]);
        run(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run(&repo, &["push", "-q", "-u", "origin", "main"]);

        let wt = tmp.path().join("proj-feature");
        let wt_s = wt.to_str().unwrap();
        run(
            &repo,
            &[
                "worktree", "add", "-q", "--track", "-b", "feature", wt_s, "main",
            ],
        );
        std::fs::write(wt.join("app.txt"), "one\nTWO\nthree\n").unwrap();
        run(&wt, &["commit", "-q", "-am", "change two"]);
        run(&wt, &["push", "-q", "-u", "origin", "feature"]);
        let head = run(&wt, &["rev-parse", "HEAD"]);

        // A neighbouring change lands first, so the squash commit's
        // CONTEXT differs from the branch's and no patch-id matches...
        std::fs::write(repo.join("app.txt"), "ONE\ntwo\nthree\n").unwrap();
        run(&repo, &["commit", "-q", "-am", "change one (#6)"]);
        // ...then the squash, then a later edit of the same line, so the
        // file no longer reads the way the branch left it.
        std::fs::write(repo.join("app.txt"), "ONE\nTWO\nthree\n").unwrap();
        run(&repo, &["commit", "-q", "-am", "change two (#7)"]);
        std::fs::write(repo.join("app.txt"), "ONE\nTWO!\nthree\n").unwrap();
        run(&repo, &["commit", "-q", "-am", "later edit"]);
        run(&repo, &["push", "-q", "origin", "main"]);
        (tmp, repo, wt, head)
    }

    fn merges(default: &str, branch: &str, prs: Vec<MergedPr>) -> Merges {
        Merges {
            default_branch: Some(default.into()),
            by_branch: HashMap::from([(branch.to_string(), prs)]),
        }
    }

    fn pr(number: u64, head_oid: &str, base: &str) -> MergedPr {
        MergedPr {
            number,
            head_oid: head_oid.into(),
            base: base.into(),
        }
    }

    fn row(wt: &Path, head: &str, safety: Safety) -> Worktree {
        Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            head: head.into(),
            safety,
            ..Default::default()
        }
    }

    /// The fixture reproduces the defect: offline, this merged branch is
    /// "not merged". Without this the tests below could pass against a
    /// fixture the offline pass already handles.
    #[test]
    fn the_fixture_is_the_reported_defect() {
        let (_t, repo, wt, _) = decayed_squash();
        let rows = crate::worktrees::scan::classify_repo(repo.to_str().unwrap()).unwrap();
        let w = rows
            .iter()
            .find(|w| w.branch == "feature")
            .expect("the feature worktree");
        assert_eq!(w.safety, Safety::Unmerged, "{:?}", wt);
    }

    /// HEAD == `headRefOid`, base == default: the one case the issue is
    /// about, and it upgrades.
    #[test]
    fn an_exact_head_on_a_pr_merged_into_the_default_branch_qualifies() {
        let (_t, _repo, wt, head) = decayed_squash();
        let m = merges("main", "feature", vec![pr(7, &head, "main")]);
        assert_eq!(qualifying_pr(&wt, &head, "feature", &m), Some(7));

        let mut rows = vec![row(&wt, &head, Safety::Unmerged)];
        assert_eq!(apply(&mut rows, &m), vec![0]);
        assert_eq!(rows[0].safety, Safety::MergedAsPr(7));
        assert!(rows[0].safety.is_safe());
    }

    /// The PR carried MORE than the worktree has: its head is a
    /// descendant of HEAD. Everything here is in the PR, so it qualifies.
    #[test]
    fn a_head_that_is_an_ancestor_of_the_pr_head_qualifies() {
        let (_t, _repo, wt, head) = decayed_squash();
        // The PR got one more commit, pushed from elsewhere; this
        // worktree is still at the older tip.
        run(&wt, &["commit", "-q", "--allow-empty", "-m", "review fix"]);
        let pr_head = run(&wt, &["rev-parse", "HEAD"]);
        run(&wt, &["reset", "-q", "--hard", &head]);

        let m = merges("main", "feature", vec![pr(7, &pr_head, "main")]);
        assert_eq!(qualifying_pr(&wt, &head, "feature", &m), Some(7));
    }

    /// A local commit BEYOND the PR's head is work GitHub never saw. The
    /// worktree must not read as merged, whatever the PR says.
    #[test]
    fn a_local_commit_past_the_pr_head_does_not_qualify() {
        let (_t, _repo, wt, pr_head) = decayed_squash();
        run(
            &wt,
            &["commit", "-q", "--allow-empty", "-m", "unpushed work"],
        );
        let head = run(&wt, &["rev-parse", "HEAD"]);

        let m = merges("main", "feature", vec![pr(7, &pr_head, "main")]);
        assert_eq!(qualifying_pr(&wt, &head, "feature", &m), None);

        let mut rows = vec![row(&wt, &head, Safety::Unpushed(1))];
        assert!(apply(&mut rows, &m).is_empty());
        assert_eq!(rows[0].safety, Safety::Unpushed(1));
    }

    /// Merged into something other than the default branch is not
    /// merged into the default branch.
    #[test]
    fn a_pr_merged_into_another_base_does_not_qualify() {
        let (_t, _repo, wt, head) = decayed_squash();
        let m = merges("main", "feature", vec![pr(7, &head, "release")]);
        assert_eq!(qualifying_pr(&wt, &head, "feature", &m), None);
        // Nor when GitHub's default branch could not be read.
        let unknown_default = Merges {
            default_branch: None,
            ..merges("main", "feature", vec![pr(7, &head, "main")])
        };
        assert_eq!(qualifying_pr(&wt, &head, "feature", &unknown_default), None);
    }

    /// A PR closed without merging never reaches the rule: the document
    /// asks for `states: MERGED` only, so GitHub's answer for such a
    /// branch is an empty node list -- which is an ANSWER, and upgrades
    /// nothing. Driven through the mapper, from the shape GitHub returns.
    #[test]
    fn a_closed_unmerged_pr_upgrades_nothing() {
        let (_t, _repo, wt, head) = decayed_squash();
        let mut m = Merges::default();
        let v = serde_json::json!({
            "repository": {
                "defaultBranchRef": { "name": "main" },
                "h0": { "nodes": [] }
            }
        });
        map_merged_heads(&v, &["feature".to_string()], &mut m).unwrap();
        assert_eq!(m.by_branch.get("feature"), Some(&Vec::new()));

        let mut rows = vec![row(&wt, &head, Safety::Unmerged)];
        assert!(apply(&mut rows, &m).is_empty());
        assert_eq!(rows[0].safety, Safety::Unmerged);
    }

    /// Only `Unmerged` and `Unpushed` are candidates. Dirty, in-progress,
    /// locked and the rest are never upgraded, even with a PR that would
    /// qualify -- a dirty tree must never become removable here.
    #[test]
    fn only_unmerged_and_unpushed_rows_are_upgraded() {
        let (_t, _repo, wt, head) = decayed_squash();
        let m = merges("main", "feature", vec![pr(7, &head, "main")]);
        for s in [
            Safety::Dirty(2),
            Safety::NeverPushed,
            Safety::Empty,
            Safety::Pending,
            Safety::Unknown("git failed".into()),
            Safety::InProgress {
                op: crate::worktrees::model::GitOperation::Rebase,
                conflicts: None,
            },
        ] {
            let mut rows = vec![row(&wt, &head, s.clone())];
            assert!(apply(&mut rows, &m).is_empty(), "{s:?} must not move");
            assert_eq!(rows[0].safety, s);
        }
        assert!(candidates(&[row(&wt, &head, Safety::Dirty(1))]).is_empty());
    }

    /// A network-supplied `headRefOid` that is not a commit id is never
    /// handed to git.
    #[test]
    fn a_head_oid_that_is_not_a_sha_never_qualifies() {
        let (_t, _repo, wt, head) = decayed_squash();
        for bad in ["--output=/tmp/x", "HEAD", "main", ""] {
            let m = merges("main", "feature", vec![pr(7, bad, "main")]);
            assert_eq!(qualifying_pr(&wt, &head, "feature", &m), None, "{bad}");
        }
    }

    /// A null alias is NO answer and is left out; a missing repository is
    /// an error. Neither is recorded as "no merged pull request".
    #[test]
    fn the_mapper_keeps_unanswered_distinct_from_answered_empty() {
        let mut m = Merges::default();
        let v = serde_json::json!({
            "repository": {
                "defaultBranchRef": { "name": "main" },
                "h0": null,
                "h1": { "nodes": [
                    { "number": 3, "headRefOid": "a".repeat(40), "baseRefName": "main" },
                    { "number": 4, "baseRefName": "main" }
                ] }
            }
        });
        let chunk = ["one".to_string(), "two".to_string()];
        map_merged_heads(&v, &chunk, &mut m).unwrap();
        assert!(!m.by_branch.contains_key("one"), "unanswered is absent");
        assert_eq!(m.by_branch["two"], vec![pr(3, &"a".repeat(40), "main")]);
        assert_eq!(m.default_branch.as_deref(), Some("main"));

        let gone = serde_json::json!({ "repository": null });
        assert!(map_merged_heads(&gone, &chunk, &mut Merges::default()).is_err());
    }

    // ---- the removal gate ----------------------------------------------

    /// With GitHub vouching under the strict rule, the delete-time gate
    /// removes a worktree the offline gate refuses.
    #[test]
    fn the_gate_removes_what_github_vouches_for() {
        let (_t, repo, wt, head) = decayed_squash();
        let ask = |b: &str| -> Result<Merges, String> {
            assert_eq!(b, "feature");
            Ok(merges("main", "feature", vec![pr(7, &head, "main")]))
        };
        let repo_s = repo.to_str().unwrap();
        let wt_s = wt.to_str().unwrap();
        // The offline gate alone still refuses: nothing about it changed.
        assert!(crate::worktrees::scan::remove_worktree(repo_s, wt_s).is_err());
        crate::worktrees::scan::remove_worktree_asking(repo_s, wt_s, &ask).unwrap();
        assert!(!wt.is_dir(), "the merged worktree is removed");
    }

    /// A lookup that could not be made leaves the offline refusal in
    /// place, and the message says GitHub was not asked -- it does not
    /// present the offline verdict as GitHub's answer.
    #[test]
    fn a_failed_lookup_keeps_the_offline_refusal_and_says_so() {
        let (_t, repo, wt, _) = decayed_squash();
        let ask =
            |_: &str| -> Result<Merges, String> { Err("not signed in to GitHub".to_string()) };
        let err = crate::worktrees::scan::remove_worktree_asking(
            repo.to_str().unwrap(),
            wt.to_str().unwrap(),
            &ask,
        )
        .unwrap_err();
        assert!(err.contains("branch not merged"), "{err}");
        assert!(err.contains("could not be asked"), "{err}");
        assert!(wt.is_dir(), "the worktree must still exist");
    }

    /// A dirty worktree is refused WITHOUT asking GitHub at all, however
    /// merged its pull request is.
    #[test]
    fn the_gate_never_asks_about_a_dirty_worktree() {
        let (_t, repo, wt, head) = decayed_squash();
        std::fs::write(wt.join("scratch.txt"), "unsaved\n").unwrap();
        let asked = std::cell::Cell::new(false);
        let ask = |_: &str| -> Result<Merges, String> {
            asked.set(true);
            Ok(merges("main", "feature", vec![pr(7, &head, "main")]))
        };
        let err = crate::worktrees::scan::remove_worktree_asking(
            repo.to_str().unwrap(),
            wt.to_str().unwrap(),
            &ask,
        )
        .unwrap_err();
        assert!(err.contains("uncommitted"), "{err}");
        assert!(!asked.get(), "a dirty tree is never a GitHub question");
        assert!(wt.is_dir());
    }

    /// A commit made after the PR merged is refused at delete time, even
    /// though the same PR would have vouched for the tree a moment ago.
    #[test]
    fn the_gate_refuses_a_commit_made_after_the_merge() {
        let (_t, repo, wt, pr_head) = decayed_squash();
        run(&wt, &["commit", "-q", "--allow-empty", "-m", "late work"]);
        let ask = |_: &str| -> Result<Merges, String> {
            Ok(merges("main", "feature", vec![pr(7, &pr_head, "main")]))
        };
        let err = crate::worktrees::scan::remove_worktree_asking(
            repo.to_str().unwrap(),
            wt.to_str().unwrap(),
            &ask,
        )
        .unwrap_err();
        assert!(err.contains("not safe to remove"), "{err}");
        assert!(wt.is_dir());
    }

    // ---- the lookup, against a mock GitHub -------------------------------

    async fn client_for(server: &wiremock::MockServer) -> GitHubClient {
        let oc = octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        GitHubClient::new(oc)
    }

    /// The lookup reads GitHub's answer into `Merges`. No `rateLimit` in
    /// the mock on purpose: `Budget::record` would write the process-wide
    /// figure every other test's gate reads (src-tauri/CLAUDE.md).
    #[tokio::test]
    async fn the_lookup_reads_merged_prs_per_branch() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, ResponseTemplate};
        // Cold start, scoped to this test: no shared budget figure read or written.
        let _scope = crate::github::stats::budget::scoped::enter(u64::MAX);
        let server = wiremock::MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("states: MERGED"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "repository": {
                    "defaultBranchRef": { "name": "main" },
                    "h0": { "nodes": [
                        { "number": 7, "headRefOid": "b".repeat(40), "baseRefName": "main" }
                    ] }
                } }
            })))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let m = lookup(&client, "octo-org/octo-app", &["feature".to_string()])
            .await
            .unwrap();
        assert_eq!(m.default_branch.as_deref(), Some("main"));
        assert_eq!(m.by_branch["feature"], vec![pr(7, &"b".repeat(40), "main")]);
    }

    /// A failed lookup is an `Err` -- "did not answer" -- never an empty
    /// `Merges` that a caller could mistake for "nothing merged".
    #[tokio::test]
    async fn a_failed_lookup_is_an_error_not_an_empty_answer() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        // Cold start, scoped to this test: no shared budget figure read or written.
        let _scope = crate::github::stats::budget::scoped::enter(u64::MAX);
        let server = wiremock::MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "repository": null },
                "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository" }]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let err = lookup(&client, "octo-org/octo-app", &["feature".to_string()])
            .await
            .unwrap_err();
        assert!(err.contains("no repository"), "{err}");
    }

    /// Under the rate-limit reserve the lookup is NOT MADE, and the error
    /// says "not asked" -- the poll loop's budget is not spent on this,
    /// and nobody reads the refusal as GitHub's answer.
    #[tokio::test]
    async fn the_budget_reserve_means_not_asked() {
        use wiremock::matchers::method;
        use wiremock::{Mock, ResponseTemplate};
        let _scope = crate::github::stats::budget::scoped::enter(100);
        let server = wiremock::MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let err = lookup(&client, "octo-org/octo-app", &["feature".to_string()])
            .await
            .unwrap_err();
        assert!(err.contains("not asked"), "{err}");
    }

    /// Past one chunk, the answers of EVERY chunk that arrived are kept.
    #[tokio::test]
    async fn every_chunk_is_asked_and_kept() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        // Cold start, scoped to this test: no shared budget figure read or written.
        let _scope = crate::github::stats::budget::scoped::enter(u64::MAX);
        let server = wiremock::MockServer::start().await;
        // Answers every alias a chunk can hold with one merged PR, so
        // both chunks' branches must land.
        let mut repo = serde_json::Map::new();
        repo.insert(
            "defaultBranchRef".into(),
            serde_json::json!({ "name": "main" }),
        );
        for i in 0..MERGED_HEADS_CHUNK {
            repo.insert(
                format!("h{i}"),
                serde_json::json!({ "nodes": [
                    { "number": 1, "headRefOid": "c".repeat(40), "baseRefName": "main" }
                ] }),
            );
        }
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "data": { "repository": repo } })),
            )
            .expect(2)
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let branches: Vec<String> = (0..MERGED_HEADS_CHUNK + 1)
            .map(|i| format!("branch-{i}"))
            .collect();
        let m = lookup(&client, "octo-org/octo-app", &branches)
            .await
            .unwrap();
        assert_eq!(m.by_branch.len(), branches.len());
    }
}
