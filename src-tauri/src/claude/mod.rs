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
//! [`overview`] (#921) is the aggregate layer for the overview page. It
//! counts over the rows [`store`] holds and derives no liveness of its
//! own -- #917's `liveness` module owns that, and two answers to one
//! question disagree the first time either changes. [`live`] is the seam
//! between them until #917 lands, and its own comment says so.
//!
//! [`install`] (#915) is the ONE exception to the read-only rule below, and
//! it is narrow on purpose: it appends two hook matchers to
//! `~/.claude/settings.json` and refuses to touch anything it cannot parse.
//! Nothing else in here writes to `~/.claude`. The transcripts in
//! particular are read-only by design -- they are Claude Code's data and the
//! files `claude --resume` depends on.

pub mod cli;
pub mod hook;
pub mod install;
pub mod live;
pub mod overview;
pub mod store;
pub mod transcript;

// Re-exported so `commands.rs` names the operation rather than the module it
// happens to live in. Dropping these breaks the CALL SITE rather than the
// module, which is how a merge has eaten them twice in this epic -- the error
// names a function in a module that still contains it, and five CI checks
// fail for one missing line. Do not remove them to "tidy" a conflict.
pub use transcript::{scan, scan_default, Scan, Transcript};
