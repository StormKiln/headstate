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
//! [`liveness`] is the reader's half (#917): whether a stored session is
//! still running is DERIVED from the machine every time a row is
//! rendered, never stored, and has three answers rather than two. The
//! third -- "could not tell" -- is what keeps a failed probe from
//! offering to resurrect a session that is alive and mid-work.
//!
//! [`sessions`] assembles what the Claude Code view renders: the stored
//! rows, each one's derived liveness, and the tri-state cwd check that
//! decides which resume command is safe to copy (#918).
//!
//! Nothing in here writes to `~/.claude`. Read-only by design: those
//! transcripts are Claude Code's data and the file `claude --resume`
//! depends on.

pub mod liveness;
pub mod sessions;
pub mod store;
pub mod transcript;

pub use transcript::{scan, scan_default, Scan, Transcript};
