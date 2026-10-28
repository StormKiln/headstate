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
    pub parent_key: Option<String>,
    pub project_key: String,
    /// The browse URL, derived from the API `self` link. `None` when the
    /// response carried no link to derive it from.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
