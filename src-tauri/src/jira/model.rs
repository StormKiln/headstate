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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraIssue {
    pub key: String,
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
    pub updated: Option<String>,
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
    pub error: Option<String>,
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
    /// The first failure met, if any. A batch can carry issues AND an error:
    /// partial is not nothing (#1044).
    pub error: Option<String>,
    /// For a search rather than a key list: whether the result hit its
    /// limit, so the UI says "at least N" rather than N.
    pub truncated: bool,
}

/// Whether Jira can be asked anything, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JiraConnection {
    /// The capability switch is off. Nothing was asked (#1050).
    Disabled,
    NotInstalled,
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
    /// Why the dictionary or the index could not be read, if it could not.
    /// `links` is then what could be read, which may be nothing.
    pub error: Option<String>,
}
