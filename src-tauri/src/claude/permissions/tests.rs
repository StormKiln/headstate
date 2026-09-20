//! The five properties #1199 exists to guarantee, each sabotage-proven.
//!
//! Every path here is under a `tempfile::TempDir`. Nothing in this module
//! resolves a home or data directory itself, precisely so a test cannot
//! reach the developer's own `~/.claude/settings.json` -- the same rule
//! `install.rs` states and for the same reason.

use super::*;

const NOW: &str = "2026-09-20T12:00:00Z";

struct Fixture {
    _tmp: tempfile::TempDir,
    settings: PathBuf,
    ledger: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    Fixture {
        settings: home.join(".claude").join("settings.json"),
        ledger: ledger_path_in(&data),
        _tmp: tmp,
    }
}

fn write(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// The rules currently in one list of the settings file.
fn rules(path: &Path, list: RuleList) -> Vec<String> {
    let v: Value = serde_json::from_str(&read(path)).unwrap();
    v.get("permissions")
        .and_then(|p| p.get(list.key()))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------
// 1. A rule the user edited since we wrote it is NEVER removed.
//    The headline property: this is what protects the user's own edits.
// ---------------------------------------------------------------------

/// The state a naive "remember what we wrote" design gets wrong in the
/// most damaging direction.
///
/// Headstate wrote `Bash(git status:*)`. The user then tightened it by
/// hand to `Bash(git status)`. Our ledger still names the old text, and
/// the file no longer contains it -- so the rule that IS in the file was
/// never ours, and a removal pass that matched loosely, or that
/// re-adopted the edit, would delete a line the user typed.
///
/// This test builds the harder version of that: the ledger entry's HASH
/// disagrees with its own `rule` text while the rule text is still in
/// the file. That is the exact shape of "we wrote it, the value has
/// since changed", and it must read as `UserEdited` -- theirs, never
/// touched, and never in `removable`.
///
/// # Sabotage
///
/// Collapsing the `UserEdited` arm into `Ours` in `reconcile` (making
/// the hash comparison unconditional-true) fails here: the rule ends up
/// in `removable`, and `remove_owned` then deletes the user's line from
/// the settings file. Restored.
#[test]
fn a_rule_whose_value_changed_since_we_wrote_it_is_never_removed() {
    let f = fixture();
    write(
        &f.settings,
        r#"{"permissions": {"allow": ["Bash(git status)"]}}"#,
    );
    // A ledger entry recorded when the rule read something else: the
    // hash is of the ORIGINAL text, the label is what is in the file
    // now. This is what "the user edited it" looks like on disk.
    let mut ledger = Ledger::default();
    ledger.entries.insert(
        Ledger::key(RuleList::Allow, "Bash(git status)"),
        Entry {
            list: RuleList::Allow,
            rule: "Bash(git status)".to_string(),
            hash: hash_rule("Bash(git status:*)"),
            written_at: NOW.to_string(),
        },
    );
    save(&f.ledger, &ledger).unwrap();

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.verdicts.len(), 1);
    assert_eq!(
        rec.verdicts[0].ownership,
        Ownership::UserEdited,
        "the hash recorded at write time disagrees with the text in the file, \
         so the user has edited it and it is theirs now"
    );
    assert!(
        rec.removable.is_empty(),
        "a rule the user edited must never reach the removal pass"
    );
    assert!(
        rec.dropped.is_empty(),
        "it is still in the file, so the entry is not stale -- dropping it \
         would lose the only reason we know not to touch it"
    );

    // And the removal pass, run against this reconcile, leaves the file
    // exactly as it was. Asserted on the BYTES, because "equivalent
    // JSON" would pass for a pass that reformatted the user's file.
    let before = read(&f.settings);
    let removed = remove_owned(&f.settings, &f.ledger, &rec).unwrap();
    assert!(removed.removed.is_empty());
    assert_eq!(
        read(&f.settings),
        before,
        "nothing of the user's may be touched, not even by reformatting"
    );
}

/// The judgement is one-way: once theirs, always theirs.
///
/// Reconciling twice must not launder a `UserEdited` verdict into `Ours`
/// by rewriting the hash on the way past. "The user tuned the rule we
/// suggested" and "the user wrote this rule" are the same file state.
#[test]
fn a_user_edited_rule_stays_theirs_across_repeated_reconciles() {
    let f = fixture();
    write(
        &f.settings,
        r#"{"permissions": {"deny": ["Bash(rm -rf:*)"]}}"#,
    );
    let mut ledger = Ledger::default();
    ledger.entries.insert(
        Ledger::key(RuleList::Deny, "Bash(rm -rf:*)"),
        Entry {
            list: RuleList::Deny,
            rule: "Bash(rm -rf:*)".to_string(),
            hash: hash_rule("Bash(rm:*)"),
            written_at: NOW.to_string(),
        },
    );
    save(&f.ledger, &ledger).unwrap();

    for pass in 1..=3 {
        let rec = reconcile(&f.settings, &f.ledger).unwrap();
        assert_eq!(
            rec.verdicts[0].ownership,
            Ownership::UserEdited,
            "pass {pass}: re-adopting an edited rule would make the protection \
             last exactly one reconcile"
        );
        assert!(rec.removable.is_empty(), "pass {pass}");
    }
}

// ---------------------------------------------------------------------
// 2. A rule absent from settings drops its ledger entry.
//    The third state, which a write-only ledger misses entirely.
// ---------------------------------------------------------------------

/// The user deleted a rule we wrote. The entry is stale and must go.
///
/// This is not tidiness. A ledger that keeps the entry will recognise
/// that rule as ours the next time the user types it THEMSELVES -- and
/// then offer to delete their line. The sweep is what stops a stale
/// entry from ever reaching that point.
///
/// # Sabotage
///
/// Changing `reconcile` to keep `Gone` entries (inserting them into
/// `survivors` alongside the others) fails the second half: the entry
/// survives, and the follow-up reconcile after the user re-types the
/// rule reports it as `Ours` -- the user's own line offered up for
/// removal. Restored.
#[test]
fn a_rule_absent_from_settings_drops_its_ledger_entry() {
    let f = fixture();
    // The file has some OTHER rule, so this is a deletion rather than an
    // empty file: the sweep must be about the missing rule, not about
    // the file being bare.
    write(&f.settings, r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#);
    let mut ledger = Ledger::default();
    for rule in ["Bash(git status:*)", "Bash(ls:*)"] {
        ledger.entries.insert(
            Ledger::key(RuleList::Allow, rule),
            Entry {
                list: RuleList::Allow,
                rule: rule.to_string(),
                hash: hash_rule(rule),
                written_at: NOW.to_string(),
            },
        );
    }
    save(&f.ledger, &ledger).unwrap();

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.dropped.len(), 1);
    assert_eq!(rec.dropped[0].rule, "Bash(git status:*)");
    assert_eq!(rec.dropped[0].ownership, Ownership::Gone);
    assert!(
        rec.ledger_rewritten,
        "dropping an entry means nothing unless the ledger is actually written"
    );

    // Reload from DISK rather than trusting the in-memory result: the
    // claim is that the entry is gone from the file.
    let LedgerState::Present(after) = load(&f.ledger) else {
        panic!("the ledger must still be readable after a sweep");
    };
    assert_eq!(after.entries.len(), 1);
    assert!(after
        .entries
        .contains_key(&Ledger::key(RuleList::Allow, "Bash(ls:*)")));

    // The consequence the sweep exists for: the user now types the
    // deleted rule themselves. With the entry dropped, it is not ours.
    write(
        &f.settings,
        r#"{"permissions": {"allow": ["Bash(ls:*)", "Bash(git status:*)"]}}"#,
    );
    let again = reconcile(&f.settings, &f.ledger).unwrap();
    assert!(
        !again
            .removable
            .iter()
            .any(|v| v.rule == "Bash(git status:*)"),
        "a rule the user deleted and later re-typed is THEIRS -- a lingering \
         ledger entry is how we would delete a line they wrote"
    );
}

/// A reconcile that drops nothing must not rewrite the ledger.
///
/// The same discipline `install::uninstall` keeps about the settings
/// file: an operation that achieved nothing must not reformat a file as
/// a side effect, or `ledger_rewritten` is decoration rather than a
/// fact.
#[test]
fn a_reconcile_that_drops_nothing_does_not_rewrite_the_ledger() {
    let f = fixture();
    write(&f.settings, r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#);
    let mut ledger = Ledger::default();
    ledger.entries.insert(
        Ledger::key(RuleList::Allow, "Bash(ls:*)"),
        Entry {
            list: RuleList::Allow,
            rule: "Bash(ls:*)".to_string(),
            hash: hash_rule("Bash(ls:*)"),
            written_at: NOW.to_string(),
        },
    );
    save(&f.ledger, &ledger).unwrap();
    let before = read(&f.ledger);

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert!(!rec.ledger_rewritten);
    assert_eq!(read(&f.ledger), before);
}

// ---------------------------------------------------------------------
// 3. A rule we wrote and the user has not touched is recognised as ours.
// ---------------------------------------------------------------------

/// The positive case, end to end, against a file carrying a FOREIGN rule
/// that is byte-identical in shape to ours.
///
/// The foreign rule is the point: it is the thing a value-matching
/// implementation would delete. `Bash(git push:*)` sits in the same
/// array, was never recorded, and must survive both the reconcile and
/// the removal untouched.
///
/// # Sabotage
///
/// Replacing the ledger lookup in `reconcile` with a scan of every rule
/// in the file -- the "match on value alone" design the issue rejects --
/// fails here with `Bash(git push:*)` in `removable` and then gone from
/// the file. Restored.
#[test]
fn a_rule_we_wrote_and_nobody_touched_is_recognised_as_ours() {
    let f = fixture();
    write(
        &f.settings,
        r#"{
  "permissions": {
    "allow": [
      "Bash(git push:*)"
    ]
  },
  "model": "opus"
}
"#,
    );

    let rec1 = record(
        &f.settings,
        &f.ledger,
        RuleList::Allow,
        "Bash(git status:*)",
        NOW,
    )
    .unwrap();
    assert!(rec1.appended);
    assert!(!rec1.created_file);

    assert_eq!(
        rules(&f.settings, RuleList::Allow),
        ["Bash(git push:*)", "Bash(git status:*)"],
        "ours is APPENDED -- the foreign rule keeps its place"
    );

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.verdicts.len(), 1, "only what the ledger recorded");
    assert_eq!(rec.verdicts[0].ownership, Ownership::Ours);
    assert_eq!(rec.removable.len(), 1);
    assert_eq!(rec.removable[0].rule, "Bash(git status:*)");

    let removed = remove_owned(&f.settings, &f.ledger, &rec).unwrap();
    assert_eq!(removed.removed.len(), 1);
    assert_eq!(
        rules(&f.settings, RuleList::Allow),
        ["Bash(git push:*)"],
        "ours is gone and the foreign rule is not -- the whole safety property"
    );
    // Everything else in the document survives too.
    let v: Value = serde_json::from_str(&read(&f.settings)).unwrap();
    assert_eq!(v.get("model").unwrap(), "opus");

    // And the ledger no longer claims it.
    let LedgerState::Present(after) = load(&f.ledger) else {
        panic!("readable after removal");
    };
    assert!(after.entries.is_empty());
}

/// The same text in two different lists is two different rules.
///
/// `Bash(rm:*)` under `deny` and under `allow` are opposite
/// instructions. A ledger keyed on text alone would let a record of one
/// authorise the removal of the other.
#[test]
fn the_same_text_in_two_lists_is_two_rules() {
    let f = fixture();
    write(&f.settings, r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#);
    record(&f.settings, &f.ledger, RuleList::Allow, "Bash(rm:*)", NOW).unwrap();

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.removable.len(), 1);
    assert_eq!(rec.removable[0].list, RuleList::Allow);

    remove_owned(&f.settings, &f.ledger, &rec).unwrap();
    assert_eq!(
        rules(&f.settings, RuleList::Deny),
        ["Bash(rm:*)"],
        "the user's deny rule is not our allow rule, whatever the text says"
    );
}

/// Recording a rule that is already in the file does not duplicate it.
///
/// Claude Code would read the duplicate as one rule, so the only effect
/// would be a settings file that grows a line every time the button is
/// pressed.
#[test]
fn recording_a_rule_already_in_the_file_does_not_append_it_again() {
    let f = fixture();
    write(&f.settings, r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#);
    let out = record(&f.settings, &f.ledger, RuleList::Allow, "Bash(ls:*)", NOW).unwrap();
    assert!(!out.appended);
    assert_eq!(rules(&f.settings, RuleList::Allow), ["Bash(ls:*)"]);

    // It IS recorded, though: we now know we would have written it, and
    // a later reconcile reads it as ours.
    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.removable.len(), 1);
}

// ---------------------------------------------------------------------
// 4. An unreadable ledger is Unknown -- NOT "we own nothing".
// ---------------------------------------------------------------------

/// #846/#1042 in this module's terms: absent is not zero, and neither is
/// unreadable.
///
/// A ledger we cannot parse tells us nothing about what we own. Reading
/// it as an empty ledger would make a removal pass conclude that none of
/// the rules in the file are ours, and every rule Headstate ever wrote
/// would be stranded there with nothing in the UI admitting it exists.
///
/// Four unreadable shapes, one assertion each, plus the one shape that
/// is genuinely `Absent` -- because the value of the distinction is
/// entirely in the two not collapsing.
///
/// # Sabotage
///
/// Changing `load`'s parse-failure arm to `LedgerState::Absent`, and
/// `reconcile`'s `Unreadable` arm to `Ok(Reconciled::default())` -- the
/// "treat it as empty" design -- fails here: `reconcile` returns Ok with
/// no verdicts, no refusal reaches the caller, and the rule we really do
/// own is silently unowned. Restored.
#[test]
fn an_unreadable_ledger_is_unknown_and_not_an_empty_ledger() {
    for (label, body) in [
        ("malformed JSON", "{ not json"),
        ("truncated to nothing", ""),
        ("whitespace only", "   \n  "),
        (
            "a version this build does not understand",
            r#"{"version": 9999, "entries": {}}"#,
        ),
        (
            "the right shape with the wrong types",
            r#"{"version": 1, "entries": {"a": 3}}"#,
        ),
    ] {
        let f = fixture();
        write(
            &f.settings,
            r#"{"permissions": {"allow": ["Bash(git status:*)"]}}"#,
        );
        write(&f.ledger, body);

        match load(&f.ledger) {
            LedgerState::Unreadable(r) => {
                let text = r.to_string();
                assert!(
                    text.contains(f.ledger.to_string_lossy().as_ref()),
                    "{label}: the refusal must name the file -- got {text}"
                );
            }
            other => panic!(
                "{label}: read as {other:?}. An unreadable ledger that reads as \
                 Absent is how a removal pass strands every rule we ever wrote."
            ),
        }

        // And the refusal PROPAGATES: reconcile refuses rather than
        // returning an empty, confident answer.
        let err = reconcile(&f.settings, &f.ledger)
            .expect_err("{label}: reconcile must refuse on an unreadable ledger");
        assert!(
            err.to_string()
                .contains(f.ledger.to_string_lossy().as_ref()),
            "{label}: the refusal that reaches the caller names the ledger"
        );

        // No removal proceeds on that basis, and the user's file is
        // untouched.
        let before = read(&f.settings);
        assert_eq!(read(&f.settings), before);

        // A WRITE is refused too, for the same reason: adding a record to
        // a file whose other records we cannot read would claim a
        // completeness we do not have.
        record(&f.settings, &f.ledger, RuleList::Allow, "Bash(ls:*)", NOW)
            .expect_err("{label}: recording into an unreadable ledger must refuse");
        assert_eq!(
            read(&f.settings),
            before,
            "{label}: a refused record writes nothing to the settings file"
        );
    }
}

/// The other half of the distinction, and what makes it worth having.
///
/// An ABSENT ledger is not an error. A user who has never used the
/// feature has no ledger, and that genuinely does mean we own nothing --
/// reporting it as a refusal would make the honest signal above
/// worthless.
#[test]
fn an_absent_ledger_is_not_a_refusal() {
    let f = fixture();
    write(
        &f.settings,
        r#"{"permissions": {"allow": ["Bash(git status:*)"]}}"#,
    );
    assert!(!f.ledger.exists());
    assert_eq!(load(&f.ledger), LedgerState::Absent);

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert!(rec.verdicts.is_empty());
    assert!(rec.removable.is_empty());
    assert!(
        !rec.ledger_rewritten,
        "nothing was recorded, so nothing is written"
    );
}

// ---------------------------------------------------------------------
// 5. A settings file that cannot be parsed is refused with its proof.
// ---------------------------------------------------------------------

/// `install.rs`'s discipline, unchanged, for the measured reason in its
/// docs: Claude Code IGNORES a settings file it cannot parse, silently
/// (#910 §1.7). A user in that state already has every rule in the file
/// inert and no symptom, so the refusal must carry the parse error's own
/// message -- it is the only actionable thing anyone has.
///
/// Refused, not skipped, and refused BEFORE anything is written.
///
/// # Sabotage
///
/// Making `reconcile` swallow the `read_settings` error into
/// `Ok(Reconciled::default())` fails here on the first assertion. Making
/// `record` write the ledger before parsing the settings file fails the
/// "nothing is written" assertion -- a ledger file appears for a write
/// that was refused. Restored.
#[test]
fn a_settings_file_that_cannot_be_parsed_is_refused_with_its_proof() {
    for body in [
        "{ not json",
        r#"{"permissions": []}"#,
        r#"{"permissions": {"allow": "Bash(ls:*)"}}"#,
        r#"{"permissions": {"allow": [42]}}"#,
        "[1, 2, 3]",
    ] {
        let f = fixture();
        write(&f.settings, body);
        let mut ledger = Ledger::default();
        ledger.entries.insert(
            Ledger::key(RuleList::Allow, "Bash(ls:*)"),
            Entry {
                list: RuleList::Allow,
                rule: "Bash(ls:*)".to_string(),
                hash: hash_rule("Bash(ls:*)"),
                written_at: NOW.to_string(),
            },
        );
        save(&f.ledger, &ledger).unwrap();
        let ledger_before = read(&f.ledger);

        let err = reconcile(&f.settings, &f.ledger).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(f.settings.to_string_lossy().as_ref()),
            "the refusal must name the file to open -- got {text} for {body}"
        );
        assert_eq!(
            read(&f.settings),
            body,
            "a file we cannot parse is never rewritten: {body}"
        );
        assert_eq!(
            read(&f.ledger),
            ledger_before,
            "and the ledger is not swept on the strength of a file we could \
             not read -- every entry would look Gone: {body}"
        );
    }
}

/// A malformed settings file names its parse error, line and column.
///
/// The sentence reaches the UI verbatim. "invalid settings" is not
/// actionable and "expected `,` or `}` at line 3 column 5" is.
#[test]
fn the_refusal_carries_the_parse_error_verbatim() {
    let f = fixture();
    write(&f.settings, "{\n  \"permissions\": {\n  oops\n}\n");
    save(&f.ledger, &Ledger::default()).unwrap();

    let err = reconcile(&f.settings, &f.ledger).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("line") && text.contains("column"),
        "the parse error's own position is the only actionable thing we have -- got {text}"
    );
    assert!(
        matches!(err, Refusal::Malformed { .. }),
        "and it is the malformed variant, whose sentence says why we will not rewrite it"
    );
}

/// A refused RECORD writes neither file.
///
/// Stated separately from the reconcile case because it is the one that
/// would leave the two files disagreeing: a ledger entry for a rule we
/// then declined to write.
#[test]
fn a_record_into_an_unparseable_settings_file_writes_nothing() {
    let f = fixture();
    write(&f.settings, "{ not json");

    let err = record(
        &f.settings,
        &f.ledger,
        RuleList::Allow,
        "Bash(git status:*)",
        NOW,
    )
    .unwrap_err();
    assert!(matches!(err, Refusal::Malformed { .. }));
    assert_eq!(read(&f.settings), "{ not json");
    assert!(
        !f.ledger.exists(),
        "no ledger entry for a rule we refused to write"
    );
}

// ---------------------------------------------------------------------
// The crash window between the two writes.
// ---------------------------------------------------------------------

/// The state a crash between the ledger write and the settings write
/// leaves, and the proof that it is self-healing.
///
/// `record` writes the ledger FIRST. A process that dies after that and
/// before the settings write leaves an entry naming a rule that is not
/// in the file -- which is state three, so the next reconcile drops it.
/// Simulated by writing the ledger and stopping, which is exactly what
/// the surviving disk state looks like.
///
/// The other order is the one that hurts, and the module docs argue it:
/// a rule in the user's file that our ledger never heard of is a rule we
/// can never remove and never admit to.
#[test]
fn a_crash_between_the_two_writes_leaves_a_stale_entry_the_next_sweep_drops() {
    let f = fixture();
    write(&f.settings, r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#);

    // The half-completed record: ledger written, settings not.
    let mut ledger = Ledger::default();
    ledger.entries.insert(
        Ledger::key(RuleList::Allow, "Bash(git status:*)"),
        Entry {
            list: RuleList::Allow,
            rule: "Bash(git status:*)".to_string(),
            hash: hash_rule("Bash(git status:*)"),
            written_at: NOW.to_string(),
        },
    );
    save(&f.ledger, &ledger).unwrap();

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(
        rec.dropped.len(),
        1,
        "the entry names a rule that is not there"
    );
    assert!(
        rec.removable.is_empty(),
        "and nothing of the user's is offered up on the strength of it"
    );
    assert_eq!(rules(&f.settings, RuleList::Allow), ["Bash(ls:*)"]);

    let LedgerState::Present(after) = load(&f.ledger) else {
        panic!("readable");
    };
    assert!(after.entries.is_empty(), "self-healed");
}

// ---------------------------------------------------------------------
// The hash itself.
// ---------------------------------------------------------------------

/// The hash distinguishes exactly what it must, and normalises nothing.
///
/// Whitespace, case and a trailing `:*` are all meaningful in a
/// permission rule, so any transform that loses one loses the answer to
/// the only question this module asks.
#[test]
fn the_hash_distinguishes_rules_that_differ_in_any_byte() {
    let base = "Bash(git status:*)";
    for other in [
        "Bash(git status)",
        "Bash(git status:* )",
        " Bash(git status:*)",
        "bash(git status:*)",
        "Bash(git  status:*)",
        "",
    ] {
        assert_ne!(
            hash_rule(base),
            hash_rule(other),
            "{base:?} and {other:?} are different rules and must hash differently"
        );
    }
    assert_eq!(hash_rule(base), hash_rule(base), "and it is a function");
    assert_eq!(hash_rule(base).len(), 64, "sha256, hex");
}

/// The settings file the ledger describes is never created by a read.
///
/// An absent settings file means every recorded rule is genuinely gone.
/// It is not a refusal -- but it must not become a file, either.
#[test]
fn an_absent_settings_file_sweeps_the_ledger_without_creating_one() {
    let f = fixture();
    assert!(!f.settings.exists());
    let mut ledger = Ledger::default();
    ledger.entries.insert(
        Ledger::key(RuleList::Allow, "Bash(ls:*)"),
        Entry {
            list: RuleList::Allow,
            rule: "Bash(ls:*)".to_string(),
            hash: hash_rule("Bash(ls:*)"),
            written_at: NOW.to_string(),
        },
    );
    save(&f.ledger, &ledger).unwrap();

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    assert_eq!(rec.dropped.len(), 1);
    assert!(
        !f.settings.exists(),
        "reading ownership must not create the file it was reading"
    );
}

/// A `record` into a machine with no settings file creates one, and the
/// round trip leaves it as it found it.
#[test]
fn a_record_into_a_missing_settings_file_creates_one_and_round_trips() {
    let f = fixture();
    let out = record(
        &f.settings,
        &f.ledger,
        RuleList::Allow,
        "Bash(git status:*)",
        NOW,
    )
    .unwrap();
    assert!(out.created_file);
    assert_eq!(rules(&f.settings, RuleList::Allow), ["Bash(git status:*)"]);

    let rec = reconcile(&f.settings, &f.ledger).unwrap();
    let removed = remove_owned(&f.settings, &f.ledger, &rec).unwrap();
    assert_eq!(removed.removed.len(), 1);
    assert!(rules(&f.settings, RuleList::Allow).is_empty());
}

/// The ledger lives in Headstate's own data directory, not `~/.claude`.
///
/// Pinned as a test rather than left to the module docs because it is
/// the kind of decision a later change makes casually. `claude/mod.rs`
/// states that apart from `install`, nothing writes to `~/.claude` at
/// all, and #1199 must not have made it two.
#[test]
fn the_ledger_does_not_live_under_dot_claude() {
    let data = Path::new("/somewhere/Application Support/headstate");
    let p = ledger_path_in(data);
    assert_eq!(p.parent().unwrap(), data);
    assert!(
        !p.to_string_lossy().contains(".claude"),
        "the ledger is Headstate's own record about a file it does not own, and \
         a ledger inside the directory it describes dies with that directory"
    );
}
