//! Tests for the advice report cache (#1293).
//!
//! # How "the producers did not re-run" is PROVEN, not asserted
//!
//! `Freshness::Fresh { recomputed: false }` is the code's own claim, and
//! a test that only read the flag would pass against a `serve` that ran
//! the producers and then lied about it. So the hit tests store a report
//! carrying a SENTINEL finding that no producer can emit -- its
//! `finding` sentence exists nowhere in the source -- and assert the
//! sentinel comes back. A run would have replaced it, whatever the flag
//! said.
//!
//! The converse, "a change DID recompute", is proven the same way: the
//! sentinel must be GONE.

use super::*;
use crate::claudemd::advice::{
    Check, CheckCoverage, CheckRun, Evidence, Finding, Locator, Report, Severity, Subject,
};
use crate::claudemd::{scan_effective_opt, Scope};
use rusqlite::Connection;
use std::path::PathBuf;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    conn
}

/// A sentence no producer in the tree emits, so its survival through a
/// `serve` proves the producers did not run.
const SENTINEL: &str = "SENTINEL: this sentence is written only by the cache tests";

fn sentinel_finding(repo: &Path) -> Finding {
    Finding::new(
        Check::Imports,
        Severity::Problem,
        Subject::ClaudeMd {
            path: repo.join("CLAUDE.md").to_string_lossy().into_owned(),
            scope: Scope::Repo,
            section: None,
        },
        vec![Evidence {
            at: Locator::File {
                path: repo.join("CLAUDE.md").to_string_lossy().into_owned(),
                line: Some(1),
            },
            measured: "written by the test, never by a producer".into(),
        }],
        SENTINEL.into(),
    )
}

/// A report carrying the sentinel, and whatever coverage the caller
/// wants. Every check is `Ran` unless `unknown` names it.
fn sentinel_report(repo: &Path, unknown: &[(Check, &str)]) -> Report {
    let checks = Check::ALL
        .iter()
        .map(|&check| CheckCoverage {
            check,
            run: match unknown.iter().find(|(c, _)| *c == check) {
                Some((_, reason)) => CheckRun::Unknown {
                    reason: (*reason).to_string(),
                },
                None => CheckRun::Ran { findings: 0 },
            },
        })
        .collect();
    let mut r = Report {
        repo: repo.to_string_lossy().into_owned(),
        findings: vec![sentinel_finding(repo)],
        checks,
        brief: String::new(),
    };
    r.brief = crate::claudemd::advice::brief::render_report(&r);
    r
}

/// A repository with one CLAUDE.md, and the context over it.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    conn: Connection,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        std::fs::write(repo.join("CLAUDE.md"), "# Root\n\nBuild with `make`.\n").unwrap();
        Fixture {
            _dir: dir,
            repo,
            conn: db(),
        }
    }

    fn serve(&self, mode: Mode, now: &str) -> AdviceResult {
        // The scan is rebuilt per call on purpose: that is what the
        // command does, and the fingerprint is taken over it.
        let scan = scan_effective_opt(&self.repo, None);
        let cx = Context {
            repo: &self.repo,
            home: None,
            scan: &scan,
            definitions: None,
            conn: Some(&self.conn),
        };
        super::serve(&cx, mode, now)
    }

    fn fingerprint(&self) -> Fingerprint {
        let scan = scan_effective_opt(&self.repo, None);
        Fingerprint::of(&self.repo, None, &scan, None, Some(&self.conn))
    }

    /// Put a sentinel-bearing report in the cache, stamped with the
    /// CURRENT fingerprint so a `Cached` read is a hit.
    fn seed(&self, unknown: &[(Check, &str)], at: &str) {
        let fp = self.fingerprint();
        store(
            &self.conn,
            &self.repo,
            &sentinel_report(&self.repo, unknown),
            &fp,
            at,
        )
        .unwrap();
    }

    fn has_sentinel(r: &AdviceResult) -> bool {
        r.report.findings.iter().any(|f| f.finding == SENTINEL)
    }
}

/// Assert the full contract for a CHANGED tracked input.
///
/// Three things, and all three matter:
///
/// 1. `Mode::Cached` still answers immediately, with the previous run.
///    Withholding it would put the expensive pass back on the path
///    #1293 exists to take it off.
/// 2. That answer is LABELLED `cached { stale: true }` -- the change was
///    detected. A stale report served as `fresh` is the failure this
///    whole issue is about.
/// 3. `Mode::Fresh` then actually re-runs the producers, proven by the
///    sentinel being gone rather than by the flag the code sets.
fn assert_change_is_detected_and_recomputes(f: &Fixture) {
    let cached = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert!(
        Fixture::has_sentinel(&cached),
        "a previous run was withheld instead of being served and labelled"
    );
    assert_eq!(
        cached.freshness,
        Freshness::Cached { stale: true },
        "a changed input was not reported as stale -- this is the #1293 failure"
    );
    assert_eq!(
        cached.computed_at, "2026-01-01T00:00:00Z",
        "a stale report must carry the time the PRODUCERS ran, not now"
    );

    let fresh = f.serve(Mode::Fresh, "2026-06-01T00:00:00Z");
    assert!(
        !Fixture::has_sentinel(&fresh),
        "the refresh did not re-run the producers"
    );
    assert_eq!(fresh.freshness, Freshness::Fresh { recomputed: true });

    // And the refresh REPLACED the entry, so the next open is a clean
    // hit rather than a second stale answer.
    let after = f.serve(Mode::Cached, "2026-07-01T00:00:00Z");
    assert_eq!(
        after.freshness,
        Freshness::Fresh { recomputed: false },
        "the refresh did not update the stored fingerprint"
    );
}

/// ACCEPTANCE: a second open of an unchanged repository does not re-run
/// the producers.
///
/// Proven by the sentinel, not by the flag. The seeded report holds a
/// finding no producer emits; if `serve` ran them it would be gone.
#[test]
fn an_unchanged_repository_is_served_without_running_the_producers() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");

    let got = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");

    assert!(
        Fixture::has_sentinel(&got),
        "the producers ran: the stored report was replaced"
    );
    assert_eq!(
        got.freshness,
        Freshness::Fresh { recomputed: false },
        "an unchanged repository is verified current, not merely cached"
    );
    assert_eq!(
        got.computed_at, "2026-01-01T00:00:00Z",
        "`computed_at` is when the PRODUCERS ran, not when this call answered"
    );
}

/// ACCEPTANCE: touching a tracked input causes a recompute.
///
/// The edit changes CONTENT, so this also pins that content is what is
/// hashed. `touching_without_editing_does_not_recompute` below pins the
/// other half of the mtime-vs-hash choice.
#[test]
fn editing_a_claude_md_recomputes() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");

    std::fs::write(f.repo.join("CLAUDE.md"), "# Root\n\nBuild with `just`.\n").unwrap();
    assert_change_is_detected_and_recomputes(&f);
}

/// A NEW CLAUDE.md is a tracked input too, not only an edit to a known
/// one. Without this, the fingerprint could hash only the paths it had
/// last time and call a new file no change.
#[test]
fn adding_a_claude_md_recomputes() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");

    std::fs::create_dir_all(f.repo.join("src")).unwrap();
    std::fs::write(f.repo.join("src/CLAUDE.md"), "# src\n").unwrap();
    assert_change_is_detected_and_recomputes(&f);
}

/// The other half of the content-hash choice: `touch` without an edit is
/// NOT a change.
///
/// This is the case `mtime` gets wrong, and the reason #1293 asked for
/// the choice to be justified. A `git checkout` rewrites timestamps on
/// files whose bytes did not move, and an mtime cache would re-run the
/// whole-body transcript pass for every one of them.
#[test]
fn touching_without_editing_does_not_recompute() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");
    let before = f.fingerprint();

    // Rewrite the SAME bytes, which moves mtime and leaves content put.
    let same = std::fs::read(f.repo.join("CLAUDE.md")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(f.repo.join("CLAUDE.md"), &same).unwrap();

    let after = f.fingerprint();
    assert_eq!(
        before.digest, after.digest,
        "the same bytes at a new mtime must not read as a change"
    );

    let got = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert!(
        Fixture::has_sentinel(&got),
        "a touch-without-edit re-ran the producers"
    );
}

/// ACCEPTANCE: the three freshness states are distinguishable.
///
/// One test over all three, because the property is that they DIFFER --
/// three tests each asserting one value would pass against a `serve`
/// that returned the same variant for two different situations.
#[test]
fn the_three_freshness_states_are_distinguishable() {
    // 1: fresh. Nothing stored, so the producers run.
    let fresh = Fixture::new();
    let a = fresh.serve(Mode::Cached, "2026-01-01T00:00:00Z");
    assert_eq!(a.freshness, Freshness::Fresh { recomputed: true });

    // 1 again, by the other route: stored and verified current.
    let verified = Fixture::new();
    verified.seed(&[], "2026-01-01T00:00:00Z");
    let b = verified.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert_eq!(b.freshness, Freshness::Fresh { recomputed: false });

    // 2: from cache, and known to be out of date. This is what a caller
    // composes "from cache, refreshing" out of -- it shows this report
    // and fires a `Mode::Fresh` call behind it.
    let stale = Fixture::new();
    stale.seed(&[], "2026-01-01T00:00:00Z");
    std::fs::write(stale.repo.join("CLAUDE.md"), "# changed\n").unwrap();
    let c2 = stale.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert_eq!(c2.freshness, Freshness::Cached { stale: true });

    // 3: unverified. An unreadable tracked input, so currency is
    // UNKNOWN -- and that is not state 1.
    let unverified = Fixture::new();
    unverified.seed(&[], "2026-01-01T00:00:00Z");
    unreadable(&unverified.repo.join("CLAUDE.md"));
    let c3 = unverified.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    restore(&unverified.repo.join("CLAUDE.md"));

    match &c3.freshness {
        Freshness::Unverified { reason, .. } => {
            assert!(
                reason.contains("CLAUDE.md"),
                "the reason must name the input it could not read, got {reason:?}"
            );
        }
        other => panic!("an unreadable tracked input reported {other:?}, not Unverified"),
    }
    // The states are PAIRWISE distinct, which is the actual property --
    // three separate assertions of one value each would pass against a
    // `serve` that returned the same variant for two situations.
    assert_ne!(
        b.freshness, c2.freshness,
        "verified-current is not stale-cache"
    );
    assert_ne!(
        b.freshness, c3.freshness,
        "verified-current is not could-not-verify"
    );
    assert_ne!(
        c2.freshness, c3.freshness,
        "stale-cache is not could-not-verify"
    );
    assert_ne!(
        c3.freshness,
        Freshness::Fresh { recomputed: true },
        "could-not-verify is not fresh, even when the producers just ran"
    );
    // `a` is the fourth distinguishable thing: computed now, which a
    // caller tells from verified-current by `recomputed`.
    assert_ne!(
        a.freshness, b.freshness,
        "computed-now is not verified-current"
    );
}

/// A run that JUST happened can still be unverified.
///
/// The case a `recomputed => fresh` shortcut would get wrong: the
/// producers ran, so the report is the newest available, and the
/// fingerprint stored beside it still omits an input -- so the NEXT open
/// cannot use it to prove anything either.
#[test]
fn a_recomputed_report_over_an_unreadable_input_is_unverified_not_fresh() {
    let f = Fixture::new();
    unreadable(&f.repo.join("CLAUDE.md"));
    let got = f.serve(Mode::Fresh, "2026-01-01T00:00:00Z");
    restore(&f.repo.join("CLAUDE.md"));

    match &got.freshness {
        Freshness::Unverified { recomputed, .. } => assert!(*recomputed),
        other => panic!("expected Unverified, got {other:?}"),
    }

    // And the unverifiedness STICKS: the row records it, so a later open
    // that CAN read everything still knows the digest it is comparing
    // against was never complete.
    let stored = load(&f.conn, &f.repo).unwrap().unwrap();
    assert!(
        stored.stored_unverified.is_some(),
        "a report computed under an incomplete fingerprint must be filed as such"
    );
    let later = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert!(
        matches!(later.freshness, Freshness::Unverified { .. }),
        "a report stored unverified is unverified on every later open, got {:?}",
        later.freshness
    );
}

/// ACCEPTANCE, and the worst case #1293 names: a cached PARTIAL report
/// must not come back looking complete.
///
/// Every `CheckRun::Unknown` reason must survive the round-trip
/// verbatim. A cache that dropped `checks`, defaulted a missing one to
/// `Ran`, or lost the reason string would turn "three checks could not
/// run" into "nothing found", which the panel renders as good news.
#[test]
fn a_partial_report_survives_the_round_trip_with_its_reasons_intact() {
    let f = Fixture::new();
    let reasons = [
        (
            Check::Transcripts,
            "the transcripts directory could not be listed: Permission denied",
        ),
        (
            Check::Skills,
            "~/.claude: no home directory is set, so no skill could be resolved",
        ),
    ];
    f.seed(&reasons, "2026-01-01T00:00:00Z");

    let got = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");

    assert!(
        Fixture::has_sentinel(&got),
        "precondition: this must be a cache hit, not a recompute"
    );
    assert!(
        got.report.is_partial(),
        "a partial report came back reporting itself complete"
    );
    assert_eq!(
        got.report.ran(),
        Check::ALL.len() - reasons.len(),
        "the count of checks that ran must not absorb the ones that could not"
    );
    for (check, reason) in reasons {
        let coverage = got
            .report
            .checks
            .iter()
            .find(|c| c.check == check)
            .unwrap_or_else(|| panic!("{check:?} is missing from the round-tripped coverage"));
        assert_eq!(
            coverage.run,
            CheckRun::Unknown {
                reason: reason.to_string()
            },
            "{check:?} lost its Unknown reason through the cache"
        );
    }
    // The brief is what gets copied to an agent, so its "could not
    // check" lines must survive too.
    for (_, reason) in reasons {
        assert!(
            got.report.brief.contains(reason),
            "the round-tripped brief dropped `{reason}`"
        );
    }
}

/// ACCEPTANCE: a cache entry that cannot be deserialized falls back to
/// recompute -- never a crash, never a half-decoded report.
///
/// Written as raw JSON that is well-formed but not a `Report`, which is
/// what an app upgrade that renamed a field leaves behind.
#[test]
fn an_undeserializable_entry_falls_back_to_recompute() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");
    // A `Report`-shaped object from a hypothetical future release:
    // `findings` renamed, `checks` carrying a variant this build has
    // never heard of.
    f.conn
        .execute(
            "UPDATE claude_advice_report SET payload = ?1 WHERE repo = ?2",
            rusqlite::params![
                r#"{"repo":"/x","observations":[],"checks":[{"check":"quantumFlux",
                    "run":{"state":"ran","findings":0}}],"brief":"x"}"#,
                f.repo.to_string_lossy().to_string(),
            ],
        )
        .unwrap();

    assert!(
        load(&f.conn, &f.repo).unwrap().is_none(),
        "a payload that will not decode must read as a MISS, not an error and not a partial"
    );

    let got = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert!(!Fixture::has_sentinel(&got));
    assert_eq!(got.freshness, Freshness::Fresh { recomputed: true });

    // And the bad row is REPLACED, so it cannot fail the same way on
    // every open forever.
    let stored = load(&f.conn, &f.repo).unwrap();
    assert!(
        stored.is_some(),
        "the undecodable row was not overwritten by the recompute"
    );
}

/// A row from a different payload version is a miss too, and it is
/// checked BEFORE the decode.
///
/// The case a decode check alone misses: a report that still decodes and
/// no longer means the same thing, because a producer's rule changed
/// under it.
#[test]
fn a_row_at_another_payload_version_is_a_miss() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");
    f.conn
        .execute(
            "UPDATE claude_advice_report SET payload_version = ?1",
            [PAYLOAD_VERSION + 1],
        )
        .unwrap();

    assert!(load(&f.conn, &f.repo).unwrap().is_none());
    let got = f.serve(Mode::Cached, "2026-06-01T00:00:00Z");
    assert!(!Fixture::has_sentinel(&got));
}

/// `Mode::Fresh` runs the producers even when the cache is current.
///
/// The reason `mode` is a parameter rather than a heuristic: Refresh is
/// pressed precisely when the inputs have not moved and the user wants
/// the answer re-checked anyway.
#[test]
fn fresh_mode_runs_even_on_a_verified_hit() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");
    // Precondition: Cached would have been a hit.
    assert!(Fixture::has_sentinel(
        &f.serve(Mode::Cached, "2026-06-01T00:00:00Z")
    ));

    let got = f.serve(Mode::Fresh, "2026-06-02T00:00:00Z");
    assert!(
        !Fixture::has_sentinel(&got),
        "Fresh served the cache: Refresh is a no-op"
    );
    assert_eq!(got.freshness, Freshness::Fresh { recomputed: true });
    assert_eq!(got.computed_at, "2026-06-02T00:00:00Z");
}

/// A session appearing under the repository is a tracked input.
///
/// The transcripts producer reads sessions, so a new one changes its
/// answer. Without this the `(size, mtime)` half of the fingerprint
/// could be absent and every other test would still pass.
#[test]
fn a_new_session_under_the_repository_recomputes() {
    let f = Fixture::new();
    f.seed(&[], "2026-01-01T00:00:00Z");

    let transcript = f.repo.join("session.jsonl");
    std::fs::write(&transcript, "{}\n").unwrap();
    f.conn
        .execute(
            "INSERT INTO claude_session (session_id, cwd, transcript_path, first_seen_at)
             VALUES ('s1', ?1, ?2, '2026-01-01T00:00:00Z')",
            rusqlite::params![
                f.repo.to_string_lossy().to_string(),
                transcript.to_string_lossy().to_string()
            ],
        )
        .unwrap();

    assert_change_is_detected_and_recomputes(&f);
}

/// A session that GREW is a tracked change, which is the `(size, mtime)`
/// key doing its job on an append-mostly log.
#[test]
fn a_grown_transcript_recomputes() {
    let f = Fixture::new();
    let transcript = f.repo.join("session.jsonl");
    std::fs::write(&transcript, "{}\n").unwrap();
    f.conn
        .execute(
            "INSERT INTO claude_session (session_id, cwd, transcript_path, first_seen_at)
             VALUES ('s1', ?1, ?2, '2026-01-01T00:00:00Z')",
            rusqlite::params![
                f.repo.to_string_lossy().to_string(),
                transcript.to_string_lossy().to_string()
            ],
        )
        .unwrap();
    f.seed(&[], "2026-01-01T00:00:00Z");

    std::fs::write(&transcript, "{}\n{\"more\":true}\n").unwrap();

    assert_change_is_detected_and_recomputes(&f);
}

/// A definition added under the repository's `.claude` is a tracked
/// input.
///
/// #1293 offered "track only CLAUDE.md mtimes" as an acceptable trade,
/// noting it leaves stale advice after a new skill. The trade is NOT
/// taken, and this is what says so: the skills and rot producers read
/// the inventory, and a skill that appeared must move their answer.
#[test]
fn a_new_skill_in_the_inventory_recomputes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    std::fs::write(repo.join("CLAUDE.md"), "# Root\n").unwrap();
    let conn = db();

    let inventory = |repo: &Path| {
        use crate::claude::definitions as defs;
        let roots = defs::roots(None, std::slice::from_ref(&repo.to_path_buf()), &[]);
        defs::scan_scopes(&roots)
    };

    let scan = scan_effective_opt(&repo, None);
    let before = Fingerprint::of(&repo, None, &scan, Some(&inventory(&repo)), Some(&conn));

    let skill = repo.join(".claude/skills/verify");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: verify\ndescription: run the gate\n---\n\nRun `make lint`.\n",
    )
    .unwrap();

    let after = Fingerprint::of(&repo, None, &scan, Some(&inventory(&repo)), Some(&conn));
    assert_ne!(
        before.digest, after.digest,
        "a new skill left the fingerprint unchanged, so stale advice would be served as current"
    );
}

/// Editing a skill's BODY is a change, not only adding one.
#[test]
fn editing_a_skill_recomputes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    std::fs::write(repo.join("CLAUDE.md"), "# Root\n").unwrap();
    let skill = repo.join(".claude/skills/verify");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: verify\ndescription: a\n---\n\nRun `make lint`.\n",
    )
    .unwrap();
    let conn = db();

    use crate::claude::definitions as defs;
    let roots = defs::roots(None, std::slice::from_ref(&repo), &[]);
    let scan = scan_effective_opt(&repo, None);
    let before = Fingerprint::of(
        &repo,
        None,
        &scan,
        Some(&defs::scan_scopes(&roots)),
        Some(&conn),
    );

    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: verify\ndescription: a\n---\n\nRun `yarn lint`.\n",
    )
    .unwrap();

    let after = Fingerprint::of(
        &repo,
        None,
        &scan,
        Some(&defs::scan_scopes(&roots)),
        Some(&conn),
    );
    assert_ne!(before.digest, after.digest, "an edited skill hashed equal");
}

/// The fingerprint is stable across calls when nothing moved.
///
/// The guard against a digest built from an unordered walk: a cache that
/// recomputes every time is not a cache, and it would fail silently --
/// the app would just be as slow as before #1293.
#[test]
fn an_unchanged_repository_fingerprints_identically() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.repo.join("a")).unwrap();
    std::fs::create_dir_all(f.repo.join("b")).unwrap();
    std::fs::write(f.repo.join("a/CLAUDE.md"), "# a\n").unwrap();
    std::fs::write(f.repo.join("b/CLAUDE.md"), "# b\n").unwrap();

    assert_eq!(f.fingerprint().digest, f.fingerprint().digest);
    assert_eq!(f.fingerprint().verification, Verification::Complete);
}

/// A store-less run is COMPLETE, not unverified.
///
/// Two different facts that a lazy `Option` handling would collapse:
/// "there is no session input to track" and "there is one and it would
/// not read". Only the second is unverified.
#[test]
fn no_store_is_a_complete_fingerprint_not_an_unverified_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "# Root\n").unwrap();
    let scan = scan_effective_opt(dir.path(), None);

    let fp = Fingerprint::of(dir.path(), None, &scan, None, None);
    assert_eq!(fp.verification, Verification::Complete);

    // And it is a DIFFERENT digest from the same repository with a
    // store, because the input set differs.
    let conn = db();
    let with = Fingerprint::of(dir.path(), None, &scan, None, Some(&conn));
    assert_ne!(
        fp.digest, with.digest,
        "a run with a store and one without must not share a digest"
    );
}

/// Two repositories do not share a cache row.
#[test]
fn each_repository_has_its_own_entry() {
    let a = Fixture::new();
    let b = Fixture::new();
    // One connection, two repositories -- the real shape, since the
    // store is per install.
    let fp = a.fingerprint();
    store(
        &a.conn,
        &a.repo,
        &sentinel_report(&a.repo, &[]),
        &fp,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    store(
        &a.conn,
        &b.repo,
        &sentinel_report(&b.repo, &[]),
        &fp,
        "2026-01-02T00:00:00Z",
    )
    .unwrap();

    assert_eq!(
        load(&a.conn, &a.repo).unwrap().unwrap().computed_at,
        "2026-01-01T00:00:00Z"
    );
    assert_eq!(
        load(&a.conn, &b.repo).unwrap().unwrap().computed_at,
        "2026-01-02T00:00:00Z"
    );
}

/// The hash is length-prefixed, so a path/content boundary cannot be
/// moved without changing the digest.
///
/// Without the prefix, `"a" + "bc"` and `"ab" + "c"` hash equal, and a
/// rename that shifted one character between a path and the content
/// after it would read as no change.
#[test]
fn the_input_record_is_length_prefixed() {
    let mut a = Sha256::new();
    feed(&mut a, b"a");
    feed(&mut a, b"bc");
    let mut b = Sha256::new();
    feed(&mut b, b"ab");
    feed(&mut b, b"c");
    assert_ne!(hex(a.finalize().as_slice()), hex(b.finalize().as_slice()));
}

/// An unreadable CLAUDE.md is BOTH a changed digest and an unverified
/// one.
///
/// Not either-or: the file going unreadable IS a change (the content we
/// hashed is no longer what we can see), and it also means this digest
/// is not a complete statement. A fingerprint that only did one of the
/// two would either serve the old report as current or would recompute
/// and then call the result verified.
#[test]
fn an_unreadable_input_both_changes_the_digest_and_flags_it() {
    let f = Fixture::new();
    let before = f.fingerprint();
    assert_eq!(before.verification, Verification::Complete);

    unreadable(&f.repo.join("CLAUDE.md"));
    let after = f.fingerprint();
    restore(&f.repo.join("CLAUDE.md"));

    assert_ne!(before.digest, after.digest);
    match after.verification {
        Verification::Unverified { reason } => assert!(reason.contains("CLAUDE.md")),
        Verification::Complete => panic!("an unreadable input hashed as a complete statement"),
    }
}

/// Make a file unreadable, on the platforms where that is possible.
///
/// `chmod 000` does nothing for root, and Windows has no equivalent, so
/// both are skipped rather than faked: a test that pretended would pin
/// nothing. The tests that call this assert on the RESULT of the read
/// failing, so on a platform where it does not fail they assert the
/// readable path, which is still true.
#[cfg(unix)]
fn unreadable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
}

#[cfg(unix)]
fn restore(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
}

#[cfg(not(unix))]
fn unreadable(path: &Path) {
    // Deleting is the portable "cannot be read", and it is a real case:
    // the scan listed the file and it was removed before the
    // fingerprint read it.
    std::fs::remove_file(path).unwrap();
}

#[cfg(not(unix))]
fn restore(_path: &Path) {}

/// A missing home directory is a STATED ABSENCE, not a failed read.
///
/// `scan_effective_opt` records "no home directory is set, so the global
/// scope could not be read" in the same list that holds real read
/// failures. Treating the two alike would make every home-less run
/// permanently unverified, which drains the third state of meaning --
/// and it would be #1050's error exactly: reporting "we did not ask" as
/// "they did not answer".
///
/// The two ARMS of the assertion are the test: with no home the run is
/// Complete; with a home whose `.claude` cannot be listed it is not.
#[test]
fn a_missing_home_is_complete_and_an_unreadable_home_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("CLAUDE.md"), "# Root\n").unwrap();
    let conn = db();

    let no_home = scan_effective_opt(&repo, None);
    assert!(
        !no_home.unreadable.is_empty(),
        "precondition: the scan records the scope it could not look for"
    );
    assert_eq!(
        Fingerprint::of(&repo, None, &no_home, None, Some(&conn)).verification,
        Verification::Complete,
        "an absent home was reported as a read that failed"
    );

    // A home that EXISTS and holds a CLAUDE.md that will not read.
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/CLAUDE.md"), "# Global\n").unwrap();
    unreadable(&home.join(".claude/CLAUDE.md"));
    let with_home = scan_effective_opt(&repo, Some(&home));
    let fp = Fingerprint::of(&repo, Some(home.as_path()), &with_home, None, Some(&conn));
    restore(&home.join(".claude/CLAUDE.md"));

    assert!(
        matches!(fp.verification, Verification::Unverified { .. }),
        "a global CLAUDE.md that would not read was reported as a complete statement, got {:?}",
        fp.verification
    );
}

/// The unverified reason NAMES the input and counts the rest.
///
/// A reason of "some inputs could not be read" is not actionable, and a
/// wall of every path is not readable. One verbatim, plus a count, is
/// the shape the rest of this codebase uses for a floor.
#[test]
fn the_unverified_reason_names_one_input_and_counts_the_others() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.repo.join("a")).unwrap();
    std::fs::create_dir_all(f.repo.join("b")).unwrap();
    std::fs::write(f.repo.join("a/CLAUDE.md"), "# a\n").unwrap();
    std::fs::write(f.repo.join("b/CLAUDE.md"), "# b\n").unwrap();
    unreadable(&f.repo.join("a/CLAUDE.md"));
    unreadable(&f.repo.join("b/CLAUDE.md"));

    let fp = f.fingerprint();
    restore(&f.repo.join("a/CLAUDE.md"));
    restore(&f.repo.join("b/CLAUDE.md"));

    match fp.verification {
        Verification::Unverified { reason } => {
            assert!(reason.contains("CLAUDE.md"), "{reason}");
            assert!(
                reason.contains("and 1 more"),
                "two unreadable inputs must not read as one: {reason}"
            );
        }
        Verification::Complete => panic!("two unreadable inputs hashed as complete"),
    }
}

/// The wire shape of `AdviceResult` matches `ClaudeMdAdviceResult` in
/// `src/types/pr.ts`, field for field.
///
/// #1288 is a live crash caused by exactly this disagreeing: a
/// `rename_all = "camelCase"` attribute and a TypeScript interface that
/// did not match. This asserts the JSON the frontend will actually
/// receive, not the Rust field names, so a `serde` attribute changed or
/// forgotten here fails a test rather than a user's panel.
#[test]
fn the_wire_shape_matches_the_typescript_interface() {
    let f = Fixture::new();
    let v = serde_json::to_value(f.serve(Mode::Fresh, "2026-01-01T00:00:00Z")).unwrap();
    let obj = v.as_object().unwrap();

    // `computedAt`, not `computed_at`. The one camelCase rename on this
    // struct, and the one a missing attribute would break.
    assert!(obj.contains_key("computedAt"), "{v}");
    assert!(!obj.contains_key("computed_at"), "{v}");
    assert!(obj.contains_key("report"));
    assert!(obj.contains_key("freshness"));

    // `Freshness` is internally tagged on `state`, which is what the
    // TypeScript discriminated union switches on.
    let fresh = &obj["freshness"];
    assert_eq!(fresh["state"], "fresh", "{v}");
    assert_eq!(fresh["recomputed"], true, "{v}");

    // The other two members, by construction rather than by a run, so
    // every arm of the union is asserted.
    let cached = serde_json::to_value(Freshness::Cached { stale: true }).unwrap();
    assert_eq!(
        cached,
        serde_json::json!({ "state": "cached", "stale": true })
    );
    let unverified = serde_json::to_value(Freshness::Unverified {
        reason: "x: Permission denied".into(),
        recomputed: false,
    })
    .unwrap();
    assert_eq!(
        unverified,
        serde_json::json!({
            "state": "unverified",
            "reason": "x: Permission denied",
            "recomputed": false
        })
    );

    // `Mode` is what the frontend SENDS, so its two values must be the
    // strings `ClaudeMdAdviceMode` names.
    assert_eq!(serde_json::to_value(Mode::Cached).unwrap(), "cached");
    assert_eq!(serde_json::to_value(Mode::Fresh).unwrap(), "fresh");
    // And they must decode back, because the command takes `Option<Mode>`
    // off the wire.
    assert_eq!(
        serde_json::from_value::<Option<Mode>>(serde_json::json!(null)).unwrap(),
        None
    );
    assert_eq!(
        serde_json::from_value::<Mode>(serde_json::json!("fresh")).unwrap(),
        Mode::Fresh
    );
}
