//! Claude Code sessions: what Headstate knows about them and where it
//! learned it.
//!
//! Epic #910. Today this holds the transcript importer (#914), which is
//! the source that works retroactively: `~/.claude/projects` already
//! contains every session that ever ran on the machine, so the Claude
//! Code view opens with real history rather than an empty list waiting
//! for a hook to fire.
//!
//! The hook path (#912, #913) is the other source and will land beside
//! this one. They meet in [`store`], which upserts on `session_id` and
//! resolves the overlap per field by which source actually knows the
//! answer -- see [`store::import`].
//!
//! [`install`] (#915) is the ONE exception to the rule below, and it is
//! narrow on purpose: it appends two hook matchers to
//! `~/.claude/settings.json` and refuses to touch anything it cannot parse.
//! Nothing else in here writes to `~/.claude`. The transcripts in
//! particular are read-only by design -- they are Claude Code's data and the
//! files `claude --resume` depends on.

pub mod install;
pub mod store;
pub mod transcript;

pub use transcript::{scan, scan_default, Scan, Transcript};
