//! The desktop side of the mobile companion: what a paired phone can reach.
//!
//! The desktop's own TLS identity, the mTLS listener it answers on, the
//! setting that turns the listener on, and the pairing protocol. Off by
//! default; nothing in this module runs until the user enables **Allow
//! phone connections**.
//!
//! See `docs/superpowers/specs/2026-09-05-mobile-companion-design.md`.

pub mod discovery;
pub mod error_kind;
pub mod events;
pub mod gate;
pub mod identity;
pub mod listener;
pub mod pairing;
pub mod privacy;
pub mod stepup;
pub mod surface;

/// The whole surface, end to end, over loopback: the spec's "loopback
/// integration test".
#[cfg(test)]
mod loopback_tests;
