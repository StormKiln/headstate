//! Claude Code sessions: what Headstate knows about them and where it
//! learned it.
//!
//! Epic #910. There are TWO sources, and the split is the design:
//!
//! [`transcript`] is the retroactive one (#914). `~/.claude/projects`
//! already contains every session that ever ran on the machine, so the
//! Claude Code view opens with real history rather than an empty list
//! waiting for a hook to fire.
//!
//! [`hook`] is the live one (#912): the `claude-hook` subcommand that
//! Claude Code itself runs on `SessionStart` and `SessionEnd`. It is the
//! only part of this feature that runs inside somebody else's process and
//! under somebody else's timeout, and its module docs state the rules that
//! follow from that. It contributes what no transcript can -- the pid, and
//! the `source`/`reason` of a run.
//!
//! [`cli`] is how that subcommand is reached: the argument classification
//! and the exit path, kept separate from [`hook`] so the record shape can
//! be tested without a process to launch, and so `main.rs` depends on a
//! named function rather than on argument-parsing spelled inline.
//!
//! The two sources meet in [`store`], which upserts on `session_id` and
//! resolves the overlap per field by which source actually knows the
//! answer -- see [`store::import`]. The consumer that reads what the hook
//! writes is #913 and lives in Headstate's poll loop, not here.
//!
//! Nothing in here writes to `~/.claude`. Read-only by design: those
//! transcripts are Claude Code's data and the file `claude --resume`
//! depends on. The one exception will be the hook installer (#915), which
//! edits `settings.json` and is deliberately a separate, explicit action.
//!
//! # Why nothing here has a `remote/surface.rs` class of its own
//!
//! `surface.rs` classes **Tauri commands** -- the things a paired phone can
//! reach over `POST /v1/call/{command}`. The hook registers none: it is a
//! separate process that Claude Code executes, reached by an argv rather
//! than by IPC, and it is not in `generate_handler!`. So there is no row to
//! add for it, and `every_registered_command_has_exactly_one_class` stays
//! satisfied without one.
//!
//! The importer's command IS classed, as `Read` -- it asks the desktop to
//! re-read the desktop's own history, which is the same reasoning that
//! makes `refresh_now` a read.

pub mod cli;
pub mod hook;
pub mod store;
pub mod transcript;

// Re-exported so `commands.rs` names the operation rather than the module it
// happens to live in. `claude_import_transcripts` reaches the importer this
// way, so dropping these breaks the CALL SITE rather than the module -- which
// is how a merge lost them twice: once in my first resolution of the #912 +
// #914 add/add conflict, and again when a later merge of the same branches
// reintroduced the same gap. The compiler is what caught it both times
// (`cannot find function scan_default in module crate::claude`), never review.
pub use transcript::{scan, scan_default, Scan, Transcript};
