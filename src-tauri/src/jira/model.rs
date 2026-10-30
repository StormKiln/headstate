//! What Headstate knows about Jira. Mirrored in `src/types/jira.ts`.

use serde::{Deserialize, Serialize};

/// Jira's own three-way grouping of every status in every workflow.
///
/// This is what makes drift checks (#2042) workflow-independent: "In
/// Testing", "QA" and "Code Review" are all `InProgress`, whatever a project
/// calls them.
///
/// `Unknown` is a real state, not a fallback for `New`. A status whose
/// category did not come back must never read as "To Do" -- that would turn
/// "we could not tell" into a confident drift warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory {
    New,
    InProgress,
    Done,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraStatus {
    pub name: String,
    pub category: StatusCategory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraIssueType {
    pub name: String,
    /// -1 sub-task, 0 standard, 1 epic, 2+ initiative and above. `None` when
    /// the site did not say, which older company-managed projects may not.
    pub hierarchy_level: Option<i32>,
    /// The type's icon, as Jira links it. Through `acli` this names an
    /// internal Atlassian host (#2032), which may need a session to load,
    /// so a consumer must treat a failed load as "no icon", not an error.
    #[serde(default)]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraIssue {
    pub key: String,
    /// Jira's numeric id, as a string. Stable across a key change (an issue
    /// moved to another project keeps it); `None` when the reply had none.
    #[serde(default)]
    pub id: Option<String>,
    pub summary: String,
    pub issue_type: JiraIssueType,
    pub status: JiraStatus,
    pub assignee: Option<String>,
    /// The parent's key: an epic for a story, an initiative for an epic.
    /// Meaningful only when `parent_checked`.
    pub parent_key: Option<String>,
    /// Whether the parent has been read. `acli`'s search cannot return
    /// parents, so they are read one `workitem view` at a time and may not
    /// have been yet: `parent_key: None` with this false is "not known",
    /// never "has no epic".
    #[serde(default)]
    pub parent_checked: bool,
    pub project_key: String,
    /// The browse URL on the signed-in site. `None` when the site is not
    /// known: the API's `self` link names an internal Atlassian host, not
    /// the site, so it cannot be used (#2032).
    pub url: Option<String>,
    /// Never filled through `acli` 1.3: its search refuses `updated`
    /// ("field 'updated' is not allowed", re-checked on #2033), and asking
    /// for it fails the whole call. Kept for a transport that can (#2048);
    /// `None` is "not read", never "never updated".
    pub updated: Option<String>,
    /// When this status was read from Jira, RFC 3339 (#2036). Set by the
    /// cache on every issue it sends; `None` only for an issue that has
    /// not been through it, and never rendered as an age.
    #[serde(default)]
    pub read_at: Option<String>,
    /// Served from the cache because the read that should have refreshed
    /// it failed (#2036). `read_at` then says how old the answer is.
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraProject {
    pub key: String,
    pub name: String,
}

/// The project list, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectList {
    pub projects: Vec<JiraProject>,
    /// Set when the list could not be read. `projects` is then whatever was
    /// read before the failure, which for one call is nothing.
    pub error: Option<JiraError>,
}

/// The answer for one set of requested keys.
///
/// Three disjoint outcomes per key, because they mean different things on
/// screen (root CLAUDE.md, "Pending and Unknown are different states"):
/// `issues` came back, `missing` were asked and Jira did not return them
/// (not found, or hidden from this user -- Jira does not say which), and
/// `unasked` were never sent because the call budget ran out first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueBatch {
    pub issues: Vec<JiraIssue>,
    pub missing: Vec<String>,
    pub unasked: Vec<String>,
    /// Why the read is incomplete, if it is. A batch can carry issues AND
    /// an error: partial is not nothing (#1044). A failure that ends the
    /// read -- rate limited, timed out, signed out -- wins over an earlier
    /// per-batch refusal, because it is the one that says what to do.
    pub error: Option<JiraError>,
    /// For a search rather than a key list: whether the result hit its
    /// limit, so the UI says "at least N" rather than N.
    pub truncated: bool,
    /// For a search: when the LIST itself was read, RFC 3339 (#2036).
    /// With `error` set, the list is the last one that was read, served
    /// stale; `None` there means no list has ever been read.
    #[serde(default)]
    pub list_read_at: Option<String>,
}

/// Why a Jira read did not happen, or did not finish (#2033).
///
/// One enum for every command and every partial answer, so each state
/// renders distinctly (root CLAUDE.md): `NotAsked` is "we did not ask"
/// and must never read as "Jira did not answer" or "nothing found"
/// (#1050); `RateLimited`, `TimedOut` and `NotLoggedIn` end a read
/// part-way, and what they leave is `unasked`, not `missing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JiraError {
    /// The capability switch is off: nothing was asked, nothing spawned.
    /// `Display` leads with [`crate::commands::NOT_ASKED`] so the mobile
    /// companion, which receives a rejection as text, still tells it apart.
    #[error(
        "{} Jira is switched off, so it was not asked",
        crate::commands::NOT_ASKED
    )]
    NotAsked,
    #[error("acli is not installed: looked in {}; set HEADSTATE_ACLI to its path", searched.join(", "))]
    NotInstalled { searched: Vec<String> },
    #[error("acli is not signed in to Jira: run `acli jira auth login --web`")]
    NotLoggedIn,
    /// Jira refused for rate. `retry_after_secs` is what it asked for, when
    /// it said; `None` is "did not say", not "retry now". Detected from
    /// `acli`'s text and unverified against a live 429 (#2033).
    #[error("Jira is rate limiting requests{}", retry_text(*retry_after_secs))]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("acli did not answer within {secs}s")]
    TimedOut { secs: u64 },
    #[error("{detail}")]
    Failed { detail: String },
}

fn retry_text(secs: Option<u64>) -> String {
    match secs {
        Some(s) => format!("; retry after {s}s"),
        None => "; it did not say when to retry".into(),
    }
}

impl JiraError {
    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    /// Whether this failure ends a read: the next call would meet it too,
    /// so what is left goes back `unasked` rather than being tried. Only a
    /// plain `Failed` -- one query Jira refused -- is worth narrowing.
    pub fn ends_the_read(&self) -> bool {
        !matches!(self, Self::Failed { .. })
    }
}

/// Whether Jira can be asked anything, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JiraConnection {
    /// The capability switch is off. Nothing was asked (#1050).
    Disabled,
    /// `acli` was not found. `searched` names every place looked in, so
    /// the reader can tell a missing install from one in an odd place.
    NotInstalled {
        searched: Vec<String>,
    },
    NotLoggedIn {
        detail: String,
    },
    LoggedIn {
        site: Option<String>,
        account: Option<String>,
    },
    /// `acli` answered in a way this build does not recognise.
    CannotTell {
        reason: String,
    },
}

/// Which of the user's lists a pull request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrRole {
    Authored,
    Reviewing,
    /// Found by searching GitHub for the key (#2041): not in the user's
    /// lists, and perhaps not theirs at all.
    Searched,
}

/// A pull request's state, where it is known. Pull requests from the
/// user's lists are open by being listed and carry none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// What names a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkTarget {
    /// A pull or merge request from one of the user's lists.
    Pr {
        source: crate::identity::Source,
        role: PrRole,
        repo: String,
        number: u64,
        title: String,
        url: String,
        head_ref: String,
        is_draft: bool,
        /// Known only for a searched pull request.
        #[serde(default)]
        state: Option<PrState>,
        #[serde(default)]
        author: Option<String>,
    },
    Worktree {
        repo: String,
        path: String,
        branch: String,
    },
    /// A local branch, noted when the Branches page scans its repository.
    Branch { repo_path: String, branch: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraLink {
    pub key: String,
    pub found_in: super::keys::FoundIn,
    /// In the latest read of where it lives. A `Pr` that is not current was
    /// open earlier and has since left its list: merged or closed, unknown
    /// which.
    pub current: bool,
    pub target: LinkTarget,
}

/// Every link whose key is in the user's projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkIndex {
    pub links: Vec<JiraLink>,
    /// The dictionary applied, after Settings' exclusions.
    pub projects: Vec<String>,
    /// Why the project list -- the dictionary -- could not be read, if it
    /// could not. `links` is then what could be read, which may be nothing.
    pub error: Option<JiraError>,
    /// The user's open pull requests the index has looked at, as it saw
    /// them (#2046). A pull request here with no current link names no
    /// ticket; one NOT here, or here under a different title or branch than
    /// a row now shows, has not been looked at yet -- which is not "has no
    /// ticket".
    #[serde(default)]
    pub checked: Vec<CheckedPr>,
}

/// One open, authored pull request as the index read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedPr {
    pub url: String,
    pub title: String,
    pub head_ref: String,
}

/// The statuses seen in one project's issues (#2034).
///
/// `acli` 1.3.39 has no command that lists a workflow's statuses, so these
/// are read from the project's most recently updated issues: a status no
/// issue currently holds is not here. The UI says so ("seen in"), and
/// always offers the three categories besides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectStatuses {
    pub project: String,
    /// Distinct by name and category, in the order first seen.
    pub statuses: Vec<JiraStatus>,
    /// Whether the read hit its limit, so older issues -- and statuses only
    /// they hold -- were not looked at.
    pub truncated: bool,
    /// Why the read failed, if it did. `statuses` is then empty: not "this
    /// project has none".
    pub error: Option<JiraError>,
}

/// A status category a user can expect: [`StatusCategory`] without
/// `Unknown`, which is something Jira reports, never something to want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedCategory {
    New,
    InProgress,
    Done,
}

/// What a ticket's status should be at one point in a pull request's life.
/// A category matches any status in it; a named status matches by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusExpectation {
    Category { category: ExpectedCategory },
    Status { name: String },
}

/// One project's expectations. `None` is "not set": the drift rules then
/// use the defaults (in progress while open, done once merged), which are
/// exactly what they did before this setting existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusExpectations {
    #[serde(default)]
    pub on_open: Option<StatusExpectation>,
    #[serde(default)]
    pub on_merged: Option<StatusExpectation>,
}

/// What `acli` said about one transition (#2044), before anything is
/// re-read. Never the outcome on its own: Jira may run automation on the
/// move, or `acli` may report success for a move that left the status
/// where it was, so what the issue holds now is in
/// [`TransitionResult::after`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TransitionReply {
    /// `acli` reported `SUCCESS` for the key. Its message, verbatim.
    Accepted { message: String },
    /// `acli` reported `FAILURE` for the key: Jira's reason, verbatim --
    /// a move the workflow does not allow from here, a required field, a
    /// validator, or an issue this user cannot see. `acli` 1.3.39 exits 0
    /// for this, so it is read from the reply, never from the exit code.
    Refused { reason: String },
    /// Whether Jira moved it is not known: the call timed out (and may
    /// have been written before it was killed), failed, or printed a reply
    /// this build does not read. The re-read says where it is.
    Unknown { error: JiraError },
}

/// One transition and what Jira reports after it (#2044).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionResult {
    pub key: String,
    /// The status asked for. Shown only as what was asked, never as where
    /// the issue is.
    pub requested: String,
    pub reply: TransitionReply,
    /// The issue read fresh from Jira after the call, which also updated
    /// the cache. `None` when that read failed or did not return the key:
    /// then `reread_error` says why, and nothing is claimed about where
    /// the issue is now. Never an answer from the cache.
    pub after: Option<JiraIssue>,
    pub reread_error: Option<JiraError>,
}
