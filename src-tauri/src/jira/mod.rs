//! Jira Cloud, read through Atlassian's `acli` (epic #2031).
//!
//! `acli` holds the credential, exactly as `gh` and `glab` do for the two
//! code hosts: Headstate never sees a Jira token. Everything here is a READ;
//! the first release is read-only by decision on #2031.
//!
//! Key DETECTION is not here. It runs in the frontend (`src/lib/jira/keys.ts`),
//! which already holds every pull request, worktree and branch row; this
//! module is only ever handed keys that were already found, and answers what
//! Jira says about them.
//!
//! The JSON shapes in [`parse`] follow Jira Cloud's REST v3 issue and project
//! objects, which is what `acli --json` is documented to wrap. They are NOT
//! yet verified against a live `acli` (#2032), so every reader is tolerant: a
//! field that is missing reads as absent, never as a default that looks like
//! data.

pub mod cli;
pub mod commands;
pub mod model;
pub mod parse;

pub use model::{
    IssueBatch, JiraConnection, JiraIssue, JiraIssueType, JiraProject, JiraStatus, ProjectList,
    StatusCategory,
};
