//! How Jira is reached: one enum, so a second way in -- Jira's REST API
//! directly (#2048) -- is a new variant here and nothing else.
//!
//! [`super::read`] and [`super::commands`] call only through this; none of
//! them names `acli`. An enum rather than a trait: there is a closed set of
//! transports, the methods are async, and a match keeps them `Clone` and
//! `Send` without boxing every future.

use super::cli::Acli;
use super::model::{IssueBatch, JiraConnection, JiraError, ProjectList, ProjectStatuses};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// Atlassian's CLI, which holds the credential (#2031).
    Acli(Acli),
}

impl Transport {
    /// The transport this machine has, or why there is none.
    pub fn locate() -> Result<Self, JiraError> {
        super::cli::locate().map(Self::Acli)
    }

    pub async fn connection(&self) -> JiraConnection {
        match self {
            Self::Acli(a) => a.connection().await,
        }
    }

    pub async fn projects(&self) -> ProjectList {
        match self {
            Self::Acli(a) => a.projects().await,
        }
    }

    /// Found, missing or unasked, per key; see [`Acli::issues`].
    pub async fn issues(&self, keys: &[String]) -> IssueBatch {
        match self {
            Self::Acli(a) => a.issues(keys).await,
        }
    }

    pub async fn assigned(&self, limit: usize) -> IssueBatch {
        match self {
            Self::Acli(a) => a.assigned(limit).await,
        }
    }

    pub async fn version(&self) -> Result<String, JiraError> {
        match self {
            Self::Acli(a) => a.version().await,
        }
    }

    /// The statuses seen in `project`'s recent issues; see
    /// [`Acli::project_statuses`].
    pub async fn project_statuses(&self, project: &str, limit: usize) -> ProjectStatuses {
        match self {
            Self::Acli(a) => a.project_statuses(project, limit).await,
        }
    }

    /// `Ok(None)` is "has no parent"; anything unreadable is an `Err`.
    pub async fn parent(&self, key: &str) -> Result<Option<String>, JiraError> {
        match self {
            Self::Acli(a) => a.parent(key).await,
        }
    }
}
