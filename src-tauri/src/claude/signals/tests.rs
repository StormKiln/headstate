//! Tests for the three signals #1065, #1066 and #1067 read.
//!
//! The staleness tests below are the ones with teeth; each names the
//! sabotage that was run against it and what was seen.

use super::*;
use crate::claude::handoff;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    conn
}

fn running() -> Liveness {
    Liveness::Running {
        pid: 4242,
        status: Some("idle".into()),
    }
}

fn dead() -> Liveness {
    Liveness::Dead {
        why: "its process is no longer running".into(),
    }
}

fn unknown() -> Liveness {
    Liveness::Unknown {
        why: "the live session registry could not be read".into(),
    }
}

/// Write one hook record through the REAL consume path.
///
/// Going through `handoff::consume` rather than inserting rows directly
/// is deliberate: it is the only way these tests also cover the
/// `write_record` routing that decides a point event is not a run. A
/// test that seeded `claude_hook_event` by hand would pass even if the
/// hook path never wrote to it.
fn feed(conn: &mut Connection, lines: &[String]) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = handoff::path_in(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let out = handoff::consume(conn, &path, handoff::Offset(0), &Default::default()).unwrap();
    assert!(
        out.unparseable.is_empty() && out.write_failures.is_empty(),
        "fixture did not store cleanly: {out:?}"
    );
}

fn line(event: &str, session: &str, at: &str, extra: &str) -> String {
    let sep = if extra.is_empty() { "" } else { "," };
    format!(
        r#"{{"v":1,"event":"{event}","session_id":"{session}","ppid":4242,"ts":"{at}"{sep}{extra}}}"#
    )
}

/// Read one session's events, with the run table folded in exactly as
/// production does it.
fn events_for(conn: &Connection, session: &str) -> Option<Events> {
    let mut map = events_by_session(conn).unwrap();
    let runs = newest_run_activity(conn).unwrap();
    for (sid, at) in &runs {
        if let Some(e) = map.get_mut(sid) {
            e.observe_activity(at);
        }
    }
    map.remove(session)
}

// ---------------------------------------------------------------
// #1065: compaction pressure.
// ---------------------------------------------------------------

#[test]
fn compactions_are_split_by_trigger() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:00:00+00:00",
                r#""trigger":"auto""#,
            ),
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:05:00+00:00",
                r#""trigger":"manual""#,
            ),
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:09:00+00:00",
                r#""trigger":"auto""#,
            ),
        ],
    );

    let c = events_for(&conn, "s1").unwrap().compactions().unwrap();
    assert_eq!(c.auto, 2);
    assert_eq!(c.manual, 1);
    assert_eq!(c.total(), 3);
    assert!(
        c.under_pressure(),
        "two auto-compactions is the repeated case #1065 asks to flag"
    );
}

/// ZERO compactions and NOT-RECORDED are different answers.
///
/// #1065 names this test by hand, and it is the root rule: a session
/// with no `PreCompact` row either never compacted or ran before the
/// hook existed, and on today's corpus it is always the second. A
/// `Some(0)` here would put "0 compactions" on 1,500 rows that were
/// never watched.
///
/// PROVEN BY SABOTAGE: replacing the empty check in `compactions` with
/// `Some(Compactions::default())` fails here with a `Some` where the
/// absence is required.
#[test]
fn zero_compactions_is_distinguishable_from_not_recorded() {
    let mut conn = db();
    // A session with hook events, but none of them compactions.
    feed(
        &mut conn,
        &[line(
            "Notification",
            "watched",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"auth_success""#,
        )],
    );

    let watched = events_for(&conn, "watched").unwrap();
    assert_eq!(
        watched.compactions(),
        None,
        "a session we watched but saw no compaction for still has no \
         compaction MEASUREMENT -- absent is not zero"
    );
    assert!(
        events_for(&conn, "never-seen").is_none(),
        "a session with no hook rows at all is absent from the map"
    );
}

/// A trigger that is neither `manual` nor `auto` renders as itself.
///
/// #1065 names this test too. The rule the epic states is that unknown
/// enum values render as themselves and never as "other", because a
/// vocabulary that grew is information and a bucket named "other"
/// destroys it.
///
/// PROVEN BY SABOTAGE: folding the `Some(other)` arm into `auto`
/// (`Some(_) => out.auto += 1`) fails here -- `unknown` comes back empty
/// and `auto` reads 1, so a trigger nobody has seen before would have
/// been silently reported as the session hitting a wall.
#[test]
fn an_unknown_trigger_renders_as_itself() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:00:00+00:00",
                r#""trigger":"emergency""#,
            ),
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:01:00+00:00",
                r#""trigger":"auto""#,
            ),
        ],
    );

    let c = events_for(&conn, "s1").unwrap().compactions().unwrap();
    assert_eq!(c.unknown, vec![("emergency".to_owned(), 1)]);
    assert_eq!(c.auto, 1, "the unknown value must not inflate a known one");
    assert_eq!(
        c.total(),
        2,
        "but it IS a compaction and counts in the total"
    );
    assert!(
        !c.under_pressure(),
        "one auto-compaction is not the repeated case, whatever else happened"
    );
}

/// A compaction record with no trigger at all is its own count.
///
/// Different from an unknown VALUE: the field was absent, which means
/// the payload shape moved rather than the vocabulary growing, and the
/// two have different remedies.
#[test]
fn a_compaction_with_no_trigger_is_counted_apart() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line("PreCompact", "s1", "2026-09-13T10:00:00+00:00", "")],
    );

    let c = events_for(&conn, "s1").unwrap().compactions().unwrap();
    assert_eq!(c.untriggered, 1);
    assert!(c.unknown.is_empty(), "absent is not an unknown value");
    assert_eq!(c.total(), 1);
}

// ---------------------------------------------------------------
// #1066: agent types stated rather than inferred.
// ---------------------------------------------------------------

#[test]
fn stated_agent_types_are_counted_by_name() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:00:00+00:00",
                r#""agent_type":"general-purpose""#,
            ),
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:01:00+00:00",
                r#""agent_type":"code-reviewer""#,
            ),
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:02:00+00:00",
                r#""agent_type":"general-purpose""#,
            ),
        ],
    );

    let a = events_for(&conn, "p").unwrap().agent_types(3).unwrap();
    assert_eq!(
        a.stated,
        vec![
            ("general-purpose".to_owned(), 2),
            ("code-reviewer".to_owned(), 1)
        ],
        "commonest first -- this is what turns \"3 subagents\" into \
         \"2 general-purpose, 1 code-reviewer\""
    );
    assert_eq!(a.spawns(), 3);
    assert_eq!(a.disagreement(), None, "both sources found subagents");
}

/// A pre-hook session still groups through the inference.
///
/// #1066 names this test. Every session on the machine today is this
/// case: `claude_subagent` attributed children, and no `SubagentStart`
/// row exists because the hook was not installed when it ran. The
/// rollup must keep working, and it must NOT report a disagreement --
/// flagging the entire corpus as contradictory on day one is how a
/// guard gets turned off.
#[test]
fn a_pre_hook_session_groups_through_the_inference() {
    let conn = db();

    let a = Events::default().agent_types(4).expect(
        "a session with inferred children and no hook rows still has a \
         rollup -- the inference is the fallback, not a casualty",
    );
    assert_eq!(a.inferred_children, 4);
    assert_eq!(a.spawns(), 0, "the hook observed nothing");
    assert!(a.stated.is_empty());
    assert_eq!(
        a.disagreement(),
        None,
        "inference-only is the NORMAL pre-hook state and must not be \
         reported as the two sources contradicting each other"
    );

    assert!(
        Events::default().agent_types(0).is_none(),
        "neither source has anything to say: absent, not an empty rollup"
    );
    drop(conn);
}

/// A hook-stated spawn the inference did not find surfaces the
/// disagreement rather than picking a winner.
///
/// #1066 names this test, and it is the one that carries the issue's
/// central rule: where the two sources disagree, that is a finding --
/// it means the directory layout the inference rests on has moved.
///
/// PROVEN BY SABOTAGE: making `disagreement` return `None`
/// unconditionally fails here; making it also fire in the other
/// direction (`inferred_children > 0 && spawns == 0`) fails
/// `a_pre_hook_session_groups_through_the_inference`, which is the
/// false-positive direction and the reason the check is one-sided.
#[test]
fn a_hook_stated_spawn_the_inference_missed_is_surfaced() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "SubagentStart",
            "p",
            "2026-09-13T10:00:00+00:00",
            r#""agent_type":"Explore""#,
        )],
    );

    // The hook says one subagent started; the cwd rule attributed none.
    let a = events_for(&conn, "p").unwrap().agent_types(0).unwrap();
    let why = a
        .disagreement()
        .expect("the two sources disagree and that must be said out loud");
    assert!(
        why.contains("1 subagent start") && why.contains("working directory"),
        "the sentence must name both what was stated and what was not \
         found: {why}"
    );
    assert_eq!(
        a.stated,
        vec![("Explore".to_owned(), 1)],
        "and the stated type is still reported -- surfacing the \
         disagreement must not cost the answer the hook gave"
    );
}

/// An empty `agent_type` is not a type.
///
/// Measured from the shipped binary: `SubagentStop` writes
/// `agent_type: b ?? ""`. An empty string is a value, and a value would
/// render as a blank chip that looks like a real agent type.
#[test]
fn an_empty_agent_type_is_untyped_not_a_blank_name() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "SubagentStart",
            "p",
            "2026-09-13T10:00:00+00:00",
            r#""agent_type":"""#,
        )],
    );

    let a = events_for(&conn, "p").unwrap().agent_types(1).unwrap();
    assert_eq!(a.untyped, 1);
    assert!(
        a.stated.is_empty(),
        "an empty string must never become a named bucket"
    );
}

// ---------------------------------------------------------------
// #1067: who is waiting on you, and the staleness that makes it hard.
// ---------------------------------------------------------------

#[test]
fn a_live_session_at_an_idle_prompt_is_waiting_now() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"idle_prompt""#,
        )],
    );

    let w = events_for(&conn, "s1").unwrap().waiting(&running());
    assert_eq!(
        w,
        Waiting::Now {
            kind: "idle_prompt".into(),
            at: "2026-09-13T10:00:00+00:00".into(),
        }
    );
}

/// A session whose process is gone NEVER shows present-tense "waiting".
///
/// #1067 names this test, and it is the one the whole type exists for.
/// A stale indicator sends the user to a session that does not need
/// them, and SIGKILL leaves no record to correct it -- so the only
/// defence is refusing the present tense without present-tense
/// evidence.
///
/// PROVEN BY SABOTAGE, and this is the sabotage the brief asked for.
/// Replacing the liveness match in `waiting` with an unconditional
/// `Waiting::Now { .. }` -- which is exactly what "trust the record"
/// looks like as code -- fails here and in
/// `an_unknown_liveness_does_not_earn_the_present_tense`:
///
/// ```text
/// assertion `left == right` failed: a dead session must never claim
///   the present tense
///   left: Now { kind: "idle_prompt", at: "2026-09-13T10:00:00+00:00" }
///  right: LastSeen { .. }
/// ```
///
/// The failure names the dead session claiming to be waiting, which is
/// the defect in the form a user would meet it.
#[test]
fn a_session_whose_process_is_gone_never_shows_present_tense_waiting() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"idle_prompt""#,
        )],
    );

    let w = events_for(&conn, "s1").unwrap().waiting(&dead());
    assert_eq!(
        w,
        Waiting::LastSeen {
            kind: "idle_prompt".into(),
            at: "2026-09-13T10:00:00+00:00".into(),
            why: "its process is no longer running".into(),
        },
        "a dead session must never claim the present tense"
    );
    assert!(
        !matches!(w, Waiting::Now { .. }),
        "stated twice on purpose: this is the assertion the feature rests on"
    );
}

/// "Could not tell" earns no more than "gone" does.
///
/// `Unknown` is not a shade of `Dead`, but it is not a shade of
/// `Running` either, and only `Running` licenses the present tense.
#[test]
fn an_unknown_liveness_does_not_earn_the_present_tense() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"permission_prompt""#,
        )],
    );

    let w = events_for(&conn, "s1").unwrap().waiting(&unknown());
    assert!(
        matches!(w, Waiting::LastSeen { ref why, .. }
                 if why.contains("registry could not be read")),
        "an unconfirmable liveness must degrade to last-seen and carry \
         its own reason, got {w:?}"
    );
}

/// A waiting indicator does not survive a `SessionEnd`.
///
/// #1067 names this test. The `SessionEnd` is in `claude_run`, not in
/// `claude_hook_event`, so this only passes if the expiry clock folds
/// in the run table -- which is what `newest_run_activity` and
/// `observe_activity` are for.
///
/// PROVEN BY SABOTAGE: dropping the `observe_activity` fold from
/// `events_for` (the production path is `sessions::assemble`) fails
/// here with `Now`, because the notification is then the newest thing
/// this module can see. That is the defect in its purest form: a
/// session the user cleanly exited, still listed as waiting for them.
#[test]
fn a_waiting_indicator_does_not_survive_a_session_end() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "SessionStart",
                "s1",
                "2026-09-13T09:00:00+00:00",
                r#""source":"startup""#,
            ),
            line(
                "Notification",
                "s1",
                "2026-09-13T10:00:00+00:00",
                r#""notification_type":"idle_prompt""#,
            ),
            line(
                "SessionEnd",
                "s1",
                "2026-09-13T10:30:00+00:00",
                r#""reason":"prompt_input_exit""#,
            ),
        ],
    );

    // Even with a LIVE process -- the pid could have been reused, and
    // the run record is the stronger evidence either way.
    let w = events_for(&conn, "s1").unwrap().waiting(&running());
    assert_eq!(
        w,
        Waiting::No {
            why: NotWaiting::Superseded
        },
        "a session that ended is not waiting for anyone"
    );
}

/// Any newer record expires the indicator, not just an ending.
///
/// The session was asked for input and then carried on working, which
/// is the common case: the user answered. Nothing records the answer
/// itself, so the evidence is that something happened afterwards.
#[test]
fn any_newer_record_expires_the_indicator() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "Notification",
                "s1",
                "2026-09-13T10:00:00+00:00",
                r#""notification_type":"idle_prompt""#,
            ),
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:20:00+00:00",
                r#""trigger":"auto""#,
            ),
        ],
    );

    assert_eq!(
        events_for(&conn, "s1").unwrap().waiting(&running()),
        Waiting::No {
            why: NotWaiting::Superseded
        },
        "it compacted after asking, so it was answered and moved on"
    );
}

/// An event this app has never heard of still expires the indicator.
///
/// A newer hook writing `StopFailure` or anything else is activity on
/// the session. Ignoring an unrecognised event would let a "waiting"
/// flag outlive evidence that the session moved on, and the safe
/// direction for this flag is always toward clearing it.
#[test]
fn an_unrecognised_newer_event_still_expires_the_indicator() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"idle_prompt""#,
        )],
    );
    // Written straight in: by construction this app's `write_record`
    // does not route an event it does not know to this table, so the
    // row can only come from a NEWER Headstate's hook -- which is the
    // case being tested.
    conn.execute(
        "INSERT INTO claude_hook_event (session_id, event, at) VALUES (?1, ?2, ?3)",
        rusqlite::params!["s1", "SomethingNewer", "2026-09-13T10:05:00+00:00"],
    )
    .unwrap();

    assert_eq!(
        events_for(&conn, "s1").unwrap().waiting(&running()),
        Waiting::No {
            why: NotWaiting::Superseded
        }
    );
}

/// No notification ever seen is NOT a settled "not waiting".
///
/// Absent is not zero, in the place it matters most: on today's corpus
/// every session is this case.
#[test]
fn never_observed_is_not_the_same_as_not_waiting() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "PreCompact",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""trigger":"auto""#,
        )],
    );

    assert_eq!(
        events_for(&conn, "s1").unwrap().waiting(&running()),
        Waiting::No {
            why: NotWaiting::NeverObserved
        },
        "a session with hook rows but no notification has still never \
         been observed waiting"
    );
    assert_eq!(
        Events::default().waiting(&running()),
        Waiting::No {
            why: NotWaiting::NeverObserved
        }
    );
}

/// A notification that is not a request for input does not raise the
/// indicator, and is not mistaken for one.
#[test]
fn a_non_prompt_notification_is_not_waiting() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"auth_success""#,
        )],
    );

    assert_eq!(
        events_for(&conn, "s1").unwrap().waiting(&running()),
        Waiting::No {
            why: NotWaiting::NotAPrompt
        }
    );
}

/// `permission_prompt` is waiting, and is NOT relabelled `idle_prompt`.
///
/// #1067: "a session repeatedly asking permission is a different story
/// from one idling", so the kind travels verbatim and the UI can tell
/// them apart.
#[test]
fn a_permission_prompt_is_waiting_under_its_own_name() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line(
            "Notification",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""notification_type":"permission_prompt""#,
        )],
    );

    assert!(
        matches!(
            events_for(&conn, "s1").unwrap().waiting(&running()),
            Waiting::Now { ref kind, .. } if kind == "permission_prompt"
        ),
        "the kind must survive as itself"
    );
}

// ---------------------------------------------------------------
// Routing: a point event is not a run.
// ---------------------------------------------------------------

/// The three installed events do not create runs.
///
/// A `Notification` stored as a run would be a second run of a session
/// that never restarted, and every liveness read downstream would have
/// an extra open run to reason about.
///
/// PROVEN BY SABOTAGE: removing the `point_event` early return from
/// `handoff::write_record` fails here with three runs where none
/// belong, and the runs carry no `ended_at` -- so they would read as
/// three live processes.
#[test]
fn point_events_do_not_create_runs() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "PreCompact",
                "s1",
                "2026-09-13T10:00:00+00:00",
                r#""trigger":"auto""#,
            ),
            line(
                "SubagentStart",
                "s1",
                "2026-09-13T10:01:00+00:00",
                r#""agent_type":"Explore""#,
            ),
            line(
                "Notification",
                "s1",
                "2026-09-13T10:02:00+00:00",
                r#""notification_type":"idle_prompt""#,
            ),
        ],
    );

    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 0, "a point event is not a run");

    let events: i64 = conn
        .query_row("SELECT COUNT(*) FROM claude_hook_event", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, 3, "but all three were stored");

    // The session row still exists: a point event is evidence the
    // session exists even when no boundary was ever observed.
    let sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 1);
}

/// Re-reading the same records stores them once.
///
/// `handoff::consume` re-reads after a rotation-then-crash, and the
/// `claude_run` inserts are `OR IGNORE` for exactly this reason. The
/// new table needs the same property or a crash would double every
/// compaction count.
#[test]
fn re_reading_the_same_point_events_is_a_no_op() {
    let mut conn = db();
    let lines = [
        line(
            "PreCompact",
            "s1",
            "2026-09-13T10:00:00+00:00",
            r#""trigger":"auto""#,
        ),
        line(
            "PreCompact",
            "s1",
            "2026-09-13T10:01:00+00:00",
            r#""trigger":"auto""#,
        ),
    ];
    feed(&mut conn, &lines);
    feed(&mut conn, &lines);

    let c = events_for(&conn, "s1").unwrap().compactions().unwrap();
    assert_eq!(
        c.auto, 2,
        "two records read twice are still two compactions"
    );
}

/// An unrecognised event keeps its existing behaviour.
///
/// The hook's literal `"unknown"` still records a run, per
/// `handoff::tests::an_unrecognised_event_is_not_an_ending`. The point
/// events divert by NAME, so adding them must not change what happens
/// to an event nobody has heard of.
#[test]
fn an_unrecognised_event_still_records_a_run() {
    let mut conn = db();
    feed(
        &mut conn,
        &[line("unknown", "s1", "2026-09-13T10:00:00+00:00", "")],
    );

    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        runs, 1,
        "diverting the three named events must not change how an \
         unnamed one degrades"
    );
}

/// Counts come out in a stable, total order.
///
/// Two types with the same count would otherwise follow `HashMap`
/// iteration order, which differs between runs -- so a list would
/// reshuffle on a poll where nothing changed.
#[test]
fn equal_counts_order_by_name_so_the_list_does_not_reshuffle() {
    let mut conn = db();
    feed(
        &mut conn,
        &[
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:00:00+00:00",
                r#""agent_type":"zebra""#,
            ),
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:01:00+00:00",
                r#""agent_type":"alpha""#,
            ),
            line(
                "SubagentStart",
                "p",
                "2026-09-13T10:02:00+00:00",
                r#""agent_type":"middle""#,
            ),
        ],
    );

    let a = events_for(&conn, "p").unwrap().agent_types(3).unwrap();
    assert_eq!(
        a.stated,
        vec![
            ("alpha".to_owned(), 1),
            ("middle".to_owned(), 1),
            ("zebra".to_owned(), 1)
        ]
    );
}
