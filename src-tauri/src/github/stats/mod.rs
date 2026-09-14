//! The PR Stats query layer: parameterised by subject and scope, metered,
//! bounded, and complete.
//!
//! This module ships NO user-visible feature. It is the layer #825's
//! sidebar and #826's views sit on, hardened first and deliberately
//! alone, because every protection in it is one whose absence has already
//! caused a shipped bug here:
//!
//! | Protection | The bug its absence caused |
//! |---|---|
//! | Wall-clock timeout | #790 -- `get_pr_detail` had none; one POST can block for minutes |
//! | Budget metering | none existed; `cost` was requested on three queries and read nowhere |
//! | Read-concurrency cap | only MUTATIONS were capped (`commands.rs:437-444`) |
//! | Visible truncation | #802 and #790 both shipped silent truncation |
//!
//! # The shape of a load
//!
//! 0. [`tree`] enumerates what scopes EXIST -- the organisations,
//!    repositories and people a question can be asked about (#825). It
//!    fetches no statistics at all: two requests, one rate-limit point
//!    each, measured. Discovery is cheap and measurement is separate
//!    (`hooks.ts:712-717`), so entering the view costs 2 points and
//!    clicking is what spends.
//! 1. [`scope`] turns a subject and a scope into a search qualifier. Every
//!    stats query before this was hardcoded `author:@me`, which is the one
//!    thing blocking the second audience #823 names.
//! 2. [`fetch`] routes: a single-repo scope goes to
//!    `repository.pullRequests`, a connection with no 1,000-result cap.
//!    Everything else must use `search`, which has one.
//! 3. [`slice`] probes `issueCount` and subdivides until every slice is
//!    under the cap. This is the completeness mechanism, and it is
//!    probe-driven rather than calendar-driven because a calendar grid
//!    measurably does not work.
//! 4. [`budget`] accumulates what the whole load cost, read off
//!    `rateLimit` on every request rather than assumed.
//! 5. [`board`] maps the nodes to people: per-author aggregates, the two
//!    views (#826's Mine and Others) and the three leaderboards. It owns
//!    the mapping, which is why it also owns the refusal attribution
//!    `fetch::Outcome` could not do.
//!
//! # The one number worth knowing before editing any document here
//!
//! Cost is driven by connections that PAGE, not by the number of searches,
//! and a connection nested inside another is invisible to a substring
//! count of its parent. `poll.rs:1503-1580` is the richest record of that
//! in this repo and is worth reading in full before adding a field.
//!
//! The precise rule, MEASURED for #826 and narrower than what #823 and
//! #827 believed: it is the `first:` ARGUMENT that is priced, not the
//! connection. `additions`, `deletions` and `changedFiles` are free
//! scalars; `reviews { totalCount }` is ALSO free (1 point at 3, 6 and 15
//! searches, tracking the scalars-only control exactly), while
//! `reviews(first: 1) { totalCount }` costs 2 from 6 searches up. `labels`
//! behaves the same both ways. That reconciles with `poll.rs` rather than
//! contradicting it -- every connection on its cost list is a paged one.
//! `board.rs`'s module docs carry the table.

pub mod board;
pub mod budget;
pub mod fetch;
pub mod query;
pub mod scope;
pub mod slice;
/// The scope hierarchy #825's sidebar renders: which organisations,
/// repositories and people exist to ask about. Cheap by construction --
/// two requests, one point each, no statistics.
pub mod tree;

pub use board::{load_board, AuthorRow, Board, BoardPr, ShortSlice, TOP_N};
pub use budget::{Budget, Spend};
pub use fetch::{
    load_count, load_detail, load_reviewers, load_series, Outcome, ReviewerRow, Reviewers,
    ScopedPoint, Series,
};
pub use query::Slice;
pub use scope::{Measure, Scope, StatsQuery, Subject};
pub use tree::{load_tree, MemberRow, OrgTree, RepoRow, Tree};
