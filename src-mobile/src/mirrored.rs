//! The constants this crate shares with the desktop, asserted against the
//! desktop's own source.
//!
//! # Why this file exists (#854)
//!
//! #850 found five constants whose doc comments said the agreement with
//! their other copy was asserted, and which no test actually checked. One
//! had already drifted -- `15 * 60` in Rust against `60 * 60` in the UI,
//! both comments claiming they matched -- with three user-visible
//! consequences. `src/lib/mirroredConstants.test.ts` is the remedy, and
//! it covers exactly those five pairs, every one of which has its second
//! copy in TypeScript.
//!
//! The Rust-to-Rust pairs were never in scope of that file, and
//! `invariants::tests::every_cross_crate_constant_is_read_from_both_sides`
//! in the desktop crate found eleven of them. This is their assertion.
//!
//! Worst of the eleven, and the reason this is a file rather than a
//! footnote: `ECDSA_SIG_LEN` and `MLDSA_SIG_LEN` live in
//! `crates/headstate-stepup`, whose module doc says in so many words that
//! it holds what "both ends must agree on" -- and `keys.rs` DECLARES THEM
//! AGAIN instead of importing them. `keys.rs` then asserts its own local
//! copies against literals, so a change in the shared crate is invisible
//! here. These are signature lengths on a signing boundary: disagree on
//! one and every step-up signature this phone makes is rejected by a
//! desktop that cannot say why, or -- worse, for the ML-DSA length --
//! a truncated signature is verified against a padded buffer.
//!
//! # The mechanism, and why it is source text
//!
//! `include_str!` on the desktop's and the shared crate's sources, the
//! idiom `surface.rs:248` already uses here for the command table, with
//! the reason it gives: it "ties this test to the desktop file at compile
//! time, so the two are compared as they are checked in, not as someone
//! remembers them." A `cargo` dependency cannot do it -- `src-mobile` is
//! a separate crate with its own lockfile and does not depend on
//! `src-tauri` at all, which is exactly why these values were copied.
//!
//! The DIRECTION that matters is that changing either side alone fails:
//! the extracted number moves when the desktop moves, and the compared
//! constant moves when this crate does. An assertion against a literal --
//! `assert_eq!(NONCE_LEN, 16)`, which `src-tauri/src/remote/stepup.rs`
//! still does -- reads one side twice and cannot fail when the other
//! moves. That is the anti-pattern `mirroredConstants.test.ts`' header
//! was written to condemn and this file avoids.
//!
//! # What this cannot see
//!
//! - **A pair with a different name on each side.** The desktop's
//!   `SEED_LEN` (32) and this crate's `VAULT_KEY_LEN` (32) are the same
//!   32 bytes, and nothing here or in the guard relates them.
//! - **A value copied as a bare literal** rather than as a named
//!   constant. `listener::PORT` is `41919` on the desktop and an unnamed
//!   `41919` in five places in this crate; only the named side is visible
//!   to the guard that demanded this file.
//! - **Whether the two values MEAN the same thing.** That judgement is
//!   the `COINCIDENTAL` list in the desktop's guard, and it is made by
//!   reading both, once, deliberately.

#[cfg(test)]
mod tests {
    /// The integer value of a `const NAME: ty = <literal>;` in Rust
    /// source.
    ///
    /// Parsed from the source text rather than matched against an
    /// expected spelling, for the reason
    /// `mirroredConstants.test.ts`' `rustConst` gives: the spelling is
    /// not the invariant, `3309` and `3_309` are one number, and a test
    /// demanding one spelling fails a correct edit while still passing on
    /// a wrong value written the expected way.
    ///
    /// Anything that is not a plain integer literal PANICS rather than
    /// being approximated. A mirror test that quietly stops reading the
    /// real value is worse than none: it goes on passing while describing
    /// a value the other side no longer has.
    fn int_const(src: &str, name: &str, file: &str) -> u64 {
        let value = const_expr(src, name, file);
        value
            .replace('_', "")
            .parse()
            .unwrap_or_else(|_| panic!("{file}'s {name} is not an integer literal: {value}"))
    }

    /// The string value of a `const NAME: &str = "...";`.
    fn str_const(src: &str, name: &str, file: &str) -> String {
        let value = const_expr(src, name, file);
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or_else(|| panic!("{file}'s {name} is not a string literal: {value}"))
            .to_string()
    }

    /// The right-hand side of a `const NAME ... = <expr>;`, as text.
    ///
    /// Anchored on `const NAME:` with the colon required, so a mention of
    /// the name in a doc comment or another expression cannot be read as
    /// the declaration -- the scoping mistake
    /// `every_stats_query_meters_itself` records an earlier version of
    /// itself making.
    fn const_expr(src: &str, name: &str, file: &str) -> String {
        let anchor = format!("const {name}:");
        let at = src
            .find(&anchor)
            .unwrap_or_else(|| panic!("{file} must define {name}"));
        let rest = &src[at + anchor.len()..];
        let (_, value) = rest
            .split_once('=')
            .unwrap_or_else(|| panic!("{file}'s {name} has no value"));
        let (value, _) = value
            .split_once(';')
            .unwrap_or_else(|| panic!("{file}'s {name} does not terminate"));
        value.trim().to_string()
    }

    /// The desktop's sources, and the shared crate's.
    const DESKTOP_PAIRING: &str = include_str!("../../src-tauri/src/remote/pairing.rs");
    const DESKTOP_DISCOVERY: &str = include_str!("../../src-tauri/src/remote/discovery.rs");
    const DESKTOP_EVENTS: &str = include_str!("../../src-tauri/src/remote/events.rs");
    const DESKTOP_IDENTITY: &str = include_str!("../../src-tauri/src/remote/identity.rs");
    const DESKTOP_LISTENER: &str = include_str!("../../src-tauri/src/remote/listener.rs");
    const STEPUP: &str = include_str!("../../crates/headstate-stepup/src/lib.rs");

    /// Guards the guard. Every assertion below is only as good as this
    /// extraction, and a parser that silently matched nothing -- or
    /// matched and mis-read -- would make the whole file vacuously true.
    /// That is the exact failure mode #850 is about, so it is asserted
    /// rather than assumed.
    #[test]
    fn the_parser_that_reads_the_other_side_works() {
        assert_eq!(int_const("const A: usize = 64;", "A", "t"), 64);
        assert_eq!(int_const("pub const B: usize = 3_309;", "B", "t"), 3309);
        assert_eq!(str_const("const C: &str = \"fp\";", "C", "t"), "fp");
        // And it must refuse what it cannot read, rather than returning a
        // default that would compare equal to nothing.
        assert!(
            std::panic::catch_unwind(|| int_const("const D: usize = OTHER;", "D", "t")).is_err()
        );
        assert!(
            std::panic::catch_unwind(|| int_const("const E: usize = 1;", "MISSING", "t")).is_err()
        );
    }

    /// The step-up signature lengths, which are declared in the shared
    /// crate AND again here.
    ///
    /// `headstate-stepup`'s module doc says it holds "what both ends must
    /// agree on", so importing these would be better than asserting them.
    /// Asserting is what can be done without restructuring the phone's
    /// dependency graph, and it makes the duplication visible the moment
    /// either side moves. `keys.rs`' own `debug_assert_eq!`s compare
    /// against its LOCAL copies, so they cannot see the shared crate
    /// change at all.
    #[test]
    fn the_signature_lengths_match_the_shared_crate() {
        assert_eq!(
            crate::keys::ECDSA_SIG_LEN,
            int_const(STEPUP, "ECDSA_SIG_LEN", "headstate-stepup/lib.rs") as usize,
            "a P-256 signature this phone makes must be the length the \
             desktop's verifier expects, or every step-up is refused"
        );
        assert_eq!(
            crate::keys::MLDSA_SIG_LEN,
            int_const(STEPUP, "MLDSA_SIG_LEN", "headstate-stepup/lib.rs") as usize,
            "ML-DSA-65, FIPS 204 table 2. A disagreement here is worse than \
             a refusal: a short signature verified against a padded buffer"
        );
    }

    /// The public-key lengths, declared in the desktop's pairing module
    /// and again here.
    ///
    /// These bound what each end will accept from a pairing QR, so a
    /// disagreement makes pairing fail at the far end with a length error
    /// about a key that is the right length on the side that made it.
    #[test]
    fn the_public_key_lengths_match_the_desktop() {
        assert_eq!(
            crate::keys::ECDSA_P256_LEN,
            int_const(DESKTOP_PAIRING, "ECDSA_P256_LEN", "remote/pairing.rs") as usize
        );
        assert_eq!(
            crate::keys::MLDSA_65_LEN,
            int_const(DESKTOP_PAIRING, "MLDSA_65_LEN", "remote/pairing.rs") as usize
        );
    }

    /// The pairing handshake's own numbers.
    ///
    /// `QR_VERSION` is what each end stamps and checks on a pairing
    /// payload: a mismatch is the one failure mode that presents as "this
    /// QR code is from a different version of the app" when both ends are
    /// the same build. `TOKEN_LEN` bounds the pairing token.
    #[test]
    fn the_pairing_handshake_numbers_match_the_desktop() {
        assert_eq!(
            u64::from(crate::pairing::QR_VERSION),
            int_const(DESKTOP_PAIRING, "QR_VERSION", "remote/pairing.rs")
        );
        assert_eq!(
            crate::pairing::TOKEN_LEN,
            int_const(DESKTOP_PAIRING, "TOKEN_LEN", "remote/pairing.rs") as usize
        );
    }

    /// The discovery triple: the mDNS service type, the TXT key the
    /// fingerprint travels under, and how much of the fingerprint goes in
    /// it.
    ///
    /// All three are wire format. A phone advertising or reading a
    /// different `SERVICE_TYPE` simply never finds the desktop, with no
    /// error anywhere -- the silent failure this codebase calls "silence
    /// read as success". `FP_PREFIX_LEN`'s own doc comment here says
    /// "Must match the desktop's `FP_PREFIX_LEN`", which is the claim
    /// this test turns into an assertion.
    #[test]
    fn the_discovery_wire_format_matches_the_desktop() {
        assert_eq!(
            crate::discovery::SERVICE_TYPE,
            str_const(DESKTOP_DISCOVERY, "SERVICE_TYPE", "remote/discovery.rs"),
            "a different service type finds nothing, and says nothing"
        );
        assert_eq!(
            crate::discovery::TXT_FP,
            str_const(DESKTOP_DISCOVERY, "TXT_FP", "remote/discovery.rs")
        );
        assert_eq!(
            crate::discovery::FP_PREFIX_LEN,
            int_const(DESKTOP_DISCOVERY, "FP_PREFIX_LEN", "remote/discovery.rs") as usize,
            "a shorter prefix compares fewer fingerprint characters, so it \
             matches desktops it should not"
        );
    }

    /// The snapshot event name.
    ///
    /// `events.rs`' `EVENT_NAMES` list is already tied to the desktop's
    /// by `events.rs`' own `include_str!` test, and
    /// `mirroredConstants.test.ts` asserts `"prs-updated"` is in it. The
    /// named constant beside that list was the part nothing compared: the
    /// list could agree while the constant used to subscribe did not.
    #[test]
    fn the_snapshot_event_name_matches_the_desktop() {
        assert_eq!(
            crate::events::SNAPSHOT_EVENT,
            str_const(DESKTOP_EVENTS, "SNAPSHOT_EVENT", "remote/events.rs")
        );
    }

    /// The wire protocol version: what the desktop's `/v1/hello` reports
    /// and its QR embeds, and what this client speaks.
    ///
    /// `connection.rs` calls a desktop below this crate's number too old
    /// to drive, and `events.rs` warns on any other number, so a bump on
    /// one side alone tells a phone that its own desktop is the wrong
    /// version. Until #1555 no single test read both copies. The desktop's
    /// guard counted this pair as covered only because it read
    /// `events.rs`' `the_allowlist_matches_the_desktops` on to the end of
    /// its module, where another test compares this crate's copy with
    /// itself. The chain through `QR_VERSION` holds both ends, but across
    /// three tests in two crates, which no guard can see as one.
    #[test]
    fn the_protocol_version_matches_the_desktop() {
        assert_eq!(
            u64::from(crate::client::PROTOCOL_VERSION),
            int_const(DESKTOP_LISTENER, "PROTOCOL_VERSION", "remote/listener.rs"),
            "the desktop and the phone must speak one protocol"
        );
    }

    /// Session certificate validity.
    ///
    /// Ten years on both sides, and the reason is asymmetric enough to be
    /// worth pinning: this crate's comment says "Long, like the
    /// desktop's, because the desktop pins the fingerprint and checks no
    /// dates; an expiry would silently unpair the phone." That argument
    /// depends on the desktop's value, so it is the desktop's value this
    /// reads.
    #[test]
    fn the_certificate_validity_matches_the_desktop() {
        assert_eq!(
            crate::keys::VALIDITY_YEARS as u64,
            int_const(DESKTOP_IDENTITY, "VALIDITY_YEARS", "remote/identity.rs"),
            "a certificate that outlives the desktop's would silently unpair \
             the phone at an hour nobody chose"
        );
    }
}
