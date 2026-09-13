//! Claude Code integration (#910).
//!
//! [`hook`] is the `claude-hook` subcommand: the tiny synchronous writer
//! that Claude Code itself runs on `SessionStart` and `SessionEnd`. It is
//! the only part of this feature that runs inside somebody else's process
//! and under somebody else's timeout, and its module docs state the rules
//! that follow from that.
//!
//! [`cli`] is how that subcommand is reached: the argument classification
//! and the exit path, kept separate from [`hook`] so the record shape can
//! be tested without a process to launch, and so `main.rs` depends on a
//! named function rather than on argument-parsing spelled inline.
//!
//! The consumer that reads what the hook writes is #913 and lives in
//! Headstate's poll loop, not here.
//!
//! # Why nothing here has a `remote/surface.rs` class
//!
//! `surface.rs` classes **Tauri commands** -- the things a paired phone can
//! reach over `POST /v1/call/{command}`. This module registers none: the
//! hook is a separate process that Claude Code executes, reached by an
//! argv, not by IPC, and it is not in `generate_handler!`. So there is no
//! row to add, and `every_registered_command_has_exactly_one_class` stays
//! satisfied without one.
//!
//! That will change when #913 and the views land, and the classes are
//! already argued in #910's plan: reading sessions is `Read`, revealing a
//! transcript in Finder is `Local`. Neither belongs to this issue.

pub mod cli;
pub mod hook;
