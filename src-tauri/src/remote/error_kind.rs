//! A command rejection's kind beside its message, on the remote wire
//! (#1202).
//!
//! # The defect
//!
//! 138 Tauri commands reject with `Result<_, String>`, and the frontend
//! re-derives the distinctions the string just lost. `notAsked.ts`
//! recognises one case by a marker embedded in the prose; other sites
//! match on message text. The type existed on this side, was flattened
//! at the boundary, and is guessed back on the other one.
//!
//! # Why this classifies rather than re-typing 138 signatures
//!
//! The kinds are already recoverable from the strings -- that is
//! precisely what `isNotAsked` does. So the classification happens
//! ONCE, here, on the producing side, instead of at every consuming
//! site. No command signature changes, which is what keeps this from
//! being the change that stalls half-done: there is no state in which
//! half the commands are typed and half are not.
//!
//! This is the last regex. It lives in one place, next to the constant
//! it matches, in the crate that defines it.
//!
//! # Two kinds, not three
//!
//! `Cancelled` is deliberately absent. `headstate:cancelled` is
//! produced by `src-mobile`'s OWN commands (`companion.rs`), a
//! different crate whose rejections never pass through
//! [`surface::res`]. A variant for it would be a wire contract for a
//! value this side cannot emit -- a promise the producer cannot keep.
//! It belongs to that crate's own track.
//!
//! `Other` is not a bucket that destroys information: the message
//! always travels intact beside the kind, so an unclassified rejection
//! reads exactly as it does today.

use serde::{Deserialize, Serialize};

/// What kind of rejection this is, for a reader that needs to branch.
///
/// Kebab-case on the wire. The variant list is asserted against the
/// TypeScript union by `mirroredConstants.test.ts` in both directions,
/// so adding a variant on either side alone fails the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    /// Headstate declined to issue a request, rather than issuing one
    /// that failed (#1050).
    ///
    /// The difference is between two sentences that are not
    /// interchangeable: "GitHub did not answer" and "we did not ask".
    /// When no client is held, no request is constructed -- so the
    /// retry a failure banner offers cannot work. Nothing about
    /// pressing "Try again" makes a token appear.
    NotAsked,
    /// Anything else. The message carries the detail, as it always has.
    Other,
}

/// A rejection as the remote wire carries it: the kind, and the exact
/// message the webview would have seen.
///
/// The message is NOT derived from the kind and is never reconstructed
/// from it. It is the command's own prose, verbatim, so a consumer that
/// ignores `kind` entirely behaves exactly as it did before this
/// existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CommandError {
    /// Classify a rejection, keeping its message intact.
    ///
    /// The marker is NOT stripped here. Stripping it would change what
    /// existing readers see the moment this lands, and the point of
    /// this increment is that nothing downstream has to change yet.
    /// `notAsked.ts` still strips it for display; this only adds a
    /// channel that does not require reading the prose to know what
    /// happened.
    pub fn classify(message: impl Into<String>) -> Self {
        let message = message.into();
        let kind = if message.starts_with(crate::commands::NOT_ASKED) {
            ErrorKind::NotAsked
        } else {
            ErrorKind::Other
        };
        Self { kind, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{AUTH_ERR, NOT_ASKED};

    #[test]
    fn the_declined_request_marker_classifies_as_not_asked() {
        assert_eq!(CommandError::classify(AUTH_ERR).kind, ErrorKind::NotAsked);
    }

    #[test]
    fn an_ordinary_failure_classifies_as_other() {
        let e = CommandError::classify("GitHub answered 503");
        assert_eq!(e.kind, ErrorKind::Other);
        assert_eq!(e.message, "GitHub answered 503");
    }

    /// The marker travels intact. A consumer that ignores `kind` must
    /// see byte-for-byte what it saw before this type existed.
    #[test]
    fn classifying_never_alters_the_message() {
        for m in [AUTH_ERR, "GitHub answered 503", "", NOT_ASKED] {
            assert_eq!(CommandError::classify(m).message, m);
        }
    }

    /// The marker is a PREFIX of the rejection, not the whole of it:
    /// `AUTH_ERR` is the marker then the prose. Matching the whole
    /// string would classify the real rejection as `Other`.
    #[test]
    fn the_marker_is_matched_as_a_prefix_not_an_equality() {
        assert_ne!(AUTH_ERR, NOT_ASKED);
        assert!(AUTH_ERR.starts_with(NOT_ASKED));
        assert_eq!(CommandError::classify(AUTH_ERR).kind, ErrorKind::NotAsked);
    }

    /// A message that merely CONTAINS the marker later on is not a
    /// declined request -- it is prose that happens to quote it.
    #[test]
    fn the_marker_elsewhere_in_the_message_is_not_a_declined_request() {
        let quoted = format!("the command replied with {NOT_ASKED}");
        assert_eq!(CommandError::classify(quoted).kind, ErrorKind::Other);
    }

    #[test]
    fn the_kinds_serialise_kebab_case() {
        assert_eq!(
            serde_json::to_string(&ErrorKind::NotAsked).unwrap(),
            "\"not-asked\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorKind::Other).unwrap(),
            "\"other\""
        );
    }
}
