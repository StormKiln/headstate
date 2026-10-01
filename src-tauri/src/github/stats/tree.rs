//! The scope tree: which organisations, repositories and people a stats
//! question can be asked ABOUT, enumerated from GitHub.
//!
//! #825's half of the feature. `scope.rs` answers "how do I spell a
//! question about this scope"; this module answers the question that comes
//! first and had no answer at all: **what scopes exist?** Before this,
//! nothing in the app enumerated an organisation or a person -- #823
//! recorded zero hits for `organization`, `membersWithRole` and
//! `viewer.organizations`, and the only identity query in the codebase was
//! `{ viewer { login } }`.
//!
//! # What it replaces, and why that had to go
//!
//! The PR Stats sidebar listed `repoCounts(prs)` (`src/lib/repos.ts`):
//! repositories where the VIEWER HAS AN OPEN PR. That list cannot express
//! the question the feature is for. It has no organisations and no people
//! in it, so "how is my team doing?" is unaskable; and it is derived from
//! open pull requests, so a repository with no current PR activity is
//! absent even though its history is exactly what a lead wants to read.
//! #825 requirement 4, stated as a rule: **absence of PRs is not absence
//! of repo.**
//!
//! It is also local-git-free by construction (#825 requirement 3). No
//! worktree root, no `git remote`, nothing on disk: a checkout is where
//! you happen to have cloned something, which has no bearing on whose
//! statistics you may want to read.
//!
//! # Discovery is CHEAP; measurement is separate
//!
//! The rule is `hooks.ts:712-717`'s, the one every expensive hook in this
//! app already follows: enumerate on entering the view, measure nothing
//! until something is clicked. So this module fetches **no statistics at
//! all** -- not a count, not a PR, not a leaderboard. It returns names.
//!
//! That is affordable in a way worth stating precisely, because the whole
//! design rests on it. MEASURED live, `gh api graphql`, 2026-09-11, this
//! account (2 orgs: FNX-Labs 4 members / 49 repos, Stohic 4 / 10):
//!
//! | Query | Cost | Wall clock |
//! |---|---|---|
//! | [`orgs_query`] -- org logins + counts | 1 | 0.59s |
//! | [`tree_query`] -- both orgs' repos AND members, plus personal repos | 1 | 0.83-1.04s (3 runs) |
//!
//! **One point and about a second for the entire hierarchy**, because
//! rate-limit cost is driven by nested connections at the scale they are
//! *paged*, not by how many nodes come back -- the same fact
//! `poll.rs:1503-1580` records and #827 leant on. A 559-repo / 224-member
//! org measured cost 1 at 2.18-2.22s.
//!
//! # Ordering: most-recently-active, sorted by GitHub
//!
//! `repositories(orderBy: {field: PUSHED_AT, direction: DESC})` sorts
//! SERVER-SIDE at **cost 1** -- the same point an unordered connection
//! costs. VERIFIED: the 49 FNX-Labs repositories come back strictly
//! descending by `pushedAt` with no client-side sort, and a monotonicity
//! check over the live response found no inversion.
//!
//! This is worth being explicit about because #825 originally worried that
//! activity ordering would need the per-repo activity data that cheap
//! discovery exists to avoid fetching. It does not. There is no activity
//! probe and no extra request.
//!
//! ## The caveat, recorded so nobody "fixes" it into an expensive query
//!
//! `PUSHED_AT` is **any push, not pull-request activity**. A repository
//! whose only recent commit was a dependency bot outranks one with a
//! week-old human PR. That is a real limitation and it is accepted
//! deliberately:
//!
//! - It is still far better than alphabetical, which orders by nothing a
//!   user cares about.
//! - The honest alternative -- ordering by recent PR count -- needs
//!   exactly the per-repo activity query this design avoids: one search
//!   per repository, 49 of them for one org, before the user has clicked
//!   anything. That is the opposite of "discovery is cheap".
//!
//! So if the ordering proves misleading in use, the fix is a deliberate
//! second query behind the same explicit-load gate the statistics
//! themselves use -- NOT a silent upgrade of this tree into something that
//! costs 49 searches to draw a sidebar.
//!
//! [`RepoRow::pushed_at`] is carried into the UI for this reason: ordering
//! by recency is invisible unless the rows say when they were touched. A
//! user cannot otherwise tell whether position 12 is a week stale or three
//! years dead.
//!
//! # Degrading honestly (#825 requirement 2, the #769 lesson)
//!
//! `membersWithRole` needs organisation read permission. An org where the
//! token lacks it must SAY so rather than render an empty Members list,
//! because an empty list reads as "this org has no members" -- silence
//! read as success, which is the defect #769 was about and which
//! `client.rs:1140-1190` already has scar tissue for on the PR path.
//!
//! I measured what GitHub actually does rather than assuming, and the
//! result **changed the shape of the query**. A `FORBIDDEN` on one field
//! nulls the **entire parent organisation object**, not just that field:
//!
//! ```text
//! {"data":{"a":null},"errors":[{"type":"FORBIDDEN",
//!   "path":["a","ipAllowListEnabledSetting"], ...}]}
//! ```
//!
//! Two consequences, both load-bearing:
//!
//! 1. **Each org is a separate ALIAS, not an entry in
//!    `viewer.organizations.nodes`.** Inside a `nodes` list a nulled org
//!    is an anonymous hole -- there is no login left to label it with, so
//!    the UI could not name the organisation it failed to read. As
//!    aliases, [`orgs_query`] has already told us the login for every
//!    alias, so a null is attributable: see [`OrgTree::readable`].
//! 2. **The blast radius is exactly one alias.** VERIFIED by putting a
//!    forbidden field on one org beside a readable org and the viewer in
//!    one document: the bad org came back `null` while the good org kept
//!    all 4 members and 49 repositories and the viewer kept its 6. So one
//!    inaccessible organisation degrades to one labelled row and costs
//!    the others nothing.
//!
//! This works only because `client::graphql_partial_ok` keeps `data` when
//! `errors` is non-empty (`client.rs:256-270`). Octocrab's own
//! deserialization would have discarded the whole response -- so the
//! honest-degradation path here is inherited from that fix rather than
//! reinvented, and a regression there would turn one forbidden org into a
//! blank sidebar.
//!
//! # Truncation is reported, never silent
//!
//! Both connections are capped at [`PAGE`] nodes and `totalCount` keeps
//! telling the truth above it -- VERIFIED on a 559-repo / 224-member org:
//! 100 nodes returned, `totalCount` 559, `pageInfo.hasNextPage` true. So
//! [`OrgTree::repos_total`] beside a shorter [`OrgTree::repos`] is how a
//! row count that is a SAMPLE says so, which is #824 item 8's rule ("never
//! a bare number over a sample") applied to the tree. #802 and #790 both
//! shipped silent truncation; this does not.

use crate::github::client::{refused_fields_of, ClientError, GitHubClient};
use serde_json::json;

/// Nodes per connection in [`tree_query`].
///
/// 100 is GitHub's per-connection maximum, and taking all of it is right
/// here for a reason that does not generalise: these nodes are a name, a
/// timestamp and two booleans, not the per-PR diff statistics that made
/// #827 default its page to 50. Cost is 1 either way, and the measured
/// difference between a 59-node tree (0.83-1.04s) and a 659-node one
/// (2.18-2.22s) is seconds, not the ~11s deadline.
///
/// Asking for less would mean a 49-repo org arriving pre-truncated, which
/// is the outcome this feature exists to avoid -- and because the order is
/// most-recently-active, a truncation at 100 drops the DEADEST
/// repositories rather than an arbitrary alphabetical tail.
pub const PAGE: u32 = 100;

/// Organisations per [`orgs_query`].
///
/// A separate limit from [`PAGE`] because exceeding it has a worse
/// consequence: [`tree_query`] builds one alias per organisation, and
/// alias count drives a document toward the ~11s deadline #827 measured.
/// 50 organisations of aliases is well past what that document should
/// carry in one request.
///
/// This account has 2. A user in 50+ organisations gets the first 50 and
/// [`Tree::orgs_total`] says how many there are, so the number is visible
/// rather than a silent cut. Paging the rest is deferred rather than
/// guessed at: the chunking would need to be measured against the
/// deadline, and nothing in reach has the scale to measure it on.
pub const MAX_ORGS: usize = 50;

/// Step one: which organisations, and how big are they.
///
/// Deliberately separate from [`tree_query`] even though the fields could
/// nest in one document, and the reason is the FORBIDDEN shape above: this
/// query is what supplies the logins that make a nulled alias in step two
/// attributable to an organisation by name. Without it, "an org we could
/// not read" has nothing to put in the row.
///
/// It is also the cheap half -- cost 1, measured 0.59s -- and it is the
/// half that cannot be refused: an organisation the viewer belongs to is
/// always listable even when its membership is not readable.
///
/// `membersWithRole { totalCount }` and `repositories { totalCount }` are
/// nested CONNECTIONS, which is the thing `poll.rs:1503-1580` warns drives
/// cost. Both measured cost 1 here, and they are worth the line: the
/// counts let the UI render "Members (4)" before step two lands, and they
/// are the reference [`OrgTree::repos_truncated`] compares against.
/// Built from [`MAX_ORGS`] rather than spelling `first: 50` inline.
///
/// Two literals that have to agree is the drift this repo writes tests
/// about: a `MAX_ORGS` raised without the document would silently keep
/// taking 50, and a document raised without `MAX_ORGS` would fetch
/// organisations the caller then dropped on the floor. One number, used
/// twice, and `the_org_alias_count_is_bounded_by_what_the_query_asks_for`
/// pins them together.
pub fn orgs_query() -> String {
    format!(
        "query {{
  rateLimit {{ cost remaining resetAt }}
  viewer {{
    login
    organizations(first: {MAX_ORGS}) {{
      totalCount
      nodes {{
        login
        name
        membersWithRole {{ totalCount }}
        repositories {{ totalCount }}
      }}
    }}
  }}
}}
"
    )
}

/// Step two: every organisation's repositories and members, plus the
/// viewer's own repositories, in ONE request.
///
/// One request rather than one per organisation because it measured cost 1
/// and 0.83-1.04s for this account's whole hierarchy -- there is no
/// fan-out to cap and no concurrency to bound, so `fetch::READ_CONCURRENCY`
/// is not involved in drawing the tree at all.
///
/// # Why the aliases are `o0`, `o1`, ... and not the org logins
///
/// A GraphQL alias must match `/[_A-Za-z][_0-9A-Za-z]*/`, and
/// organisation logins legally contain hyphens -- `FNX-Labs` is not a
/// valid alias and would make the document a syntax error. Index aliases
/// are generated, never user text, so the document cannot be malformed by
/// an org name; the index maps back to the login through the `logins`
/// slice the caller already has from [`orgs_query`].
///
/// # The viewer's own repositories
///
/// `affiliations: [OWNER]` and `ownerAffiliations: [OWNER]` together mean
/// "repositories this user owns", which is what `Scope::Personal`'s
/// `user:<login>` qualifier asks about. Without them the connection
/// returns every repository the viewer can see INCLUDING the
/// organisations' -- so the Personal section would repeat the org sections
/// and "All repos" under Personal would silently mean "everything".
/// VERIFIED: 6 repositories, all `pktstorm/`-owned, no org repository
/// among them.
pub fn tree_query(logins: &[String]) -> String {
    let mut doc =
        String::from("query {\n  rateLimit { cost remaining resetAt }\n  viewer {\n    login\n");
    // The viewer's own repositories, ordered and flagged exactly like the
    // organisations' so the two sections of the sidebar are comparable.
    doc.push_str(&format!(
        "    repositories(first: {PAGE}, orderBy: {{field: PUSHED_AT, direction: DESC}}, \
         affiliations: [OWNER], ownerAffiliations: [OWNER]) {{\n      \
         totalCount\n      nodes {{ nameWithOwner pushedAt isArchived }}\n    }}\n  }}\n"
    ));
    for (i, login) in logins.iter().enumerate() {
        // The login is interpolated as a STRING ARGUMENT, never as an
        // identifier, so a hyphen or a dot in it is data rather than
        // syntax. Escaped because a quote or a backslash in the value
        // would otherwise close the string and change the document --
        // GitHub does not permit such logins, but a query builder that
        // relies on the server's validation for its own well-formedness
        // is one API change away from being wrong.
        doc.push_str(&format!(
            "  o{i}: organization(login: \"{}\") {{\n    login\n    name\n    \
             membersWithRole(first: {PAGE}) {{ totalCount nodes {{ login name avatarUrl }} }}\n    \
             repositories(first: {PAGE}, orderBy: {{field: PUSHED_AT, direction: DESC}}) \
             {{ totalCount nodes {{ nameWithOwner pushedAt isArchived }} }}\n  }}\n",
            escape_graphql_string(login)
        ));
    }
    doc.push_str("}\n");
    doc
}

/// Escape a value for a GraphQL double-quoted string.
///
/// Only backslash and quote, because those are the two characters that can
/// terminate or re-open the literal and therefore the two that could turn
/// data into syntax. GitHub logins cannot contain either -- this is belt
/// and braces for a document the app builds by concatenation, and it is
/// cheaper than the alternative of trusting every future caller to
/// validate before calling.
fn escape_graphql_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// One repository a stats question can be scoped to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRow {
    /// `owner/name`, which is exactly what `Scope::Repo` holds -- so a
    /// clicked row needs no reassembly and cannot disagree with the
    /// qualifier it produces.
    pub name_with_owner: String,
    /// When anything was last pushed, as GitHub's UTC timestamp string
    /// (`2026-09-11T14:32:13Z`), or `None` for a repository
    /// that has never been pushed to.
    ///
    /// `Option` because an empty repository genuinely has no value here.
    /// Defaulting it to the epoch would sort correctly by accident and
    /// then render "last active 1970", which is a lie about a repository
    /// created this morning. (MEASURED: no null across the 65 repositories
    /// in reach, so this is the unexercised-but-correct branch rather than
    /// a common case.)
    pub pushed_at: Option<String>,
    /// Whether GitHub marks this repository archived.
    ///
    /// Carried rather than filtered here. See [`Tree`] for the decision
    /// about what the UI does with it -- the rule is that this layer
    /// reports and the view decides, because "exclude from aggregates but
    /// still list" cannot be expressed by dropping rows.
    pub is_archived: bool,
}

/// One person a stats question can be scoped to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRow {
    /// The login, which is what `Subject::Login` needs.
    pub login: String,
    /// The display name, when the account has one.
    ///
    /// Shown beside the login rather than instead of it: the login is the
    /// identity the statistics are actually keyed on (`author:<login>`),
    /// and a leaderboard that showed only display names would be one
    /// rename away from being unverifiable against GitHub's own UI.
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// One organisation, with everything the sidebar needs to draw it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgTree {
    pub login: String,
    /// The display name, e.g. "FNX Labs LLC" for `FNX-Labs`.
    pub name: Option<String>,
    /// Repositories, most-recently-pushed first (GitHub's order, kept).
    pub repos: Vec<RepoRow>,
    /// What GitHub says the true repository count is, which may exceed
    /// `repos.len()` -- see [`OrgTree::repos_truncated`].
    pub repos_total: u64,
    pub members: Vec<MemberRow>,
    pub members_total: u64,
    /// Whether the organisation's detail could be read at all.
    ///
    /// `false` means the alias came back `null` -- the token resolved the
    /// organisation (it is in [`orgs_query`]'s list) and was then refused
    /// its contents.
    ///
    /// # Two causes, and the app cannot tell them apart from here (#840)
    ///
    /// Either a SAML-SSO authorisation the user has not granted, or a token
    /// without `read:org`. They are very different to the user -- the first
    /// needs an administrator, the second is one command they can run
    /// themselves (`gh auth refresh -s read:org`) -- and this flag cannot
    /// distinguish them, because a `null` alias carries no cause.
    ///
    /// #840 asked for the distinction to be drawn from the
    /// `X-OAuth-Scopes` response header, which IS present on the GraphQL
    /// endpoint (verified live 2026-09-12 against `api.github.com/graphql`:
    /// `x-oauth-scopes: admin:org, gist, repo, workflow`). It is not drawn
    /// here because this module has no access to response headers:
    /// `client::graphql_partial_ok` goes through `octocrab.post`, which
    /// deserialises the body and discards the response. Reading the header
    /// means a new call path in `client.rs` -- `octocrab::_get` does return
    /// the full `http::Response` -- which is a change to the shared client
    /// rather than to this module.
    ///
    /// What #840 is actually about is the MESSAGE, and that is fixed where
    /// the message lives (`StatsSidebar.tsx`, `StatsPage.tsx`,
    /// `stats/Leaderboard.tsx`): all three now name the scope fix first and
    /// SSO second, because the scope fix is the one the reader can act on
    /// alone. Telling a user to contact an administrator about SSO when one
    /// command would have fixed it is the failure; saying both, cheapest
    /// first, costs nothing and needs no header.
    ///
    /// Also worth recording, because #840's premise was out of date:
    /// `gh auth login` DOES request `read:org`. Its own help on gh 2.100.0
    /// states the minimum scopes as `repo`, `read:org` and `gist`, so a
    /// `gh`-authenticated user is not in this situation at all. The real
    /// case is a hand-made token in `GH_TOKEN`/`GITHUB_TOKEN` (`auth.rs`
    /// accepts both), where `read:org` is easy to omit -- which is what the
    /// README's scope table now documents.
    ///
    /// This is the whole point of #825 requirement 2 and it must not be
    /// collapsed into "empty": an org with no members readable and an org
    /// with no members are different facts, and rendering the first as the
    /// second is the #769 failure -- silence read as success. The UI is
    /// obliged to branch on this, which is why it is not an
    /// `Option<Vec<..>>` that a caller could `unwrap_or_default()` without
    /// noticing.
    pub readable: bool,
}

impl OrgTree {
    /// Whether the repository list is a SAMPLE of a longer one.
    ///
    /// Compared against `repos_total` rather than against [`PAGE`]: the
    /// two agree today, but a future page size would make a `len() == PAGE`
    /// test silently wrong for an org holding exactly `PAGE` repositories.
    pub fn repos_truncated(&self) -> bool {
        (self.repos.len() as u64) < self.repos_total
    }

    pub fn members_truncated(&self) -> bool {
        (self.members.len() as u64) < self.members_total
    }

    /// Repositories that count toward an aggregate, i.e. the unarchived
    /// ones.
    ///
    /// The decision #825 asked to be made deliberately, and this is the
    /// shape of it: archived repositories are **excluded from aggregates
    /// but still listed, marked**. Reasoning, since the issue asked for
    /// it rather than a coin flip:
    ///
    /// - **Still listed**, because a lead may legitimately want the
    ///   history of a finished project, and that history does not stop
    ///   being true when the repository is frozen. Dropping the row makes
    ///   an answerable question unaskable, and the user has no way to tell
    ///   it was ever available.
    /// - **Out of the aggregate**, because "how is this org doing" over a
    ///   recent window is a question about live work, and a frozen
    ///   repository can only ever contribute zero to a recent window while
    ///   still inflating any per-repository denominator.
    /// - MEASURED on this account: 2 of the 49 FNX-Labs repositories are
    ///   archived (`open-asm`, last pushed 2026-04; `patcher`, 2023-03),
    ///   so this is 4% of a real list rather than a hypothetical.
    ///
    /// Note this does NOT affect the `All repos` SCOPE, which is a
    /// different thing from a sum over these rows -- see [`Tree`].
    pub fn active_repos(&self) -> impl Iterator<Item = &RepoRow> {
        self.repos.iter().filter(|r| !r.is_archived)
    }

    pub fn archived_count(&self) -> usize {
        self.repos.iter().filter(|r| r.is_archived).count()
    }
}

/// The whole scope hierarchy, as the sidebar renders it.
///
/// # `All repos` is a distinct scope, not a sum of the rows
///
/// #825 requirement 5, and it is a correctness point rather than a
/// shortcut. `Scope::Org("FNX-Labs")` asks GitHub one question about the
/// organisation; summing per-repository answers asks 49 questions and adds
/// them. Those differ in two ways that matter:
///
/// - The **1,000-result cap slices differently**. An org-wide search is
///   capped across the whole org and is why #827 built probe-driven
///   slicing; a per-repository connection has no cap at all
///   (`scope.rs:needs_search`). A sum of 49 capped searches is not the
///   same number as one sliced org search, and neither is it the same as a
///   sum of 49 uncapped connections.
/// - The **row list may be truncated** ([`OrgTree::repos_truncated`]) and
///   excludes archived repositories from aggregates. A sum over the rows
///   would therefore silently omit both, while the org scope covers
///   everything GitHub has.
///
/// So `All repos` is rendered as its own row that selects the org or
/// personal scope, and the per-repository rows select repo scopes. The two
/// are not expected to agree to the unit, and a future "why don't these
/// add up" question has its answer here.
/// `PartialEq` but NOT `Eq`, unlike the row types above: `Spend` is only
/// `PartialEq` (it reports a rate-limit `pressure()` as a float), and
/// deriving `Eq` here would mean either adding a dishonest `Eq` to a type
/// with float-shaped semantics or dropping the spend from the tree. The
/// spend is worth more -- a sidebar that can say what it cost is the whole
/// point of `budget.rs`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tree {
    /// The authenticated login. The Personal section's scope value, and
    /// what resolves `@me` for a cache key (`scope.rs:cache_key`).
    pub viewer: String,
    pub orgs: Vec<OrgTree>,
    /// How many organisations GitHub says the viewer belongs to, which may
    /// exceed `orgs.len()` at [`MAX_ORGS`].
    pub orgs_total: u64,
    /// The viewer's own repositories, most-recently-pushed first.
    pub personal: Vec<RepoRow>,
    pub personal_total: u64,
    /// Fields GitHub refused across the two requests.
    ///
    /// Surfaced for the same reason `Outcome::refused_fields` is
    /// (`fetch.rs`): a tree assembled from a partial response must be able
    /// to say so. Non-zero with `orgs` all readable means something was
    /// refused that was not an organisation -- worth showing rather than
    /// swallowing, because the alternative is a quietly short list.
    pub refused_fields: usize,
    /// What the two requests cost. Read from `rateLimit`, never assumed --
    /// `budget.rs` explains why a guessed cost recorded as a measurement
    /// is the defect that module exists to prevent.
    pub spend: super::budget::Spend,
}

impl Tree {
    /// Organisations whose detail could not be read.
    ///
    /// Exists so the UI can render the honest-degradation banner from one
    /// call rather than filtering the list itself at each of the places
    /// that might want to mention it.
    pub fn unreadable_orgs(&self) -> impl Iterator<Item = &OrgTree> {
        self.orgs.iter().filter(|o| !o.readable)
    }

    /// Whether anything about this tree is incomplete.
    ///
    /// The same design as `Outcome::is_complete`: one question a view can
    /// branch on, so that a partiality channel added later cannot be
    /// forgotten at one of several call sites.
    pub fn is_complete(&self) -> bool {
        self.refused_fields == 0
            && (self.orgs.len() as u64) >= self.orgs_total
            && (self.personal.len() as u64) >= self.personal_total
            && self
                .orgs
                .iter()
                .all(|o| o.readable && !o.repos_truncated() && !o.members_truncated())
    }
}

/// Enumerate the hierarchy: two requests, cost 1 each, no statistics.
///
/// # Why two requests and not one
///
/// [`tree_query`] needs the organisation logins to build its aliases, and
/// only GitHub knows them. The alternative -- nesting everything under
/// `viewer.organizations.nodes` in one request -- was measured and works
/// (cost 1, 0.91s) but was REJECTED: inside a `nodes` list, an
/// organisation refused by permission becomes an anonymous `null`, so the
/// UI cannot name what it failed to read and requirement 2 is
/// unimplementable. Two points and ~1.6s buys attributable failure.
///
/// # Bounded by the same wall clock as a stats load
///
/// `fetch::LOAD_TIMEOUT` wraps the WHOLE enumeration, the way `load_count`
/// and `load_detail` wrap theirs, and for the reason that constant's doc
/// gives: octocrab is `HandleRateLimits { max_retries: 3, min_wait_seconds:
/// 60 }`, so a single POST can legitimately block for minutes while every
/// layer above it waits.
///
/// Two requests inherit that hazard twice rather than dozens of times, so
/// 60s is generous here -- and it is deliberately the SAME constant rather
/// than a tighter one of its own. A second number would be a second thing
/// to justify and a second thing to drift, and the ceiling exists to
/// convert an unbounded hang into an actionable error rather than to
/// tighten a latency target (`commands.rs:604-633`). A tree that honestly
/// needs 20s on a slow link should draw, not fail.
pub async fn load_tree(client: &GitHubClient) -> Result<Tree, ClientError> {
    match tokio::time::timeout(super::fetch::LOAD_TIMEOUT, load_tree_inner(client)).await {
        Ok(r) => r,
        Err(_) => Err(ClientError::Timeout(super::fetch::LOAD_TIMEOUT.as_secs())),
    }
}

async fn load_tree_inner(client: &GitHubClient) -> Result<Tree, ClientError> {
    let budget = client.request_budget();

    // Through `fetch::metered_read`, which holds a process-wide permit and
    // records the spend -- so the sidebar tree counts against the same
    // `READ_CONCURRENCY` as everything else a Stats page render has in flight
    // (#844). It used to call `stats_graphql` directly and be invisible to
    // the cap.
    let orgs_v =
        super::fetch::metered_read(client, &budget, json!({ "query": orgs_query() })).await?;
    let mut refused = refused_fields_of(&orgs_v);

    // An ERROR, not a default. The viewer's login is the Personal section's
    // whole scope value: `Scope::Personal(login)` spells `user:<login>`, so
    // an empty string here would send `user:` to GitHub -- a malformed
    // qualifier that returns nothing, rendering a Personal section whose
    // "All repos" silently answers for nobody. There is no honest fallback
    // (the viewer cannot be guessed), and every other field in this tree is
    // optional in a way this one is not.
    let viewer = orgs_v["viewer"]["login"]
        .as_str()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| {
            ClientError::NotJson("GitHub returned no viewer login for the scope tree".into())
        })?
        .to_string();
    let orgs_conn = &orgs_v["viewer"]["organizations"];
    let orgs_total = orgs_conn["totalCount"].as_u64().unwrap_or(0);

    // Logins and the counts from step one, kept so a nulled alias in step
    // two can still be rendered with its name and its sizes. This is what
    // makes "4 members, not readable" possible instead of "no members".
    let listed: Vec<(String, Option<String>, u64, u64)> = orgs_conn["nodes"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|n| {
            let login = n["login"].as_str()?.to_string();
            Some((
                login,
                n["name"].as_str().map(str::to_string),
                n["membersWithRole"]["totalCount"].as_u64().unwrap_or(0),
                n["repositories"]["totalCount"].as_u64().unwrap_or(0),
            ))
        })
        .take(MAX_ORGS)
        .collect();

    // A viewer in no organisations still gets a tree: the Personal section
    // is the whole sidebar, and `tree_query` with no aliases is a valid
    // document that returns just the viewer's repositories. Skipping the
    // second request entirely would be a micro-optimisation that adds a
    // branch and a second code path for the empty case to be wrong in.
    let logins: Vec<String> = listed.iter().map(|(l, ..)| l.clone()).collect();
    let tree_v =
        super::fetch::metered_read(client, &budget, json!({ "query": tree_query(&logins) }))
            .await?;
    refused += refused_fields_of(&tree_v);

    let personal_conn = &tree_v["viewer"]["repositories"];
    let personal = repo_rows(&personal_conn["nodes"]);
    let personal_total = personal_conn["totalCount"]
        .as_u64()
        .unwrap_or(personal.len() as u64);

    let orgs = listed
        .iter()
        .enumerate()
        .map(|(i, (login, name, members_total, repos_total))| {
            let node = &tree_v[format!("o{i}")];
            // THE degradation branch. A null alias is an organisation the
            // token could list but not read -- not an empty organisation.
            // The counts and the name come from step one, so the row is
            // still informative: "FNX-Labs, 4 members, could not read".
            if node.is_null() {
                return OrgTree {
                    login: login.clone(),
                    name: name.clone(),
                    repos: Vec::new(),
                    repos_total: *repos_total,
                    members: Vec::new(),
                    members_total: *members_total,
                    readable: false,
                };
            }
            let repos = repo_rows(&node["repositories"]["nodes"]);
            let members: Vec<MemberRow> = node["membersWithRole"]["nodes"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .filter_map(|m| {
                    Some(MemberRow {
                        login: m["login"].as_str()?.to_string(),
                        name: m["name"].as_str().map(str::to_string),
                        avatar_url: m["avatarUrl"].as_str().map(str::to_string),
                    })
                })
                .collect();
            OrgTree {
                // Step two's login where present, so a rename between the
                // two requests surfaces as GitHub's current answer rather
                // than a stale one.
                login: node["login"].as_str().unwrap_or(login.as_str()).to_string(),
                name: node["name"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| name.clone()),
                repos_total: node["repositories"]["totalCount"]
                    .as_u64()
                    .unwrap_or(*repos_total),
                members_total: node["membersWithRole"]["totalCount"]
                    .as_u64()
                    .unwrap_or(*members_total),
                repos,
                members,
                readable: true,
            }
        })
        .collect();

    Ok(Tree {
        viewer,
        orgs,
        orgs_total,
        personal,
        personal_total,
        refused_fields: refused,
        spend: budget.snapshot(),
    })
}

/// Map a repository connection's nodes, KEEPING GitHub's order.
///
/// No sort here, and that is deliberate rather than an omission: the
/// `orderBy: PUSHED_AT` in the document already sorted them server-side
/// (VERIFIED monotonically descending on the live 49-repo response). A
/// client-side re-sort would be a second ordering rule that could disagree
/// with the first -- and it would also re-sort the `None` timestamps into
/// whatever the comparator happened to do with them.
fn repo_rows(v: &serde_json::Value) -> Vec<RepoRow> {
    v.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            Some(RepoRow {
                // A repository with no `nameWithOwner` is unusable as a
                // scope -- `Scope::Repo` is spelled `owner/name` and
                // `owner_name()` would return None -- so it is dropped
                // rather than carried as a row that cannot be clicked.
                name_with_owner: r["nameWithOwner"].as_str()?.to_string(),
                pushed_at: r["pushedAt"].as_str().map(str::to_string),
                is_archived: r["isArchived"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The aliases must be GraphQL-legal identifiers even though org
    /// logins are not.
    ///
    /// `FNX-Labs` is a real organisation on this account and a hyphen is
    /// illegal in an alias, so a document aliased by login would be a
    /// syntax error against the very account this feature was built for.
    #[test]
    fn org_aliases_are_identifiers_not_logins() {
        let q = tree_query(&["FNX-Labs".into(), "Stohic".into()]);
        assert!(q.contains("o0: organization(login: \"FNX-Labs\")"));
        assert!(q.contains("o1: organization(login: \"Stohic\")"));
        // The hyphenated login must never appear before the colon.
        assert!(!q.contains("FNX-Labs:"), "login used as an alias: {q}");
    }

    /// The ordering is requested SERVER-SIDE, on every repository
    /// connection in the document.
    ///
    /// Asserted per-connection rather than as a single `contains`, because
    /// the personal section and the org sections are built by different
    /// code paths and only one of them having the clause would leave half
    /// the sidebar alphabetical -- which is exactly the kind of half-fix
    /// that reads as working.
    #[test]
    fn every_repository_connection_is_ordered_by_recent_activity() {
        let q = tree_query(&["a".into(), "b".into()]);
        let ordered = q
            .matches("orderBy: {field: PUSHED_AT, direction: DESC}")
            .count();
        let repo_conns = q.matches("repositories(first:").count();
        assert_eq!(repo_conns, 3, "viewer + 2 orgs");
        assert_eq!(ordered, repo_conns, "every repo connection must be ordered");
    }

    /// `pushedAt` and `isArchived` are selected, because the rows cannot
    /// show recency or mark dead repositories without them -- and they are
    /// free, which is why there is no reason to omit them.
    #[test]
    fn the_free_activity_fields_are_selected() {
        let q = tree_query(&["a".into()]);
        assert!(q.contains("pushedAt"));
        assert!(q.contains("isArchived"));
    }

    /// The personal connection must be OWNER-affiliated, or the Personal
    /// section repeats every organisation repository.
    ///
    /// VERIFIED live: with both affiliation arguments, the viewer's
    /// connection returns 6 repositories, all owned by the viewer and none
    /// belonging to either organisation.
    #[test]
    fn personal_repositories_are_owned_not_merely_visible() {
        let q = tree_query(&[]);
        assert!(q.contains("affiliations: [OWNER]"));
        assert!(q.contains("ownerAffiliations: [OWNER]"));
    }

    /// A viewer in no organisations still produces a valid document with
    /// the viewer's own repositories in it.
    #[test]
    fn no_organisations_is_a_valid_document() {
        let q = tree_query(&[]);
        assert!(q.contains("repositories(first:"));
        assert!(!q.contains("organization(login:"));
        assert!(q.trim_end().ends_with('}'));
    }

    /// A quote or backslash in a login is DATA, not syntax.
    ///
    /// GitHub does not permit such logins; the escape exists so that a
    /// document this app builds by concatenation is well-formed on its own
    /// terms rather than relying on the server's validation rules never
    /// changing.
    #[test]
    fn a_login_cannot_break_out_of_its_string() {
        let q = tree_query(&["ev\"il".into()]);
        assert!(q.contains("\\\""), "quote was not escaped: {q}");
        // The document must still have exactly the one organisation.
        assert_eq!(q.matches("organization(login:").count(), 1);
    }

    /// `Spend` has no `Default` derive -- a spend is a MEASUREMENT, and a
    /// default-constructed one is a zero that was never measured. The
    /// tests below are about tree shape rather than cost, so they build an
    /// explicit empty spend and say that is what it is.
    fn no_spend() -> super::super::budget::Spend {
        super::super::budget::Spend {
            points: 0,
            requests: 0,
            unmetered: 0,
            remaining: None,
            reset_at: None,
        }
    }

    fn row(name: &str, archived: bool) -> RepoRow {
        RepoRow {
            name_with_owner: name.into(),
            pushed_at: Some("2026-09-01T00:00:00Z".into()),
            is_archived: archived,
        }
    }

    fn org(login: &str, repos: Vec<RepoRow>, repos_total: u64, readable: bool) -> OrgTree {
        OrgTree {
            login: login.into(),
            name: None,
            members: Vec::new(),
            members_total: 0,
            repos_total,
            repos,
            readable,
        }
    }

    /// #825 requirement 2, the #769 lesson, as a type-level distinction:
    /// an org that could not be read is NOT an org with no members.
    ///
    /// The assertion is on `readable` rather than on emptiness precisely
    /// because both have empty lists -- which is why a caller checking
    /// `members.is_empty()` would render the refused org as "no members"
    /// and why the flag has to exist.
    #[test]
    fn an_unreadable_org_is_distinguishable_from_an_empty_one() {
        let refused = org("secret", Vec::new(), 40, false);
        let genuinely_empty = OrgTree {
            members_total: 0,
            ..org("fresh", Vec::new(), 0, true)
        };
        assert!(refused.members.is_empty() && genuinely_empty.members.is_empty());
        assert!(!refused.readable);
        assert!(genuinely_empty.readable);
        // And the refused one still knows how big it is, so the row can
        // say "40 repositories, could not read" rather than nothing.
        assert_eq!(refused.repos_total, 40);
        assert!(refused.repos_truncated());
    }

    /// Archived repositories are LISTED but excluded from aggregates.
    /// Both halves asserted, because dropping them from the list is the
    /// obvious implementation and it is the one that loses a lead's
    /// ability to read a finished project's history.
    #[test]
    fn archived_repos_are_listed_but_not_aggregated() {
        let o = org(
            "acme",
            vec![row("acme/live", false), row("acme/dead", true)],
            2,
            true,
        );
        assert_eq!(o.repos.len(), 2, "archived repos stay in the list");
        assert_eq!(o.archived_count(), 1);
        let active: Vec<&str> = o
            .active_repos()
            .map(|r| r.name_with_owner.as_str())
            .collect();
        assert_eq!(active, vec!["acme/live"], "archived is out of aggregates");
    }

    /// Truncation is measured against `totalCount`, not against `PAGE`.
    ///
    /// An organisation holding exactly `PAGE` repositories is NOT
    /// truncated, and a `len() == PAGE` test would claim it was -- then
    /// the UI would show a "showing 100 of 100" warning that is simply
    /// false.
    #[test]
    fn truncation_compares_against_the_true_total() {
        let exactly_full = org(
            "full",
            (0..PAGE).map(|i| row(&format!("a/r{i}"), false)).collect(),
            PAGE as u64,
            true,
        );
        assert!(!exactly_full.repos_truncated());
        let over = org(
            "over",
            (0..PAGE).map(|i| row(&format!("a/r{i}"), false)).collect(),
            559,
            true,
        );
        assert!(over.repos_truncated());
    }

    /// `is_complete` is one question covering every partiality channel, so
    /// a view branches once. Each channel is asserted separately, since a
    /// condition dropped from the conjunction would otherwise be invisible.
    #[test]
    fn completeness_covers_every_partiality_channel() {
        let base = Tree {
            viewer: "me".into(),
            orgs: vec![org("a", vec![row("a/r", false)], 1, true)],
            orgs_total: 1,
            personal: vec![row("me/r", false)],
            personal_total: 1,
            refused_fields: 0,
            spend: no_spend(),
        };
        assert!(base.is_complete());

        assert!(!Tree {
            refused_fields: 1,
            ..base.clone()
        }
        .is_complete());
        assert!(!Tree {
            orgs_total: 2,
            ..base.clone()
        }
        .is_complete());
        assert!(!Tree {
            personal_total: 5,
            ..base.clone()
        }
        .is_complete());
        assert!(!Tree {
            orgs: vec![org("a", Vec::new(), 9, false)],
            ..base.clone()
        }
        .is_complete());
        assert!(!Tree {
            orgs: vec![org("a", vec![row("a/r", false)], 99, true)],
            ..base.clone()
        }
        .is_complete());
    }

    #[test]
    fn unreadable_orgs_are_enumerable_for_the_banner() {
        let t = Tree {
            viewer: "me".into(),
            orgs: vec![
                org("ok", vec![row("ok/r", false)], 1, true),
                org("nope", Vec::new(), 7, false),
            ],
            orgs_total: 2,
            personal: Vec::new(),
            personal_total: 0,
            refused_fields: 0,
            spend: no_spend(),
        };
        let bad: Vec<&str> = t.unreadable_orgs().map(|o| o.login.as_str()).collect();
        assert_eq!(bad, vec!["nope"]);
    }

    /// The mapper keeps GitHub's order and does not invent a timestamp.
    ///
    /// A repository that has never been pushed to must stay `None`:
    /// defaulting it to the epoch would sort right by accident and then
    /// render "last active 1970" for a repository created this morning.
    #[test]
    fn rows_keep_server_order_and_an_absent_timestamp() {
        let v = json!([
            { "nameWithOwner": "a/new", "pushedAt": "2026-09-11T00:00:00Z", "isArchived": false },
            { "nameWithOwner": "a/empty", "isArchived": false },
            { "nameWithOwner": "a/old", "pushedAt": "2023-01-01T00:00:00Z", "isArchived": true },
            // No nameWithOwner: unusable as a scope, so dropped.
            { "pushedAt": "2026-01-01T00:00:00Z" },
        ]);
        let rows = repo_rows(&v);
        assert_eq!(
            rows.iter()
                .map(|r| r.name_with_owner.as_str())
                .collect::<Vec<_>>(),
            vec!["a/new", "a/empty", "a/old"],
            "server order kept, unusable row dropped"
        );
        assert_eq!(rows[1].pushed_at, None);
        assert!(rows[2].is_archived);
    }

    /// An empty viewer login must not become `Scope::Personal("")`.
    ///
    /// The qualifier for a personal scope is `user:<login>`, so an empty
    /// login spells `user:` -- which GitHub answers with nothing rather than
    /// an error, rendering a Personal section whose "All repos" silently
    /// reports for nobody. `load_tree_inner` refuses instead, and this pins
    /// the reason by showing what the malformed scope would look like.
    #[test]
    fn an_empty_viewer_login_would_make_a_meaningless_personal_scope() {
        let bad = super::super::scope::Scope::Personal(String::new());
        assert_eq!(bad.qualifier(), "user:");
        // Which is why the loader treats an absent login as an error rather
        // than defaulting it. A good login gives a usable qualifier.
        let good = super::super::scope::Scope::Personal("octocat".into());
        assert_eq!(good.qualifier(), "user:octocat");
    }

    /// A row's identity is exactly what `Scope::Repo` consumes, so a click
    /// needs no reassembly.
    #[test]
    fn a_repo_row_feeds_scope_repo_directly() {
        let rows = repo_rows(&json!([
            { "nameWithOwner": "pktstorm/headstate", "pushedAt": "2026-09-11T00:00:00Z", "isArchived": false }
        ]));
        let scope = super::super::scope::Scope::Repo(rows[0].name_with_owner.clone());
        assert_eq!(scope.owner_name(), Some(("pktstorm", "headstate")));
        assert!(!scope.needs_search(), "a repo scope uses the connection");
    }

    /// The tree must carry NO statistics. Asserted on the document itself,
    /// because the cheap-discovery guarantee is a property of what is
    /// asked, and a later "while we are here" addition of a count is
    /// exactly how a 1-point sidebar becomes a 49-search one.
    #[test]
    fn the_tree_query_asks_for_no_statistics() {
        let q = tree_query(&["a".into()]);
        for forbidden in [
            "pullRequests",
            "search",
            "issueCount",
            "additions",
            "deletions",
            "reviews",
            "contributions",
        ] {
            assert!(
                !q.contains(forbidden),
                "discovery must stay cheap: found {forbidden} in the tree query"
            );
        }
        // And it must meter itself, like every query in this layer.
        assert!(q.contains("rateLimit { cost remaining resetAt }"));
    }

    #[test]
    fn the_orgs_query_also_meters_and_asks_for_no_statistics() {
        let q = orgs_query();
        assert!(q.contains("rateLimit { cost remaining resetAt }"));
        for forbidden in ["pullRequests", "search", "issueCount", "additions"] {
            assert!(!q.contains(forbidden));
        }
    }

    /// The alias count is bounded by what the QUERY asks for, not merely by
    /// the constant -- `MAX_ORGS` is only a real limit if the document and
    /// the slice agree on it.
    ///
    /// Asserted against `orgs_query`'s own page size rather than as a bare
    /// comparison on the constant (which clippy correctly calls a constant
    /// assertion, since it cannot fail at runtime): the document requests
    /// `MAX_ORGS` organisations, so a larger constant could never be
    /// reached and a smaller one would silently drop organisations GitHub
    /// did return. The two numbers have to agree, and this is what says so.
    #[test]
    fn the_org_alias_count_is_bounded_by_what_the_query_asks_for() {
        assert!(orgs_query().contains(&format!("organizations(first: {MAX_ORGS})")));
        let many: Vec<String> = (0..MAX_ORGS).map(|i| format!("o{i}")).collect();
        let q = tree_query(&many);
        assert_eq!(q.matches("organization(login:").count(), MAX_ORGS);
    }
}
