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
//! [`handoff`] consumes what the hook appends (#913), and [`registry`]
//! reads `~/.claude/sessions/`, the live session registry -- a THIRD
//! source the epic missed, and the best of the three for liveness: it
//! carries the pid, `procStart`, `sessionId`, `cwd` and `name` with no
//! hook installed at all. A registry file whose pid is dead is a
//! positive crash signal, which [`crash`] records; see its module docs
//! for why that is strictly better than inferring a crash from a missing
//! `SessionEnd`.
//!
//! Nothing in here writes to `~/.claude`. Read-only by design: those
//! transcripts are Claude Code's data and the file `claude --resume`
//! depends on. The ONE exception is the handoff file, which is
//! Headstate's own: [`handoff::consume`] truncates it after committing
//! the records it read, because rotation is the only side that knows
//! which records are already stored.

pub mod crash;
pub mod handoff;
pub mod registry;
pub mod store;
pub mod transcript;

pub use transcript::{scan, scan_default, Scan, Transcript};
