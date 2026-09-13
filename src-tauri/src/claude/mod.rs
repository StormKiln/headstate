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
//! `~/.claude` is read-only by design: those transcripts are Claude
//! Code's data and the files `claude --resume` depends on. There are
//! exactly TWO exceptions, and each is narrow for its own reason.
//!
//! [`handoff::consume`] truncates the handoff file after committing the
//! records it read. That file is Headstate's OWN -- the hook writes it,
//! nothing else reads it -- and rotation is the only side that knows
//! which records are already stored.
//!
//! [`install`] (#915) appends two hook matchers to
//! `~/.claude/settings.json`, a file shared with other tools, and refuses
//! to touch anything it cannot parse.
//!
//! Nothing else in here writes to `~/.claude` at all. In particular the
//! registry under `~/.claude/sessions/` is Claude Code's, and one of its
//! files is rewritten by its owner every few seconds.
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
pub mod crash;
pub mod handoff;
pub mod hook;
pub mod install;
pub mod live;
pub mod overview;
pub mod registry;
pub mod store;
pub mod transcript;

// Re-exported so `commands.rs` names the operation rather than the module it
// happens to live in. Dropping these breaks the CALL SITE rather than the
// module, which is how a merge has eaten them twice in this epic -- the error
// names a function in a module that still contains it, and five CI checks
// fail for one missing line. Do not remove them to "tidy" a conflict.
pub use transcript::{scan, scan_default, Scan, Transcript};
