//! WHO and WHERE a stats question is about.
//!
//! Every stats query in `query.rs` is hardcoded `author:@me` -- see
//! `STATS_QUERY` (`query.rs:155-159`), `history_query_range`
//! (`:232-246`), `periods_query` (`:280-297`) and `cycle_trend_query`
//! (`:331-346`). That was right while the page answered exactly one
//! question ("how am I doing?"), and it is the single thing blocking the
//! second audience #823 names ("how is my team doing?"): there is no way
//! to ask about anyone else.
//!
//! This module is that parameter, and nothing more. No UI, no sidebar --
//! #825 builds the hierarchy that chooses a [`Scope`], #826 the views
//! that render the answers. What lands here is the part both need and
//! neither should reinvent: one place that turns a subject and a scope
//! into a GitHub search qualifier string, tested against the real
//! qualifier grammar.
//!
//! # Why a type and not a format string
//!
//! The qualifier is assembled in exactly one function, [`Scope::qualifier`],
//! because a stats answer is only comparable across slices if every slice
//! asked the same question. The adaptive slicer in `slice.rs` issues
//! dozens of searches per load and sums their counts; a subject spelled
//! `author:x` in one slice and `involves:x` in another would produce a
//! total that is not a count of anything. A string built per call site is
//! exactly how that drift happens, so there are no per-call-site strings.

use std::fmt;

/// Whose activity a stats question is about.
///
/// `@me` is kept as a distinct variant rather than resolved to a login at
/// construction. GitHub resolves `@me` server-side against the token, so
/// it is correct without a round trip -- `fetch_viewer` exists
/// (`client.rs:889-894`) but costs a request, and the viewer's own stats
/// are the common case. Resolving eagerly would put a network call in
/// front of the cheapest path in the feature.
///
/// The cost of keeping it: `@me` is not a usable CACHE KEY, because two
/// tokens resolve it differently. [`Subject::cache_key`] is therefore a
/// separate method from [`Subject::qualifier_value`], and it takes the
/// viewer's login to substitute. Persisting a row keyed on the literal
/// `@me` would serve one user's leaderboard to another on a shared
/// machine, which is why the two are not the same string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subject {
    /// The authenticated user, resolved by GitHub from the token.
    Viewer,
    /// A named login -- the case that did not exist before #824.
    Login(String),
}

impl Subject {
    /// What GitHub should be asked about.
    pub fn qualifier_value(&self) -> &str {
        match self {
            Subject::Viewer => "@me",
            Subject::Login(l) => l,
        }
    }

    /// A stable identity for persistence, with `@me` resolved.
    ///
    /// `viewer_login` is the value `fetch_viewer` returned for the
    /// current token. Two different accounts on one machine share a
    /// database file, so a cached row keyed on the literal `@me` would
    /// be served to whichever of them asked second -- a leaderboard
    /// showing someone else's numbers under your name. The resolved
    /// login makes those two rows distinct.
    pub fn cache_key(&self, viewer_login: &str) -> String {
        match self {
            Subject::Viewer => viewer_login.to_string(),
            Subject::Login(l) => l.clone(),
        }
    }

    /// Whether this subject names a specific person.
    ///
    /// A leaderboard asks "who, among everyone here" and must NOT carry
    /// an author qualifier at all -- `Subject::Anyone` would be a fifth
    /// variant that every call site had to remember to handle. Instead
    /// the absence is modelled by the caller passing `None`, and this
    /// exists so a caller can assert it did.
    pub fn is_named(&self) -> bool {
        matches!(self, Subject::Login(_))
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.qualifier_value())
    }
}

/// WHERE to look: which repositories a stats question covers.
///
/// The variants are not interchangeable ways of saying the same thing --
/// they select different GitHub APIs. [`Scope::Repo`] can be answered by
/// `repository.pullRequests`, a connection with no 1,000-result cap
/// (VERIFIED live: pktstorm/headstate reports 337 merged PRs at cost 1).
/// Every other variant spans repositories and therefore needs `search`,
/// which caps at 1,000 retrievable results while `issueCount` reports the
/// true total. [`Scope::needs_search`] is where that routing decision
/// lives, and `fetch.rs` is the only caller.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// One repository, `owner/name`. The only scope a connection can
    /// answer exactly.
    Repo(String),
    /// Every repository in one organisation.
    Org(String),
    /// The subject's own repositories, outside any organisation.
    ///
    /// Spelled `user:<login>` rather than left unqualified: an
    /// unqualified search covers every repository the token can see,
    /// which is a different and much larger question.
    Personal(String),
    /// The viewer's whole account: every organisation they belong to,
    /// plus their own repositories.
    ///
    /// # Why the members are CARRIED rather than left implicit
    ///
    /// This used to be a unit variant producing an EMPTY qualifier, on
    /// the reasoning that GitHub reads an absent repository qualifier as
    /// "everywhere the token can see". **It does not.** An absent
    /// qualifier searches the whole of GitHub: measured live on
    /// 2026-09-17, `is:pr is:merged merged:2026-09-10..2026-09-10`
    /// reports **606,016** pull requests, and the five-slice document the
    /// backfill sends returns **HTTP 502** every time (#1114).
    ///
    /// The same reasoning `Personal` already carries, one scope along:
    /// an unqualified search is a different and much larger question.
    ///
    /// Carrying the members makes the empty qualifier unrepresentable
    /// rather than merely discouraged, which is why this is a tuple
    /// variant and not a flag.
    All(AccountScope),
}

/// How many organisations one "Everything" search may name.
///
/// GitHub's search qualifier string has a practical length limit, and the
/// sidebar already caps its own organisation list at 50 and says so. This
/// is the same honesty one layer down: a viewer in more organisations than
/// this gets a search over the first `ORG_UNION_CAP`, and
/// [`AccountScope::trimmed`] reports that it happened so the page can say
/// the answer is a floor rather than a total.
pub const ORG_UNION_CAP: usize = 20;

/// The organisations and login that "Everything" spans.
///
/// A named type rather than a bare `Vec<String>` so the empty case has one
/// meaning everywhere: a viewer in NO organisations is their own
/// repositories, `user:<login>`, never an unqualified search.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AccountScope {
    /// The viewer's login. Always present, so the qualifier can never be
    /// empty even for a viewer with no organisations.
    pub login: String,
    /// Organisations the viewer belongs to, capped at [`ORG_UNION_CAP`].
    pub orgs: Vec<String>,
    /// Organisations beyond the cap that this search does NOT cover.
    ///
    /// Counted rather than dropped silently: a board over 20 of a
    /// viewer's 30 organisations is a floor, and a floor that does not
    /// say so is the lie this codebase's rules exist to prevent.
    pub trimmed: usize,
}

impl AccountScope {
    /// Parse the wire spelling: `login` then comma-separated organisations.
    ///
    /// Carried in `scope_value`, which is unused for this kind and is
    /// already persisted by `pr_backfill_scope` and round-tripped by
    /// `query_for` -- so the background worker reconstructs the SAME union
    /// the click made rather than re-deriving it by a second route that
    /// could disagree. That round trip is why this is a string rather than
    /// a richer payload.
    ///
    /// An empty or absent value yields the login alone, which is a
    /// correct, narrow, NON-empty qualifier. A viewer in no organisations
    /// and a value that failed to arrive are the same search here, and
    /// both are safe: the failure mode is a board that is too small, never
    /// one that searches the planet.
    /// Spelled `login,orgA,orgB`. The LOGIN LEADS so the value is
    /// self-contained: `query_for` reconstructs the union from the stored
    /// row alone, with no viewer lookup and no second request from a
    /// background worker nobody is waiting for.
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string);
        // No login means no searchable union. `None` rather than a
        // default: a caller that cannot name the viewer must not fall
        // back to an unqualified search, which is the whole of #1114.
        let login = parts.next()?;
        Some(Self::new(login, parts.collect()))
    }

    /// The wire spelling, for `scope_value`.
    pub fn to_value(&self) -> String {
        let mut v = String::from(&self.login);
        for o in &self.orgs {
            v.push(',');
            v.push_str(o);
        }
        v
    }

    /// Build from the viewer's login and the organisations they belong to.
    pub fn new(login: impl Into<String>, orgs: Vec<String>) -> Self {
        let login = login.into();
        let trimmed = orgs.len().saturating_sub(ORG_UNION_CAP);
        let orgs = orgs.into_iter().take(ORG_UNION_CAP).collect();
        Self {
            login,
            orgs,
            trimmed,
        }
    }

    /// `org:A org:B user:login` -- the union, as GitHub search spells it.
    ///
    /// Multiple `org:` qualifiers OR together, so this is ONE search over
    /// the union rather than one search per organisation. Verified live on
    /// 2026-09-17 at the five-slice document shape that returns HTTP 502
    /// unqualified: `org:FNX-Labs org:Stohic org:StormKiln user:pktstorm`
    /// answered all five slices for 1 point.
    ///
    /// `user:` is included so a viewer's own repositories are part of
    /// "Everything", which is what distinguishes this row from the
    /// Personal one BELOW it in the sidebar rather than duplicating it.
    pub fn qualifier(&self) -> String {
        let mut parts: Vec<String> = self.orgs.iter().map(|o| format!("org:{o}")).collect();
        parts.push(format!("user:{}", self.login));
        parts.join(" ")
    }
}

impl Scope {
    /// The repository-selecting half of a search query.
    ///
    /// Empty for [`Scope::All`], which is the correct spelling: GitHub
    /// reads an absent repository qualifier as "everywhere the token can
    /// see". An explicit wildcard is not available and inventing one
    /// (`repo:*`) returns nothing at all.
    pub fn qualifier(&self) -> String {
        match self {
            Scope::Repo(r) => format!("repo:{r}"),
            Scope::Org(o) => format!("org:{o}"),
            Scope::Personal(u) => format!("user:{u}"),
            Scope::All(a) => a.qualifier(),
        }
    }

    /// Whether answering this scope requires `search` rather than a
    /// connection.
    ///
    /// This is the completeness decision, not an optimisation. A search
    /// is capped at 1,000 retrievable results and reports a truthful
    /// `issueCount` above it -- VERIFIED live: `is:pr org:FNX-Labs`
    /// reports 1,822 and paginating past offset 1000 returns `[]` with
    /// no error at all. A connection has no such cap. So a single-repo
    /// scope answered through search would be needlessly exposed to a
    /// silent truncation that the connection simply does not have.
    ///
    /// `true` here is what obliges the caller to run the probe-driven
    /// slicer in `slice.rs`; `false` is what lets it skip it.
    pub fn needs_search(&self) -> bool {
        !matches!(self, Scope::Repo(_))
    }

    /// `owner` and `name` for a single-repo scope, for the connection
    /// query's variables.
    ///
    /// `None` for every other variant rather than a panic: the routing
    /// decision and the split are made by the same caller, and a scope
    /// that cannot be split is exactly the scope that must use search.
    pub fn owner_name(&self) -> Option<(&str, &str)> {
        match self {
            Scope::Repo(r) => r.split_once('/'),
            _ => None,
        }
    }

    /// A stable identity for persistence.
    ///
    /// Prefixed by variant so `Org("a")` and `Personal("a")` cannot
    /// collide on one cache row. They are genuinely different questions
    /// -- an organisation's repositories and a user's own -- and a
    /// leaderboard computed for one must never be served for the other.
    pub fn cache_key(&self) -> String {
        match self {
            Scope::Repo(r) => format!("repo:{r}"),
            Scope::Org(o) => format!("org:{o}"),
            Scope::Personal(u) => format!("user:{u}"),
            // The KEY stays "all", with no members in it. It is the
            // cache and ledger key, and folding a live org list into it
            // would invalidate every stored row the moment the viewer
            // joined or left an organisation -- discarding measured data
            // over a membership change that does not alter who merged
            // what in the past.
            Scope::All(_) => "all".to_string(),
        }
    }
}

/// What kind of PR activity is being counted.
///
/// The date qualifier differs per measure and is NOT interchangeable: a
/// PR opened in July and merged in August belongs to July's opened count
/// and August's merged count. The original code had this right for one
/// subject (`history_query_range` uses `merged:` and `created:` on
/// separate aliases) and this carries the distinction into the
/// parameterised layer rather than letting a caller pick a qualifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Measure {
    /// PRs merged in the window, by merge date.
    Merged,
    /// PRs opened in the window, by creation date.
    Opened,
}

impl Measure {
    /// The state and date qualifiers for this measure.
    ///
    /// `is:merged` rides along with `merged:` because a `merged:` range
    /// already implies it -- but stating it keeps the query readable in a
    /// log and matches what `history_query_range` has always sent.
    fn qualifiers(self) -> (&'static str, &'static str) {
        match self {
            Measure::Merged => ("is:merged ", "merged:"),
            Measure::Opened => ("", "created:"),
        }
    }
}

/// One fully-specified stats question: who, where, what, and when.
///
/// Constructed once per load and cloned into every slice, so the slicer
/// cannot accidentally vary the subject between slices it is going to
/// sum. See the module docs for why that matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsQuery {
    /// Whose PRs, or `None` for "anyone in scope".
    ///
    /// `None` is what a leaderboard asks: it counts every author in the
    /// scope and then ranks them, so constraining to one author would
    /// return a one-row board. Modelled as an absent subject rather than
    /// a `Subject::Anyone` variant so that the two cache-key methods on
    /// [`Subject`] never have to answer "the cache key of everyone".
    pub subject: Option<Subject>,
    pub scope: Scope,
    pub measure: Measure,
}

impl StatsQuery {
    pub fn new(subject: Option<Subject>, scope: Scope, measure: Measure) -> Self {
        Self {
            subject,
            scope,
            measure,
        }
    }

    /// The same question about the same scope, for a different measure.
    ///
    /// The scoped daily series (`query::series_query`) needs a merged
    /// count and an opened count for every day, which are two measures
    /// over one subject and scope. Without this, that document would
    /// assemble its own qualifier strings per alias -- and the module
    /// docs above forbid exactly that, because a subject spelled
    /// differently between two aliases produces a total that is not a
    /// count of anything. Clone-and-replace keeps the qualifier builder
    /// the only place a query string is made.
    pub fn with_measure(&self, measure: Measure) -> Self {
        Self {
            subject: self.subject.clone(),
            scope: self.scope.clone(),
            measure,
        }
    }

    /// The GitHub search query for one date range.
    ///
    /// `from` and `to` are inclusive `YYYY-MM-DD`, matching the format
    /// `query::period_ranges` already produces and GitHub's own `..`
    /// range grammar.
    ///
    /// Qualifier ORDER is fixed -- type, state, author, repo, date --
    /// even though GitHub does not care. Two reasons, both about humans:
    /// a diagnostic log of thirty slices is readable when the only part
    /// that varies is the tail, and the tests below can assert on a whole
    /// string rather than on the presence of substrings in any order.
    pub fn search_query(&self, from: &str, to: &str) -> String {
        let (state, date) = self.measure.qualifiers();
        let author = match &self.subject {
            Some(s) => format!("author:{} ", s.qualifier_value()),
            None => String::new(),
        };
        let scope = self.scope.qualifier();
        let scope = if scope.is_empty() {
            String::new()
        } else {
            format!("{scope} ")
        };
        format!("is:pr {state}{author}{scope}{date}{from}..{to}")
    }

    /// The same question, asked about who REVIEWED rather than who wrote
    /// (#826's reviewer leaderboard).
    ///
    /// `reviewed-by:<login>` in place of the author qualifier, which is the
    /// one qualifier swap that changes what a row MEANS rather than which
    /// rows there are. The state, scope and date halves come from
    /// [`Measure::qualifiers`] and [`Scope::qualifier`] exactly as
    /// [`StatsQuery::search_query`] builds them, so a reviewer count and an
    /// author count over the same window cannot disagree about what the
    /// window or the scope is -- the failure this module's docs exist to
    /// prevent.
    ///
    /// # Why the author qualifier is DROPPED rather than kept
    ///
    /// `author:X reviewed-by:Y` is a perfectly legal search, and it means "X's
    /// pull requests that Y reviewed" -- a third question, and not the one a
    /// reviewer leaderboard asks. A leaderboard of reviews given must count
    /// every pull request in the scope that this person reviewed, whoever
    /// wrote it; the measured proof that those differ is in
    /// `query::reviewer_query`'s docs, where the two pull requests crediting
    /// `pktstorm` as reviewer were both AUTHORED by someone else. Carrying a
    /// stale `author:@me` through would have returned the viewer's own pull
    /// requests that the named person reviewed, which on a board titled "top
    /// reviewers" would be wrong in a way no figure on screen could reveal.
    ///
    /// So the author is dropped unconditionally and this takes `&self`
    /// rather than requiring the caller to build a subject-less query first:
    /// a caller that forgot would get the three-qualifier search, and this is
    /// the one place that mistake can be made impossible.
    pub fn reviewed_by(&self, reviewer: &str, from: &str, to: &str) -> String {
        let (state, date) = self.measure.qualifiers();
        let scope = self.scope.qualifier();
        let scope = if scope.is_empty() {
            String::new()
        } else {
            format!("{scope} ")
        };
        // Qualifier ORDER matches `search_query`'s -- type, state, who,
        // repo, date -- for the same two human reasons it gives: a log of
        // thirty reviewer aliases is readable when only the name varies, and
        // a test can assert on the whole string.
        format!("is:pr {state}reviewed-by:{reviewer} {scope}{date}{from}..{to}")
    }

    /// A stable identity for persistence, with `@me` resolved.
    ///
    /// Includes the measure: a merged count and an opened count over the
    /// same window are different numbers, and a key that omitted it
    /// would serve one as the other.
    pub fn cache_key(&self, viewer_login: &str) -> String {
        let who = match &self.subject {
            Some(s) => s.cache_key(viewer_login),
            None => "*".to_string(),
        };
        let what = match self.measure {
            Measure::Merged => "merged",
            Measure::Opened => "opened",
        };
        format!("{what}|{who}|{}", self.scope.cache_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of #824's item 7: a subject who is NOT the viewer.
    ///
    /// Asserted on the FULL query string rather than on a substring,
    /// because the bug this guards is a stray `author:@me` surviving
    /// alongside the named author -- which a `contains("author:octocat")`
    /// check would pass happily while the query returned the wrong
    /// person's PRs, or nobody's.
    #[test]
    fn a_named_subject_replaces_the_viewer_entirely() {
        let q = StatsQuery::new(
            Some(Subject::Login("octocat".into())),
            Scope::Org("FNX-Labs".into()),
            Measure::Merged,
        );
        assert_eq!(
            q.search_query("2026-07-01", "2026-07-31"),
            "is:pr is:merged author:octocat org:FNX-Labs merged:2026-07-01..2026-07-31"
        );
        assert!(
            !q.search_query("2026-07-01", "2026-07-31").contains("@me"),
            "a named subject must not carry the viewer qualifier too"
        );
    }

    /// `reviewed_by` replaces the author, never joins it.
    ///
    /// The mistake this guards is specific and silent: `author:@me
    /// reviewed-by:octocat` is a LEGAL search meaning "my pull requests that
    /// octocat reviewed". A board titled "top reviewers" built on it would
    /// rank colleagues by how much of the VIEWER'S work they reviewed, and
    /// nothing on the page could reveal it -- the numbers would be plausible
    /// and just about the wrong population.
    ///
    /// Asserted on the full string for the same reason the test above is: a
    /// `contains("reviewed-by:octocat")` check passes while the author
    /// qualifier is still there.
    #[test]
    fn reviewed_by_replaces_the_author_rather_than_joining_it() {
        let q = StatsQuery::new(
            Some(Subject::Viewer),
            Scope::Org("FNX-Labs".into()),
            Measure::Merged,
        );
        assert_eq!(
            q.reviewed_by("octocat", "2026-08-12", "2026-09-10"),
            "is:pr is:merged reviewed-by:octocat org:FNX-Labs merged:2026-08-12..2026-09-10"
        );
        assert!(
            !q.reviewed_by("octocat", "2026-08-12", "2026-09-10")
                .contains("author:"),
            "an author qualifier alongside reviewed-by asks a third question"
        );
    }

    /// The reviewer and author searches agree on everything EXCEPT who.
    ///
    /// Both come from the same `Measure::qualifiers` and `Scope::qualifier`,
    /// which is this module's whole rule -- two searches that spelled the
    /// window or the scope differently would produce boards a reader cannot
    /// compare. Asserted by diffing the two strings rather than by re-reading
    /// the builders, so a future change to either qualifier has to keep them
    /// in step.
    #[test]
    fn the_reviewer_search_differs_from_the_author_search_only_in_who() {
        let q = StatsQuery::new(
            Some(Subject::Login("octocat".into())),
            Scope::Org("FNX-Labs".into()),
            Measure::Merged,
        );
        let authored = q.search_query("2026-08-12", "2026-09-10");
        let reviewed = q.reviewed_by("octocat", "2026-08-12", "2026-09-10");
        assert_eq!(
            authored.replace("author:octocat", "reviewed-by:octocat"),
            reviewed,
            "the two searches must differ only in the who qualifier"
        );
    }

    /// The viewer still spells as `@me`, so the cheapest and most common
    /// path needs no `fetch_viewer` round trip.
    ///
    /// Asserted over a NAMED scope. This test used `Scope::All` when that
    /// variant contributed no qualifier, which made the expected string
    /// shorter but coupled a test about the SUBJECT to the scope that was
    /// silently unqualified -- so when `Scope::All` was fixed to name the
    /// account (#1114), a test about `@me` failed. The subject is what is
    /// under test here, and an org scope states that without borrowing
    /// another scope's emptiness.
    #[test]
    fn the_viewer_still_resolves_server_side() {
        let q = StatsQuery::new(
            Some(Subject::Viewer),
            Scope::Org("acme".into()),
            Measure::Merged,
        );
        assert_eq!(
            q.search_query("2026-01-01", "2026-01-31"),
            "is:pr is:merged author:@me org:acme merged:2026-01-01..2026-01-31"
        );
    }

    /// A leaderboard counts EVERYONE, so it carries no author qualifier.
    ///
    /// The failure this guards is subtle and would not look like a bug:
    /// a leaderboard that kept `author:@me` would render a board with
    /// exactly one name on it -- the viewer's, in first place.
    #[test]
    fn no_subject_means_no_author_qualifier() {
        let q = StatsQuery::new(None, Scope::Org("Stohic".into()), Measure::Opened);
        let s = q.search_query("2026-08-01", "2026-08-31");
        assert_eq!(s, "is:pr org:Stohic created:2026-08-01..2026-08-31");
        assert!(!s.contains("author:"), "a leaderboard ranks every author");
    }

    /// Opened and merged use DIFFERENT date fields. A PR opened in July
    /// and merged in August belongs to July's opened count and August's
    /// merged count, so sharing one qualifier would silently conflate
    /// two different measures.
    #[test]
    fn opened_and_merged_use_different_date_fields() {
        let scope = Scope::Repo("pktstorm/headstate".into());
        let merged = StatsQuery::new(Some(Subject::Viewer), scope.clone(), Measure::Merged)
            .search_query("2026-08-01", "2026-08-31");
        let opened = StatsQuery::new(Some(Subject::Viewer), scope, Measure::Opened)
            .search_query("2026-08-01", "2026-08-31");
        assert!(merged.contains("merged:2026-08-01..2026-08-31"));
        assert!(merged.contains("is:merged"));
        assert!(opened.contains("created:2026-08-01..2026-08-31"));
        // An opened count must NOT be restricted to merged PRs, or it
        // would under-report every PR still open -- which is most of
        // them in a recent window.
        assert!(!opened.contains("is:merged"));
    }

    /// **The widest scope is still SCOPED.**
    ///
    /// This test used to assert the opposite -- that `Scope::All` carried
    /// no qualifier at all -- on the belief that GitHub reads an absent
    /// repository qualifier as "everywhere the token can see". It does
    /// not. Measured live on 2026-09-17:
    ///
    /// | query | result |
    /// |---|---|
    /// | `is:pr is:merged merged:2026-09-10..2026-09-10` | **606,016** pull requests |
    /// | the same, as the backfill's 5-slice document | **HTTP 502**, every tick |
    ///
    /// The old test passed throughout, because "no wildcard was invented"
    /// was true and was never the question. Nothing asserted the search
    /// returned the VIEWER's pull requests rather than the planet's, so
    /// #1114 shipped under a green suite.
    #[test]
    fn the_widest_scope_names_the_account_rather_than_the_whole_of_github() {
        let all = Scope::All(AccountScope::new(
            "octocat",
            vec!["acme".into(), "beta".into()],
        ));
        let q = StatsQuery::new(None, all, Measure::Merged);
        let s = q.search_query("2026-01-01", "2026-01-02");

        assert!(s.contains("org:acme"), "the union must name each org: {s}");
        assert!(s.contains("org:beta"), "the union must name each org: {s}");
        assert!(
            s.contains("user:octocat"),
            "the viewer's own repositories are part of their account: {s}"
        );
        assert!(!s.contains('*'), "no invented wildcard: {s}");
        assert!(!s.contains("  "), "a qualifier left a double space: {s}");
    }

    /// A viewer in NO organisations still gets a scoped search.
    ///
    /// The empty-union case is the one that would quietly reintroduce the
    /// defect: joining zero organisations must narrow to the viewer's own
    /// repositories, never widen to everything.
    #[test]
    fn an_account_with_no_organisations_is_still_narrowed_to_its_viewer() {
        let all = Scope::All(AccountScope::new("octocat", Vec::new()));
        assert_eq!(all.qualifier(), "user:octocat");
        assert!(!all.qualifier().is_empty(), "never an unqualified search");
    }

    /// The union round trips through `scope_value`, so the background
    /// worker rebuilds the SAME question the click asked.
    #[test]
    fn the_account_union_round_trips_through_its_stored_value() {
        let built = AccountScope::new("octocat", vec!["acme".into(), "beta".into()]);
        let parsed = AccountScope::parse(&built.to_value()).expect("a value naming a viewer");
        assert_eq!(parsed, built);
        assert_eq!(parsed.qualifier(), "org:acme org:beta user:octocat");
    }

    /// A value that names no viewer yields `None` rather than a default.
    ///
    /// The caller then SKIPS the row. Defaulting here would put an
    /// unqualified search back exactly where it was.
    #[test]
    fn a_value_that_names_no_viewer_is_refused_rather_than_defaulted() {
        assert!(AccountScope::parse("").is_none());
        assert!(AccountScope::parse("  ,  ").is_none());
    }

    /// Beyond the cap the union is TRIMMED and says by how much.
    ///
    /// A board over 20 of a viewer's 30 organisations is a floor, and a
    /// floor that does not say so is the lie this codebase exists to
    /// prevent.
    #[test]
    fn an_over_long_union_is_trimmed_and_counted() {
        let orgs: Vec<String> = (0..ORG_UNION_CAP + 5).map(|i| format!("o{i}")).collect();
        let a = AccountScope::new("octocat", orgs);
        assert_eq!(a.orgs.len(), ORG_UNION_CAP);
        assert_eq!(a.trimmed, 5, "the organisations left out must be counted");
    }

    /// Item 5 of #824, as a routing decision: only a single-repo scope
    /// can use the uncapped connection. Every other scope spans
    /// repositories and must go through search, which is capped.
    #[test]
    fn only_a_single_repo_scope_avoids_search() {
        assert!(!Scope::Repo("a/b".into()).needs_search());
        assert!(Scope::Org("a".into()).needs_search());
        assert!(Scope::Personal("a".into()).needs_search());
        assert!(Scope::All(AccountScope::new("octocat", Vec::new())).needs_search());
    }

    #[test]
    fn a_repo_scope_splits_for_the_connection_query() {
        assert_eq!(
            Scope::Repo("pktstorm/headstate".into()).owner_name(),
            Some(("pktstorm", "headstate"))
        );
        // A malformed value yields None rather than a panic, and the
        // caller falls back to search -- which still answers correctly.
        assert_eq!(Scope::Repo("nameless".into()).owner_name(), None);
        assert_eq!(Scope::Org("FNX-Labs".into()).owner_name(), None);
    }

    /// `@me` is not a cache key: two tokens resolve it to two people.
    /// Keying a persisted leaderboard on the literal would serve one
    /// user's numbers to another on a shared machine.
    #[test]
    fn the_cache_key_resolves_the_viewer_to_a_login() {
        let q = StatsQuery::new(
            Some(Subject::Viewer),
            Scope::Org("Stohic".into()),
            Measure::Merged,
        );
        assert_eq!(q.cache_key("pktstorm"), "merged|pktstorm|org:Stohic");
        assert!(!q.cache_key("pktstorm").contains("@me"));
        // Two viewers, two different rows.
        assert_ne!(q.cache_key("pktstorm"), q.cache_key("octocat"));
    }

    /// An org and a user with the same name are different questions. A
    /// key that collapsed them would serve an organisation's leaderboard
    /// as a person's.
    #[test]
    fn org_and_personal_scopes_do_not_collide_on_one_cache_row() {
        let o = StatsQuery::new(None, Scope::Org("acme".into()), Measure::Merged);
        let p = StatsQuery::new(None, Scope::Personal("acme".into()), Measure::Merged);
        assert_ne!(o.cache_key("me"), p.cache_key("me"));
    }

    /// The measure is part of the key. A merged count and an opened
    /// count over one window are different numbers.
    #[test]
    fn the_measure_is_part_of_the_cache_key() {
        let scope = Scope::Org("Stohic".into());
        let m = StatsQuery::new(None, scope.clone(), Measure::Merged);
        let o = StatsQuery::new(None, scope, Measure::Opened);
        assert_ne!(m.cache_key("me"), o.cache_key("me"));
    }

    #[test]
    fn a_named_subject_is_distinguishable_from_the_viewer() {
        assert!(Subject::Login("octocat".into()).is_named());
        assert!(!Subject::Viewer.is_named());
    }
}
