use serde::{Deserialize, Serialize};

/// A checkout with worktrees hanging off it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    /// `owner/repo` from the git remote, when it can be established.
    ///
    /// From the REMOTE, never the directory name -- this app's own
    /// directory is `ghstat` while its repository is
    /// `pktstorm/headstate`. Used to pair a worktree with its pull
    /// request; None means no pairing rather than a fuzzy one.
    #[serde(default)]
    pub identity: Option<String>,
    /// Directory name, e.g. `octo-api`.
    pub name: String,
    /// Absolute path to the main checkout.
    pub path: String,
    pub worktrees: Vec<Worktree>,
    /// When this repository's remote refs were last fetched, RFC 3339,
    /// or `None` if it has never been fetched or the time is unreadable.
    ///
    /// Every merge and upstream verdict below is computed against
    /// `origin/*` refs already on disk -- the scan deliberately never
    /// goes to the network, because a view that opens in a second must
    /// not become one that opens in thirty by fetching 37 remotes.
    ///
    /// That decision is right and stays. What was missing is telling
    /// anyone about it: on this machine one repository's refs were 12
    /// days old, so its rows were answering as of a fortnight ago while
    /// reading like the present tense (#702). `Current` is the worst of
    /// them, because "up to date" is exactly what it does NOT mean.
    ///
    /// `None` rather than a zero or a guess: never fetched and cannot
    /// tell are both "we do not know", and neither is "just now".
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// What [`crate::worktrees::scan::default_branch`] resolved for this
    /// repository -- the REMOTE-TRACKING ref where one exists
    /// (`origin/main`, `origin/master`), the bare local short name where
    /// it does not (#757, #1026).
    ///
    /// # Why this is on the wire rather than re-derived
    ///
    /// The All Repositories table names the branch its verdict is about,
    /// and a hardcoded `origin/main` is the wrong answer for over 10% of
    /// the repositories on the reporting machine: `osiris` resolves to
    /// `origin/master`, `claude-mkt` to a long feature branch because
    /// `origin/HEAD` points at one, and two fall back to a LOCAL `main`
    /// because no `origin/main` ref resolves.
    ///
    /// Re-deriving it in a fifth place was the other option and is
    /// refused: `invariants.rs` already guards four functions named
    /// `default_branch` against drifting apart, and #757 measured what
    /// disagreement costs -- a nine-row verdict swing in 34, every
    /// affected row carrying a confident, false reason.
    ///
    /// # It costs nothing
    ///
    /// `collect_inner` ALREADY calls `default_branch` to classify the
    /// worktrees; this carries the string it already has instead of
    /// discarding it. No extra git invocation, on a scan whose per-repo
    /// cost is the thing every other decision here is budgeted against.
    ///
    /// # `None` is "not resolved", never a default
    ///
    /// The orphan branch has no repository to ask, so it sends `None`
    /// rather than the word `main`. A consumer must render that as "we
    /// could not tell" -- substituting a plausible ref would be this
    /// codebase's characteristic absent-read-as-success bug (#967,
    /// #769, #841) applied to the one field that says what was compared.
    #[serde(default)]
    pub default_ref: Option<String>,
}

/// Why a worktree can or cannot be removed.
///
/// Deliberately an enum rather than a bool: the UI has to explain ITSELF,
/// and "3 uncommitted files" is actionable where a greyed-out button is
/// not. `NeverPushed` is separate from `Unmerged` because it is the
/// dangerous one -- measured, 5 of 25 sampled worktrees have no upstream
/// at all, so their commits exist nowhere else on earth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum Safety {
    /// Merged, clean, and pushed. Removable.
    ///
    /// Carries the date the branch landed in the default branch, when it
    /// can be determined -- knowing a branch merged yesterday versus four
    /// months ago changes how confidently you delete it.
    Safe,
    /// The repository's own checkout, not a worktree.
    MainCheckout,
    /// Uncommitted changes; the number of affected paths.
    Dirty(u64),
    /// Commits not on the remote; how many.
    Unpushed(u64),
    /// No upstream branch at all -- nothing has ever been pushed.
    NeverPushed,
    /// Merged, but the remote branch has since been deleted (#732).
    ///
    /// Distinct from `Safe` because the ROUTE to the verdict differs and
    /// the user deserves to see which one they got. `Safe` means the
    /// upstream still exists and agrees; this means the upstream is gone
    /// and the content was found on the default branch instead. Both are
    /// removable, but only one of them can be re-checked against a
    /// remote afterwards.
    ///
    /// Distinct from `NeverPushed` because it is the opposite verdict.
    /// `rev-parse @{u}` fails identically for both, which is exactly the
    /// bug: a branch whose PR merged and whose remote was then deleted
    /// was reported as commits existing only on this machine.
    MergedUpstreamDeleted,
    /// A branchless checkout whose HEAD is already on the default
    /// branch (#819). Removable.
    ///
    /// Carries what the sha resolves to in ref-relative terms, e.g.
    /// `v1.13.0~30`, or the bare word "detached" when no ref reaches it.
    /// That string is the difference between a row the user can act on
    /// and one they cannot: "detached at v1.13.0~30" identifies the
    /// checkout, where "detached" only says what it lacks.
    ///
    /// Its own variant rather than `Safe`, and the reason is what #776
    /// is about. `Safe` means "merged, pushed", and for a checkout with
    /// no branch the second half is a claim no evidence was gathered for
    /// -- there is no tracking config to read. `MergedUpstreamDeleted`
    /// would be worse: it specifically means the tracking config
    /// outlived the remote branch, describing evidence that never
    /// existed here. This variant says only the two things that were
    /// actually established -- the content is on the default branch, and
    /// there is no branch -- and keeps the set of verdicts a branchless
    /// checkout can produce a finite, greppable list.
    ///
    /// In `is_safe` deliberately. A clean checkout contained in the
    /// default branch has nothing to lose, and arguably less than a
    /// merged branch does: there is no branch ref to forget about. The
    /// evidence required is `merged_into`'s, unchanged -- ancestry or an
    /// exact patch-id match -- so this widens which ROWS can present
    /// that evidence, not what counts as evidence. Before #819 these
    /// rows were `Unknown`, with no action at all: four on the reporting
    /// machine, every one provably an ancestor of the default branch.
    DetachedMerged(String),
    /// The branch was created and never committed to.
    ///
    /// Its own state rather than a flavour of `Safe` or `NeverPushed`,
    /// because the CLAIM is different and the difference is what the
    /// user came for. `NeverPushed` says "these commits exist only
    /// here", which for a branch with no commits of its own is simply
    /// false -- and the row said it next to "0 commits ahead", a
    /// contradiction one user spent a session resolving by hand.
    /// `Safe` would be true but weaker: it invites "merged when?",
    /// where this answers "there was never anything here".
    Empty,
    /// The repository that owned this worktree is gone.
    ///
    /// A category of its own rather than a flavour of `Unknown`,
    /// because the claim is different: `Unknown` means a check failed
    /// and might succeed later, while this means the checkout can never
    /// be classified again -- there is no git to run in it. It is also
    /// the only state where the DIRECTORY is the whole story, since
    /// nothing else can be read from it.
    Orphaned,
    /// Branch is not merged into the default branch.
    Unmerged,
    /// Someone locked this worktree; git refuses to remove it (#753).
    ///
    /// Carries git's own lock reason, which is the whole point of the
    /// variant. `git worktree lock --reason` exists so the locker can
    /// say who they are, and a tool that locks a tree while it works in
    /// it writes something like "some tool (pid 123)". That string is
    /// the evidence a user needs to tell a LIVE claim from a leftover
    /// one -- a process that is still running versus one that died
    /// without unlocking. `None` when the lock carries no reason, which
    /// git also permits.
    ///
    /// Its own variant rather than a flavour of `Unknown`, because the
    /// check did not fail: the answer is known, specific, and has an
    /// obvious remedy. It is also not a flavour of `Dirty` -- a lock
    /// says nothing about the contents, only that something claimed the
    /// directory.
    ///
    /// NOT removable, and deliberately not force-removable behind the
    /// scenes: `git worktree remove` suggests `-f -f` to override, and
    /// passing that silently would defeat the only mechanism git gives a
    /// concurrent process for saying "I am using this". 13 of 34
    /// worktrees on the reporting machine were locked by running agents.
    ///
    /// The payload grew a struct in #775. `Option<String>` said only
    /// what git said, and on a machine where 20 of 44 worktrees are
    /// locked that turned out to be the wrong amount of information in
    /// both directions -- too much of the useless part, none of the
    /// useful. `Lock` carries the age, whether the named process is
    /// plausibly the holder, and what the worktree WOULD be without the
    /// lock. See `Lock` for why each of those exists.
    Locked(Lock),
    /// The directory is gone and git knows the registration is stale.
    ///
    /// Git emits `prunable <reason>` for exactly this, and `git worktree
    /// prune` clears it. Before #753 the missing directory fell through
    /// to `Unknown("directory is missing")`, which reads as corruption
    /// -- a true statement that told the user nothing about what to do,
    /// for what is ordinary, resolvable bookkeeping.
    ///
    /// Carries git's reason (typically "gitdir file points to
    /// non-existent location") rather than the app's own guess, so the
    /// row reports what git actually said.
    ///
    /// NOT removable, though nothing here could be lost: there is no
    /// directory left to remove, so the remove path is simply the wrong
    /// action. Pruning is the right one, and it is a different verb.
    Prunable(String),
    /// Listed, but not yet classified. A transient state the UI shows as
    /// a skeleton rather than as an answer -- distinct from `Unknown`,
    /// which means the check ran and could not decide.
    Pending,
    /// Git could not answer; never assume safe on an error.
    Unknown(String),
}

/// What is known about a lock, beyond the fact of it.
///
/// #753 carried git's reason string alone, on the reasoning that "some
/// tool (pid 123)" is what separates a live claim from a leftover one.
/// Measured on the reporting machine once locks had accumulated, that
/// reasoning did not survive contact:
///
/// - **20 of 44 worktrees are locked** -- 45% of the list, up from the
///   third #753 measured.
/// - **Every one names the same pid**, and that process is ALIVE. It is
///   the long-lived parent session, not the individual short-lived
///   workers that actually took the locks; those finished hours or days
///   ago. `ps` therefore answers "alive" for every row, and a user
///   reading the reason cannot tell a current claim from an abandoned
///   one.
/// - **`lsof -d cwd` returns nothing** for any of them. Nothing is
///   working in those directories.
/// - The reason string embeds its own `start <date>`, and **all 20
///   carry the identical timestamp** -- the session's start, not the
///   lock's. So the one thing in the string that looks like an age is
///   the same on every row and ages all of them together.
///
/// So the pid is noise dressed as evidence, and this struct exists to
/// put honest evidence beside it rather than to delete it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// Git's own lock reason, or `None` for a lock taken without
    /// `--reason` -- which git permits and reports as a bare `locked`
    /// line.
    ///
    /// Kept verbatim. It is still the only thing the locker chose to
    /// say, and rewriting it would be the app inventing a claim on
    /// another process's behalf. What changed in #775 is its STANDING:
    /// it is now one field among several rather than the whole answer.
    pub reason: Option<String>,
    /// Whole days since the lock was taken, or `None` if unreadable.
    ///
    /// **The load-bearing field, and the one that actually reads as
    /// stale.** "locked 5 days ago" is a fact a user can act on where
    /// a bare pid is not, and unlike the pid it differs per row: the
    /// locks on the reporting machine span 6 September to today.
    ///
    /// Measured from the mtime of git's own `locked` file in the
    /// worktree's admin directory, NOT from the `start` timestamp
    /// inside the reason string. Verified against real git: locking
    /// writes that file, and unlocking and re-locking rewrites it, so
    /// its mtime is when the CURRENT claim was made. The reason's own
    /// timestamp was tried first and rejected -- all 20 locks on the
    /// reporting machine carry the same one, because it dates the
    /// process, not the claim.
    ///
    /// `None` rather than 0 when it cannot be read. A lock of unknown
    /// age is not a lock taken this second, and that is exactly the
    /// direction in which a wrong guess would make removal feel safer.
    pub age_days: Option<u64>,
    /// Whether the process the reason names is still the process that
    /// took the lock, or `None` when the reason names no process to
    /// check.
    ///
    /// Deliberately NOT called "the lock is live". A live holder is weak
    /// evidence here and the app must not launder it into a strong
    /// claim: on the reporting machine this is `Some(true)` for all 20
    /// locks, every one of them abandoned, because the pid belongs to
    /// the surviving parent. It is carried so the UI can say what was
    /// checked -- and so a `Some(false)` can say the one genuinely
    /// decisive thing, that the process named is gone. `Lock::holder_is_gone`
    /// is the predicate that reads it, and it exists so the three
    /// consumers cannot drift on which of the three values is decisive.
    ///
    /// "The process that took it" and not merely "a process with that
    /// pid" (#792). Pids are recycled, so a pid-only check reads a lock
    /// naming a long-dead worker as LIVE the moment something unrelated
    /// inherits its number -- and the reporting machine had rebooted
    /// between taking these locks and reading them, which is exactly
    /// when every pid in the space gets handed out again. Our lock
    /// reasons already record the holder's start time beside the pid, so
    /// `holder_is_running` compares the pair; see it for the tolerance
    /// and the local-time reading. A reason with no start time degrades
    /// to a bare existence check, which is what lockers other than our
    /// own tooling get.
    pub holder_running: Option<bool>,
    /// What this worktree would be if the lock were cleared.
    ///
    /// **The reason unlocking stops being a blind action** (#775). The
    /// lock decides whether removal is POSSIBLE and the merge state
    /// whether it is DESIRABLE; #753 answered only the first, which was
    /// right as far as it went and left a user clearing a claim with no
    /// idea whether the thing behind it was disposable. With 45% of
    /// rows locked, that is what makes the view unusable.
    ///
    /// Boxed because `Safety` contains this struct, and a plain
    /// `Safety` field here would make the type infinitely sized.
    ///
    /// This does NOT widen the gate. It is carried for display; the
    /// verdict governing the button is still `Locked`, and `is_safe`
    /// never looks inside. A locked worktree that is merged underneath
    /// is still locked.
    pub underlying: Box<Safety>,
}

impl Lock {
    /// Prose for the age, or `None` when it is unknown.
    ///
    /// Whole days, because that is the resolution the decision needs:
    /// nobody unlocks differently for 5 days versus 5 days and 3 hours,
    /// and a precise figure would imply a precision the mtime does not
    /// really carry. "today" rather than "0 days ago", which reads as a
    /// missing value.
    pub fn age_phrase(&self) -> Option<String> {
        match self.age_days? {
            0 => Some("today".into()),
            1 => Some("yesterday".into()),
            n => Some(format!("{n} days ago")),
        }
    }

    /// Whether the process this lock names is PROVABLY gone.
    ///
    /// Its own predicate, mirrored as `lockHolderIsGone` in
    /// `src/lib/worktrees.ts`, because three places now turn on this one
    /// fact -- the row's prose, the row's colour, and which rows a bulk
    /// unlock may touch -- and a `== Some(false)` written out three
    /// times is three chances to drift into `!= Some(true)` (#792).
    ///
    /// That distinction is the whole point. `Some(false)` is the one
    /// decisive signal available here: the reason named a process and it
    /// is not running. `None` means the reason named no process to
    /// check, and it must NOT read as "nothing holds it" -- most locks
    /// not written by our own tooling land there, and treating an
    /// unasked question as a negative answer is how a live claim gets
    /// cleared. `Some(true)` is weak evidence in the other direction and
    /// is not spent as proof either; see `holder_running`.
    pub fn holder_is_gone(&self) -> bool {
        self.holder_running == Some(false)
    }
}

/// Defaults to `Pending`, never `Safe`.
///
/// A partially-constructed `Worktree` must not be deletable: the default
/// is the value a bug is most likely to leave behind, and neither
/// `Pending` nor `Unknown` is deletable.
impl Default for Safety {
    /// Not-yet-checked, which is NOT the same as checked-and-failed.
    ///
    /// This used to default to `Unknown("not yet classified")`, which the
    /// UI rendered as "could not determine: not yet classified" -- a
    /// failed check, in the same grey as a real failure. The fast listing
    /// lands in ~2.6s and classification takes up to ~57s, so for most of
    /// a minute every row claimed its safety check had failed.
    fn default() -> Self {
        Safety::Pending
    }
}

impl Safety {
    /// Only `Safe` may be deleted. Everything else is disabled in the UI
    /// rather than warned past -- a cleanup tool that occasionally eats a
    /// day of work is worse than no cleanup tool.
    ///
    /// `Empty` is deliberately NOT safe, though nothing on the branch
    /// could be lost. #701 is a report that the REPORTING was wrong --
    /// an empty branch was described as holding commits that exist only
    /// here -- not that the gate was too tight. Making `Empty` safe
    /// would silently promote a large, previously-refused population to
    /// one-click deletable as a side effect of fixing wording: 52 of
    /// 296 worktrees on the reporting machine have no upstream, and an
    /// unknown share of those are empty. Widening the only
    /// unrecoverable action in the app is its own decision, taken on
    /// its own evidence, not a rider on a copy fix.
    ///
    /// The user is not stuck: `Empty` says plainly that there is
    /// nothing to lose, and `remove_worktree_forced` -- reached through
    /// a confirmation that quotes this reason -- is exactly the path
    /// for "the app is being careful and I have read why".
    pub fn is_safe(&self) -> bool {
        // Every arm means the same thing: the work is on the default
        // branch and the tree is clean. They are separate variants so the
        // row can say which evidence was used, not because one is safer
        // than another.
        //
        // `DetachedMerged` joined them in #819, and it is the one
        // addition here that is a WIDENING of the allowlist rather than a
        // renaming, so the argument is worth stating. The evidence is
        // `merged_into`'s and is unchanged: ancestry, or an exact
        // patch-id match, the identical bar `Safe` clears. What the
        // detached row lacks is a BRANCH, and a branch is what you would
        // lose by removing a worktree -- so its absence makes removal
        // safer, not riskier. Those rows were previously `Unknown` with
        // no action offered at all, which is the dead end #819 reports.
        //
        // `Prunable` and `Empty` stay out, and for reasons that do not
        // apply here: see `Safety::Prunable` (remove is the wrong verb
        // when there is no directory) and the note on `Empty` above (a
        // large previously-refused population must not be promoted as a
        // side effect of a wording fix).
        matches!(
            self,
            Safety::Safe | Safety::MergedUpstreamDeleted | Safety::DetachedMerged(_)
        )
    }

    /// Display-ready prose for the row, so the UI does not re-derive it.
    pub fn reason(&self) -> String {
        match self {
            Safety::Safe => "merged, pushed, safe to delete".into(),
            Safety::MainCheckout => "the repository's main checkout".into(),
            Safety::Dirty(n) => format!("{n} uncommitted file{}", if *n == 1 { "" } else { "s" }),
            Safety::Unpushed(n) => {
                format!("{n} unpushed commit{}", if *n == 1 { "" } else { "s" })
            }
            Safety::NeverPushed => "never pushed — commits exist only here".into(),
            Safety::MergedUpstreamDeleted => "merged; upstream deleted".into(),
            // MERGED FIRST, then the detachment (#819).
            //
            // The old wording for this row was "could not determine:
            // detached HEAD", which led with a failure and named a
            // missing branch. Both halves were the wrong way round: the
            // user's question is "can I clear this", the answer is yes,
            // and the detachment is the caveat rather than the headline.
            //
            // The name carries the `at <ref>` when one was resolvable,
            // so the row reads "merged into main — detached at
            // v1.13.0~30". That identifies the checkout, which "detached
            // HEAD" never did.
            //
            // Says "no branch to delete" rather than stopping at
            // "detached", because the reassurance is the part the user
            // came for: the usual worry about removing a worktree is
            // losing the branch, and here there is none.
            Safety::DetachedMerged(at) => {
                format!("merged — {at}, no branch to delete")
            }
            // Says what is TRUE of the branch, not what the app will
            // let you do about it. "Nothing to lose" is the fact the
            // user was trying to establish by hand; whether the Remove
            // button is enabled is a separate, more cautious question
            // answered by `is_safe`.
            Safety::Empty => "no commits of its own — nothing to lose".into(),
            Safety::Unmerged => "branch not merged".into(),
            // AGE FIRST, then the reason (#775).
            //
            // #753 led with the reason on the theory that naming the
            // locker separates a live claim from a stale one. Measured
            // once locks accumulated, it does not: all 20 on the
            // reporting machine name one pid, and that pid is alive
            // because it is the surviving parent of workers that
            // finished days ago. So the reason reads as live evidence
            // for every row including every abandoned one.
            //
            // The age is the fact that differs per row and that a stale
            // lock cannot fake. Leading with it means a five-day-old
            // lock READS as five days old, which is the whole ask.
            //
            // The reason still follows, unrewritten. It is the locker's
            // own words and occasionally identifies something real; it
            // has simply stopped being the headline.
            Safety::Locked(lock) => {
                let mut s = "locked".to_string();
                if let Some(age) = lock.age_phrase() {
                    s.push(' ');
                    s.push_str(&age);
                }
                match &lock.reason {
                    Some(why) => s.push_str(&format!(" by {why}")),
                    // Says the lock carries no note rather than
                    // trailing off, which would read as a display bug.
                    None => s.push_str(" — no reason given"),
                }
                // APPENDED after git's reason, never folded into it
                // (#792).
                //
                // `holder_running` was computed on every scan from the
                // beginning and read by nothing but the unlock dialog,
                // so the user learned the holder was dead only AFTER
                // deciding to unlock and opening the confirmation --
                // which is the wrong end of the decision. The row is
                // where the decision is made.
                //
                // After the reason rather than replacing it, because
                // the reason is the locker's own words and rewriting
                // them would be the app inventing a claim on another
                // process's behalf (`Lock::reason` is explicit about
                // this). The row therefore reads "locked 2 days ago by
                // claude agent … — holder process is gone": git's
                // sentence, then ours.
                //
                // Said ONLY for `Some(false)`. `Some(true)` is weak
                // evidence -- on the reporting machine it was true for
                // all 20 locks and every one was abandoned -- and
                // printing "holder still running" on every row would
                // spend that weak evidence as though it were proof.
                // `None` means the reason named no pid to check, and
                // silence is the honest rendering of that.
                if lock.holder_is_gone() {
                    s.push_str(" — holder process is gone");
                }
                // What the row could not say before: whether clearing
                // the lock would reveal something disposable. Without
                // it, unlocking is a leap.
                if lock.underlying.is_safe() {
                    s.push_str(" — merged, would be safe once unlocked");
                }
                s
            }
            // THE SAFE PART FIRST, then the mechanism (#814).
            //
            // #753 gave this row "directory is gone — prunable
            // (<git's reason>)", a large improvement on "could not
            // determine: directory is missing" -- it named a remedy where
            // the old wording named neither cause nor cure -- and #793
            // shipped that remedy as a button. What both left is the
            // READING: the row still opened with a loss ("directory is
            // gone") and a piece of git vocabulary ("prunable"), while the
            // reassurance that nothing can be lost lived only in a
            // tooltip. The issue was filed twice, which is the signal --
            // the page was answering "can I clear this?" with a
            // vocabulary lesson instead of a yes.
            //
            // So the order is inverted. "Nothing to lose" is the answer,
            // the missing directory is why, and git's own reason stays in
            // parentheses because it is what git actually said and
            // occasionally differs.
            //
            // The VERB is still prune, not remove, and `is_safe` still
            // excludes this state. That distinction is correct -- there is
            // no directory to remove, and `git worktree prune` is
            // repo-wide rather than per-row -- and #814 is explicit that
            // it is not asking for the allowlist to widen. What it asks is
            // that being correct stop reading as a warning.
            Safety::Prunable(why) => {
                format!("nothing to lose — its directory is already gone, prune to clear ({why})")
            }
            Safety::Pending => "checking…".into(),
            // Says what IS known, not what could not be checked. The
            // parent repository is gone, so nothing about this
            // checkout's contents can be established -- and the user
            // needs to know that before deciding, not a hedge.
            Safety::Orphaned => "its repository is gone — nothing here can be checked".into(),
            Safety::Unknown(why) => format!("could not determine: {why}"),
        }
    }
}

/// How a checkout stands against its tracked upstream.
///
/// Separate from `Safety` on purpose: safety answers "may I delete
/// this?", while this answers "is this current?". Folding them together
/// would make the main checkout's row a safety verdict about a directory
/// nobody is proposing to delete.
///
/// Comparison is against the last fetch -- reading refs already on disk,
/// never the network. This is a local disk-usage view, and a scan that
/// silently fetched 37 remotes would be both slow and surprising.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "n")]
pub enum Upstream {
    /// Level with the upstream as of the last fetch.
    Current,
    Ahead(u64),
    Behind(u64),
    /// Both sides moved: `.0` ahead, `.1` behind.
    Diverged(u64, u64),
    /// A local-only branch. Normal, not an error -- and distinctly not
    /// "up to date", which is what a bare zero would imply.
    Untracked,
    /// No branch to compare, so the question does not apply.
    Detached,
    Unknown(String),
}

impl Upstream {
    /// Display-ready prose, so the UI does not re-derive it.
    pub fn reason(&self) -> String {
        let commits = |n: &u64| format!("{n} commit{}", if *n == 1 { "" } else { "s" });
        match self {
            Upstream::Current => "up to date with upstream".into(),
            Upstream::Ahead(n) => format!("{} ahead of upstream", commits(n)),
            Upstream::Behind(n) => format!("{} behind upstream", commits(n)),
            Upstream::Diverged(a, b) => {
                format!("diverged: {} ahead, {} behind", commits(a), commits(b))
            }
            Upstream::Untracked => "no upstream — local only".into(),
            Upstream::Detached => "detached HEAD".into(),
            Upstream::Unknown(why) => format!("upstream unknown: {why}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub head: String,
    /// Bytes on disk. `None` until measured -- sizing 202 trees is a walk
    /// over hundreds of thousands of files, so it is deliberately lazy.
    pub size_bytes: Option<u64>,
    pub safety: Safety,
    /// True for the repository's own checkout.
    pub is_main: bool,
    /// How this checkout stands against its upstream.
    ///
    /// Computed for EVERY worktree, not just the main checkout. That
    /// restriction was right when a row's only action was Remove and the
    /// safety verdict answered the only question; Claudify changed it,
    /// and "3 commits ahead" is the evidence for whether there is
    /// anything worth keeping.
    pub upstream: Option<Upstream>,
    /// RFC 3339 timestamp of the branch tip's own commit.
    ///
    /// NOT `merged_at`, which is when the work reached the default
    /// branch. A branch written in March and merged in August has both,
    /// and they answer different questions: this one says how stale the
    /// work is, that one says whether it is already accounted for.
    pub last_commit: Option<String>,
    /// `YYYY-MM-DD` when this branch landed in the default branch.
    ///
    /// The date the work reached the default branch, NOT the branch tip's
    /// own commit date. They coincide for a fast-forward but diverge for
    /// a branch written weeks before it merged, and the merge date is the
    /// one that answers "is this safe to forget about".
    pub merged_at: Option<String>,
    /// Git's `locked` line: `Some(reason)`, `Some("")` for a bare lock,
    /// `None` when the worktree is not locked (#753).
    ///
    /// Two levels of Option are not an accident. The OUTER one is the
    /// question "is this locked", and the inner emptiness is "locked,
    /// but the locker left no note" -- git permits `git worktree lock`
    /// with no `--reason`, and emits a bare `locked` line for it.
    /// Collapsing them would make an unlocked worktree and an
    /// unexplained lock the same value, and only one of those refuses
    /// to be removed.
    #[serde(default)]
    pub locked: Option<String>,
    /// Git's `prunable` reason, or `None` when the registration is live.
    ///
    /// No inner Option: git always supplies a reason on this line.
    #[serde(default)]
    pub prunable: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole `is_safe` allowlist, stated once so that widening it is
    /// a deliberate edit to a test rather than a side effect.
    ///
    /// Mirrored by `isSafe` in `src/lib/worktrees.ts`, whose own test
    /// asserts the same membership kind by kind. The two MUST agree: this
    /// one is the gate the remove command checks, that one greys the
    /// button, and a disagreement is either a button that lies or a
    /// command that refuses what the page offered.
    ///
    /// `DetachedMerged` joined in #819 and is the only widening since
    /// #732. Written out beside the refusals rather than asserted alone,
    /// because the property worth pinning is the BOUNDARY -- that
    /// `Prunable` and `Empty` did not come with it, and that `Unknown`
    /// (which is what an unmerged detached checkout reports) did not
    /// either.
    #[test]
    fn the_safe_allowlist_is_exactly_the_merged_states() {
        for s in [
            Safety::Safe,
            Safety::MergedUpstreamDeleted,
            Safety::DetachedMerged("detached at v1.13.0~30".into()),
        ] {
            assert!(s.is_safe(), "{s:?} is one of the merged states");
        }
        for s in [
            Safety::MainCheckout,
            Safety::Dirty(3),
            Safety::Unpushed(2),
            Safety::NeverPushed,
            // Nothing on the branch could be lost, and it is still not
            // one-click removable: #701 reported that the WORDING was
            // wrong, and widening the app's only unrecoverable action on
            // the back of a copy fix is not what was asked for.
            Safety::Empty,
            Safety::Unmerged,
            // #814 is explicit that this allowlist stays strict: there is
            // no directory to remove, so remove is the wrong verb, and
            // prune is repo-wide. That issue is about how the row READS.
            Safety::Prunable("gitdir file points to non-existent location".into()),
            Safety::Orphaned,
            Safety::Pending,
            // What an unmerged detached checkout reports (#819). It must
            // stay out: those commits may exist nowhere else, and having
            // no branch is not evidence that they landed.
            Safety::Unknown("detached HEAD at v1.13.0~30 — not found on main".into()),
        ] {
            assert!(!s.is_safe(), "{s:?} must not be one-click removable");
        }
    }

    /// The default must never be deletable. A partially-constructed
    /// `Worktree` is what a bug leaves behind, and this is the one place
    /// where getting it wrong deletes someone's work.
    #[test]
    fn the_default_safety_is_not_deletable() {
        assert!(!Safety::default().is_safe());
        assert_eq!(Safety::default(), Safety::Pending);
    }

    /// `Pending` and `Unknown` are different states and must stay
    /// different.
    ///
    /// `Pending` means "not checked yet" and shows as a skeleton;
    /// `Unknown` means "checked, could not decide" and shows as a
    /// failure. Collapsing them is what made every unclassified row
    /// claim its safety check had failed for the first minute of a scan.
    #[test]
    fn pending_reads_as_waiting_not_as_failure() {
        assert_eq!(Safety::Pending.reason(), "checking…");
        let unknown = Safety::Unknown("git exploded".into());
        assert!(unknown.reason().contains("could not determine"));
        assert_ne!(Safety::Pending.reason(), unknown.reason());
        assert!(!Safety::Pending.is_safe());
    }
}
