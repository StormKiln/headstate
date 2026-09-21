//! Which dependencies are out of date, per repository.
//!
//! Reporting is the bulk of it: the output is a list and a markdown
//! rendering of it, meant to be handed to an agent that does the work.
//!
//! Two modules write. `apply` runs a package manager in a throwaway
//! worktree, and `cargo_apply` edits a `Cargo.toml` directly -- the one
//! ecosystem whose tool makes three separate wrong edits when aimed at
//! a workspace, so the edit is made here instead.
//!
//! Nothing here talks to GitHub.

pub mod apply;
pub mod cargo;
pub mod cargo_apply;
pub mod detect;
pub mod markdown;
pub mod model;
pub mod pr;
pub mod registry;
pub mod run;
pub mod runs;
pub mod scripts;
pub mod swift;
pub mod terraform;
pub mod tools;
pub mod version;

pub use model::{Bump, Ecosystem, EcosystemReport, Outdated, ProjectReport};
