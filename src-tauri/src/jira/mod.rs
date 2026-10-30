//! Jira Cloud, read through Atlassian's `acli` (epic #2031).
//!
//! `acli` holds the credential, exactly as `gh` and `glab` do for the two
//! code hosts: Headstate never sees a Jira token. Everything here is a READ
//! except one: [`commands::jira_transition`] moves an issue to another
//! status (#2044), only when the user confirms the exact argv, and reports
//! the status Jira holds afterwards rather than the one asked for.
//!
//! Key detection is here too ([`keys`]), feeding an index kept in the
//! database ([`index`]): built from the pull requests, merged history,
//! worktrees and branches the app already stores, and rebuilt only when
//! those change. The frontend reads keys from the index rather than parsing
//! text on every render.
//!
//! The JSON shapes in [`parse`] are Jira Cloud's REST v3 objects, checked
//! against a live `acli` 1.3.39 (#2032; fixtures in `fixtures/`). Every
//! reader stays tolerant: a field that is missing reads as absent, never as
//! a default that looks like data.

pub mod alerts;
pub mod cache;
pub mod cli;
pub mod commands;
pub mod drift;
pub mod index;
pub mod keys;
pub mod lookup;
pub mod model;
pub mod parse;
pub mod read;
pub mod refresh;
pub mod transport;

pub use model::{
    IssueBatch, JiraConnection, JiraError, JiraIssue, JiraIssueType, JiraProject, JiraStatus,
    ProjectList, StatusCategory,
};
