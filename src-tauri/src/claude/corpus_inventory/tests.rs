use super::*;
fn fixture(files: usize) -> (tempfile::TempDir, PathBuf, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("synthetic");
    std::fs::create_dir_all(&project).unwrap();
    for n in 0..files {
        std::fs::write(project.join(format!("s-{n:04}.jsonl")), "").unwrap();
    }
    let conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
    (dir, root, conn)
}
fn finish(
    conn: &mut Connection,
    root: &Path,
    runtime: &mut DiscoveryRuntime,
    limit: usize,
) -> DiscoverySlice {
    for _ in 0..1000 {
        let slice =
            reconcile_with_limit(conn, root, &PopulationControl::discovery(), runtime, limit)
                .unwrap();
        assert!(slice.work <= limit);
        if slice.authoritative {
            return slice;
        }
        assert!(slice.scan.is_partial());
    }
    panic!("stable synthetic corpus must converge");
}
#[test]
fn bounded_live_iterator_converges_and_matches_full_certificate_without_body_reads() {
    let _clock = super::super::corpus_probe::slice_budget(None); // #1977
    let (_dir, root, mut conn) = fixture(200);
    let mut runtime = DiscoveryRuntime::default();
    #[cfg(feature = "enterprise-harness")]
    crate::enterprise_harness::resources::take_read_capture();
    let first = reconcile_with_limit(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
        64,
    )
    .unwrap();
    assert!(first.scan.is_partial());
    assert!(!first.scan.sessions.is_empty());
    let done = finish(&mut conn, &root, &mut runtime, 64);
    assert_eq!(done.scan.sessions.len(), 200);
    assert!(!done.scan.is_partial());
    for (path, size, certificate) in &done.scan.source_fingerprints {
        assert_eq!(
            super::super::transcript::discovery_fingerprint(path, &PopulationControl::discovery())
                .unwrap(),
            (*size, certificate.clone())
        );
    }
    #[cfg(feature = "enterprise-harness")]
    assert_eq!(
        crate::enterprise_harness::resources::take_read_capture(),
        [0; 4]
    );
}
#[test]
fn reopen_replays_incomplete_directory_but_keeps_completed_units_and_candidates() {
    let _clock = super::super::corpus_probe::slice_budget(None); // #1977
    let (dir, root, mut conn) = fixture(100);
    let mut runtime = DiscoveryRuntime::default();
    let slice = reconcile_with_limit(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
        32,
    )
    .unwrap();
    assert!(!slice.scan.sessions.is_empty());
    assert!(slice.units_completed > 0);
    let paths: Vec<_> = slice.scan.sessions.iter().map(|s| s.path.clone()).collect();
    drop(conn);
    drop(runtime);
    let mut conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
    assert_eq!(
        load_processing_scan(&conn, &root)
            .unwrap()
            .sessions
            .iter()
            .map(|s| s.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
    let root_completed:i64=conn.query_row("SELECT count(*) FROM claude_corpus_directory WHERE depth=0 AND certificate IS NOT NULL",[],|r|r.get(0)).unwrap();
    assert_eq!(root_completed, 1);
    let done = finish(&mut conn, &root, &mut DiscoveryRuntime::default(), 64);
    assert_eq!(done.scan.sessions.len(), 100);
}
#[test]
fn partial_add_rename_delete_retains_accepted_until_authoritative_revalidation() {
    let _clock = super::super::corpus_probe::slice_budget(None); // #1977
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    finish(&mut conn, &root, &mut runtime, 64);
    let project = root.join("synthetic");
    std::fs::remove_file(project.join("s-0000.jsonl")).unwrap();
    std::fs::rename(project.join("s-0001.jsonl"), project.join("renamed.jsonl")).unwrap();
    for i in 0..100 {
        std::fs::write(project.join(format!("new-{i}.jsonl")), "").unwrap();
    }
    let part = reconcile_with_limit(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
        64,
    )
    .unwrap();
    assert!(!part.authoritative);
    assert!(part.scan.sessions.len() > 3);
    assert!(part.scan.sessions.iter().any(|s| s.session_id == "s-0000"));
    assert!(part.scan.sessions.iter().any(|s| s.session_id == "s-0001"));
    let done = finish(&mut conn, &root, &mut runtime, 64);
    assert_eq!(done.scan.sessions.len(), 102);
    assert!(!done
        .scan
        .sessions
        .iter()
        .any(|s| s.session_id == "s-0000" || s.session_id == "s-0001"));
    assert!(done.scan.sessions.iter().any(|s| s.session_id == "renamed"));
}
#[test]
fn equal_timestamp_membership_change_invalidates_epoch() {
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    let control = PopulationControl::discovery();
    // Stop exactly after first enumeration; the second phase must compare names.
    loop {
        reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 1).unwrap();
        let phase: i64 = conn
            .query_row("SELECT phase FROM claude_corpus_epoch", [], |r| r.get(0))
            .unwrap();
        if phase == 1 {
            break;
        }
    }
    let project = root.join("synthetic");
    let before = std::fs::metadata(&project).unwrap().modified().unwrap();
    std::fs::rename(project.join("s-0000.jsonl"), project.join("renamed.jsonl")).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000).access_mode(0x100);
    }
    options
        .open(&project)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before))
        .unwrap();
    let part = reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 64).unwrap();
    assert!(!part.authoritative);
    let accepted: i64 = conn
        .query_row("SELECT accepted FROM claude_corpus_epoch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(accepted, 0);
    let done = finish(&mut conn, &root, &mut runtime, 64);
    assert_eq!(done.scan.sessions.len(), 3);
    assert!(done.scan.sessions.iter().any(|s| s.session_id == "renamed"));
}
#[test]
fn cancellation_and_failed_publication_preserve_accepted_across_reopen() {
    let (dir, root, mut conn) = fixture(10);
    let mut runtime = DiscoveryRuntime::default();
    let initial = finish(&mut conn, &root, &mut runtime, 64);
    std::fs::remove_file(root.join("synthetic/s-0000.jsonl")).unwrap();
    let control = PopulationControl::discovery();
    super::super::corpus_probe::configure(Duration::ZERO, Some((4, control.clone())));
    let cancelled = reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 64).unwrap();
    super::super::corpus_probe::configure(Duration::ZERO, None);
    assert!(!cancelled.authoritative);
    assert_eq!(cancelled.scan.sessions.len(), 10);
    conn.execute_batch("CREATE TRIGGER reject_inventory_publish BEFORE UPDATE OF accepted ON claude_corpus_epoch WHEN NEW.accepted <> OLD.accepted BEGIN SELECT RAISE(ABORT,'synthetic publication failure'); END;").unwrap();
    let mut failed = false;
    for _ in 0..10 {
        if reconcile_corpus(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
        )
        .is_err()
        {
            failed = true;
            break;
        }
    }
    assert!(failed);
    drop(conn);
    let mut conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
    let accepted: i64 = conn
        .query_row("SELECT accepted FROM claude_corpus_epoch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(accepted, initial.epoch);
    assert_eq!(
        load_processing_scan(&conn, &root).unwrap().sessions.len(),
        10
    );
    conn.execute_batch("DROP TRIGGER reject_inventory_publish")
        .unwrap();
    assert_eq!(
        finish(&mut conn, &root, &mut DiscoveryRuntime::default(), 64)
            .scan
            .sessions
            .len(),
        9
    );
}
#[test]
fn generation_change_discards_the_live_iterator_and_cannot_publish_old_epoch() {
    let (_dir, root, mut conn) = fixture(100);
    let mut runtime = DiscoveryRuntime::default();
    let part = reconcile_with_limit(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
        32,
    )
    .unwrap();
    conn.execute(
        "UPDATE claude_corpus_epoch SET staging=staging+1,phase=0",
        [],
    )
    .unwrap();
    let done = finish(&mut conn, &root, &mut runtime, 64);
    assert!(done.epoch > part.epoch);
    assert_eq!(done.scan.sessions.len(), 100);
    let old: i64 = conn
        .query_row(
            "SELECT count(*) FROM claude_corpus_inventory WHERE generation=?1",
            [part.epoch],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
}
#[test]
#[cfg(unix)]
fn permission_failure_is_partial_and_cannot_remove_accepted_membership() {
    crate::test_permissions::skip_unless_chmod_denies!();
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    finish(&mut conn, &root, &mut runtime, 64);
    let project = root.join("synthetic");
    std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = reconcile_corpus(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
    );
    std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.unwrap().scan.is_partial());
    assert_eq!(
        load_processing_scan(&conn, &root).unwrap().sessions.len(),
        3
    );
    assert_eq!(
        finish(&mut conn, &root, &mut runtime, 64)
            .scan
            .sessions
            .len(),
        3
    );
}
#[test]
#[ignore = "synthetic complete-engine measurement; retains fixtures"]
fn discovery_inventory_scaling_measurement() {
    for (files, delay) in [(2048, 0), (2048, 100), (8192, 0)] {
        let (dir, root, mut conn) = fixture(files);
        let mut runtime = DiscoveryRuntime::default();
        let fixture = dir.keep();
        super::super::corpus_probe::configure(Duration::from_micros(delay), None);
        #[cfg(feature = "enterprise-harness")]
        crate::enterprise_harness::resources::take_read_capture();
        let start = std::time::Instant::now();
        let mut passes = 0;
        let mut entries = 0;
        let mut units = 0;
        let mut work = 0;
        let mut peak_owned_payload_bytes = 0;
        loop {
            passes += 1;
            let began = std::time::Instant::now();
            let slice = reconcile_corpus(
                &mut conn,
                &root,
                &PopulationControl::discovery(),
                &mut runtime,
            )
            .unwrap();
            entries += slice.entries;
            units += slice.units_completed;
            work += slice.work;
            peak_owned_payload_bytes = peak_owned_payload_bytes.max(slice.peak_owned_payload_bytes);
            eprintln!("DISCOVERY_SLICE files={files} delay_us={delay} pass={passes} elapsed_us={} entries={} work={} candidate={} authoritative={}",began.elapsed().as_micros(),slice.entries,slice.work,slice.scan.sessions.len(),slice.authoritative);
            if slice.authoritative {
                assert_eq!(slice.scan.sessions.len(), files);
                break;
            }
            assert!(passes < 100);
        }
        #[cfg(feature = "enterprise-harness")]
        assert_eq!(
            crate::enterprise_harness::resources::take_read_capture(),
            [0; 4]
        );
        eprintln!("DISCOVERY_AFTER files={files} delay_us={delay} elapsed_us={} passes={passes} cadence_wait_seconds={} entries={entries} units={units} work={work} owned_payload_peak_bytes={peak_owned_payload_bytes} fixture={}",start.elapsed().as_micros(),(passes-1)*60,fixture.display());
        super::super::corpus_probe::configure(Duration::ZERO, None);
    }
}

#[test]
fn mutation_after_unit_validation_is_rejected_by_final_stamp_sweep() {
    let (_dir, root, mut conn) = fixture(100);
    let mut runtime = DiscoveryRuntime::default();
    finish(&mut conn, &root, &mut runtime, 64);
    let control = PopulationControl::discovery();
    loop {
        reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 1).unwrap();
        let validated:i64=conn.query_row("SELECT count(*) FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.staging WHERE d.depth=0 AND d.validated=1",[],|r|r.get(0)).unwrap();
        if validated == 1 {
            break;
        }
    }
    std::fs::create_dir(root.join("late-project")).unwrap();
    let epoch: i64 = conn
        .query_row("SELECT staging FROM claude_corpus_epoch", [], |r| r.get(0))
        .unwrap();
    for _ in 0..20 {
        let slice = reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 64).unwrap();
        assert!(!slice.authoritative);
        let now: i64 = conn
            .query_row("SELECT staging FROM claude_corpus_epoch", [], |r| r.get(0))
            .unwrap();
        if now != epoch {
            return;
        }
    }
    panic!("late directory mutation was not rejected");
}
#[test]
fn cancel_during_final_sweep_cannot_publish_or_prune() {
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    let accepted = finish(&mut conn, &root, &mut runtime, 64).epoch;
    std::fs::remove_file(root.join("synthetic/s-0000.jsonl")).unwrap();
    let control = PopulationControl::discovery();
    loop {
        reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 1).unwrap();
        let phase: i64 = conn
            .query_row("SELECT phase FROM claude_corpus_epoch", [], |r| r.get(0))
            .unwrap();
        if phase == 2 {
            break;
        }
    }
    control.cancel();
    let slice = reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 64).unwrap();
    assert!(!slice.authoritative);
    assert_eq!(slice.scan.sessions.len(), 3);
    let still: i64 = conn
        .query_row("SELECT accepted FROM claude_corpus_epoch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(still, accepted);
}
#[test]
fn cancellation_immediately_before_inventory_commit_rolls_back_authority() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    let accepted = finish(&mut conn, &root, &mut runtime, 64).epoch;
    std::fs::remove_file(root.join("synthetic/s-0000.jsonl")).unwrap();
    let control = PopulationControl::discovery();
    let cancel = control.clone();
    conn.authorizer(Some(move |ctx: AuthContext<'_>| {
        if matches!(
            ctx.action,
            AuthAction::Update {
                table_name: "claude_corpus_epoch",
                column_name: "accepted"
            }
        ) {
            cancel.cancel();
        }
        Authorization::Allow
    }))
    .unwrap();
    let slice = reconcile_corpus(&mut conn, &root, &control, &mut runtime).unwrap();
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(!slice.authoritative);
    assert_eq!(slice.scan.sessions.len(), 3);
    let still: i64 = conn
        .query_row("SELECT accepted FROM claude_corpus_epoch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(still, accepted);
}
#[test]
fn actual_index_removes_only_after_authoritative_epoch() {
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    let control = PopulationControl::discovery();
    let initial = finish(&mut conn, &root, &mut runtime, 64);
    super::super::search::index_pass_controlled(&mut conn, &initial.scan, &control).unwrap();
    std::fs::remove_file(root.join("synthetic/s-0000.jsonl")).unwrap();
    let partial = reconcile_with_limit(&mut conn, &root, &control, &mut runtime, 8).unwrap();
    assert!(!partial.authoritative);
    super::super::search::index_pass_controlled(&mut conn, &partial.scan, &control).unwrap();
    let remains: i64 = conn
        .query_row(
            "SELECT count(*) FROM claude_index_ledger WHERE session_id='s-0000'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remains, 1);
    let completed = finish(&mut conn, &root, &mut runtime, 64);
    super::super::search::index_pass_controlled(&mut conn, &completed.scan, &control).unwrap();
    let remains: i64 = conn
        .query_row(
            "SELECT count(*) FROM claude_index_ledger WHERE session_id='s-0000'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remains, 0);
}
#[test]
#[cfg(feature = "enterprise-harness")]
fn resource_inventory_mutations_are_classified_and_have_no_body_reads_or_paths() {
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    crate::enterprise_harness::metrics::take_resource_capture();
    crate::enterprise_harness::resources::take_read_capture();
    finish(&mut conn, &root, &mut runtime, 64);
    let rows = crate::enterprise_harness::metrics::take_resource_capture();
    let serialized = serde_json::to_string(&rows).unwrap();
    assert!(!serialized.contains(root.to_str().unwrap()));
    assert!(!serialized.contains("unclassified_table"));
    for table in [
        "claude_corpus_epoch",
        "claude_corpus_inventory",
        "claude_corpus_directory",
    ] {
        assert!(serialized.contains(table));
    }
    assert!(serialized.contains("corpus-discovery"));
    assert_eq!(
        crate::enterprise_harness::resources::take_read_capture(),
        [0; 4]
    );
}

#[test]
fn disappearing_project_restarts_epoch_instead_of_blocking_discovery_forever() {
    let (_dir, root, mut conn) = fixture(3);
    let mut runtime = DiscoveryRuntime::default();
    finish(&mut conn, &root, &mut runtime, 64);
    // Observe the root fully but stop before opening its project.
    loop {
        reconcile_with_limit(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
            1,
        )
        .unwrap();
        let waiting:i64=conn.query_row("SELECT count(*) FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.staging WHERE d.depth=0 AND d.certificate IS NOT NULL",[],|r|r.get(0)).unwrap();
        if waiting == 1 {
            break;
        }
    }
    // Keep synthetic evidence outside the root instead of deleting it.
    std::fs::rename(
        root.join("synthetic"),
        root.parent().unwrap().join("moved-project"),
    )
    .unwrap();
    let next = reconcile_corpus(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
    )
    .unwrap();
    assert!(!next.authoritative);
    assert_eq!(next.scan.sessions.len(), 3);
    assert!(finish(&mut conn, &root, &mut runtime, 64)
        .scan
        .sessions
        .is_empty());
}

#[test]
fn candidates_feed_actual_index_before_full_census() {
    // What one 32-unit slice reaches is a property of the work limit, not of
    // runner speed (#1977). The expired-deadline case is the next test.
    let _clock = super::super::corpus_probe::slice_budget(None);
    let (_dir, root, mut conn) = fixture(200);
    let mut runtime = DiscoveryRuntime::default();
    let slice = reconcile_with_limit(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut runtime,
        32,
    )
    .unwrap();
    assert!(!slice.authoritative);
    assert!(!slice.scan.sessions.is_empty());
    assert!(slice.scan.sessions.len() < 200);
    let result = super::super::search::index_pass_controlled(
        &mut conn,
        &slice.scan,
        &PopulationControl::discovery(),
    )
    .unwrap();
    assert!(result.indexed > 0);
    assert!(!result.population.total_known);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM claude_index_ledger", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, result.indexed as i64);
}
#[test]
fn expired_slice_deadline_still_advances_and_feeds_candidates_before_census() {
    // The slowest possible runner: every slice's deadline has already passed
    // when its loop starts. Each slice must still do one unit of work, or a
    // machine whose setup outlasts the slice never discovers anything (#1977).
    let _clock = super::super::corpus_probe::slice_budget(Some(Duration::ZERO));
    let (_dir, root, mut conn) = fixture(200);
    let mut runtime = DiscoveryRuntime::default();
    let mut slices = 0;
    let candidates = loop {
        let slice = reconcile_with_limit(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
            WORK_PER_SLICE,
        )
        .unwrap();
        slices += 1;
        assert_eq!(
            slice.work, 1,
            "slice {slices} must make exactly one unit of progress"
        );
        assert!(!slice.authoritative);
        if !slice.scan.sessions.is_empty() {
            break slice;
        }
        assert!(slices < 64, "no candidate after {slices} expired slices");
    };
    assert!(candidates.scan.sessions.len() < 200);
    let result = super::super::search::index_pass_controlled(
        &mut conn,
        &candidates.scan,
        &PopulationControl::discovery(),
    )
    .unwrap();
    assert!(result.indexed > 0);
    assert!(!result.population.total_known);
}
#[test]
fn no_directory_handle_outlives_a_slice() {
    // Windows refuses to rename or delete a directory while any handle inside
    // it is open (#1977). A slice that stops mid-directory must not leave its
    // ReadDir in the long-lived runtime, or the user's transcript folder stays
    // locked between population passes. One unit per slice stops mid-directory
    // at every possible point, without depending on the clock.
    let (dir, root, mut conn) = fixture(5);
    let mut runtime = DiscoveryRuntime::default();
    let moved = dir.path().join("moved-projects");
    let (mut renamed_root_listing, mut renamed_project_listing) = (false, false);
    let mut slices = 0;
    let done = loop {
        let slice = reconcile_with_limit(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
            1,
        )
        .unwrap();
        slices += 1;
        #[cfg(target_os = "linux")]
        assert_eq!(
            super::super::corpus_probe::open_handles_under(&root),
            Vec::<PathBuf>::new(),
            "slice {slices} left a handle open inside the corpus root"
        );
        // The portable proof: the rename Windows refused in #1977, taken once
        // while the root is mid-listing and once while its project is.
        let listing = runtime
            .current
            .as_ref()
            .filter(|d| d.count > 0 && d.sealed == 0)
            .map(|d| d.path.clone());
        let take = match &listing {
            Some(path) if path == &root => !std::mem::replace(&mut renamed_root_listing, true),
            Some(_) => !std::mem::replace(&mut renamed_project_listing, true),
            None => false,
        };
        if take {
            std::fs::rename(&root, &moved).unwrap();
            std::fs::rename(&moved, &root).unwrap();
        }
        if slice.authoritative {
            break slice;
        }
        assert!(slices < 1000, "one-unit slices must converge");
    };
    assert!(renamed_root_listing && renamed_project_listing);
    assert_eq!(done.scan.sessions.len(), 5);
    assert!(!done.scan.is_partial());
}
#[test]
fn repeated_mid_directory_restart_is_conservative_then_live_continuation_finishes() {
    let (dir, root, mut conn) = fixture(100);
    for _ in 0..4 {
        let slice = reconcile_with_limit(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut DiscoveryRuntime::default(),
            16,
        )
        .unwrap();
        assert!(!slice.authoritative);
        assert!(slice.scan.is_partial());
        drop(conn);
        conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
        let accepted: i64 = conn
            .query_row("SELECT accepted FROM claude_corpus_epoch", [], |r| r.get(0))
            .unwrap();
        assert_eq!(accepted, 0);
    }
    assert_eq!(
        finish(&mut conn, &root, &mut DiscoveryRuntime::default(), 16)
            .scan
            .sessions
            .len(),
        100
    );
}

#[test]
fn pending_directory_lookup_uses_ordered_indices() {
    let (_dir, _root, conn) = fixture(1);
    for (condition, index) in [
        ("certificate IS NULL", "claude_corpus_directory_pending"),
        ("validated=0", "claude_corpus_directory_validation"),
        ("validated=1", "claude_corpus_directory_validation"),
    ] {
        let sql=format!("EXPLAIN QUERY PLAN SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND {condition} AND issue IS NULL ORDER BY depth,path LIMIT 1");
        let details = conn
            .prepare(&sql)
            .unwrap()
            .query_map(params!["synthetic", 1], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(details.contains(index), "{details}");
        assert!(!details.contains("TEMP B-TREE"), "{details}");
    }
}

#[test]
#[cfg(unix)]
fn unreadable_transcript_does_not_starve_healthy_siblings_or_license_pruning() {
    crate::test_permissions::skip_unless_chmod_denies!();
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root, mut conn) = fixture(3);
    let source = root.join("synthetic/s-0000.jsonl");
    std::fs::write(&source, "unreadable").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = reconcile_corpus(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut DiscoveryRuntime::default(),
    );
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let slice = result.unwrap();
    assert!(slice.scan.is_partial());
    assert_eq!(slice.scan.unreadable_files.len(), 1);
    assert_eq!(slice.scan.sessions.len(), 2);
    assert_eq!(slice.scan.session_bytes, 10);
    assert_eq!(slice.scan.unsized_files, 0);
    let indexed = super::super::search::index_pass_controlled(
        &mut conn,
        &slice.scan,
        &PopulationControl::discovery(),
    )
    .unwrap();
    assert_eq!(indexed.indexed, 2);
    assert_eq!(indexed.scan_unreadable.len(), 1);
    assert_eq!(
        conn.query_row(
            "SELECT incomplete FROM claude_index_state WHERE id=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
#[cfg(unix)]
fn unreadable_projects_do_not_pin_healthy_or_recovered_project_units() {
    crate::test_permissions::skip_unless_chmod_denies!();
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root, mut conn) = fixture(3);
    let blocked = root.join("synthetic");
    let recovered = root.join("t-recover");
    let healthy = root.join("z-healthy");
    for dir in [&recovered, &healthy] {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(dir.join("healthy.jsonl"), "").unwrap();
    }
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(&recovered, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut runtime = DiscoveryRuntime::default();
    let mut progressed = false;
    for _ in 0..5 {
        let slice = reconcile_corpus(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
        )
        .unwrap();
        assert!(!slice.authoritative);
        assert!(slice.scan.is_partial());
        if slice
            .scan
            .sessions
            .iter()
            .any(|s| s.path == healthy.join("healthy.jsonl").to_string_lossy())
        {
            progressed = true;
            break;
        }
    }
    std::fs::set_permissions(&recovered, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut recovered_progress = false;
    for _ in 0..5 {
        let slice = reconcile_corpus(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
        )
        .unwrap();
        assert!(!slice.authoritative);
        if slice
            .scan
            .sessions
            .iter()
            .any(|s| s.path == recovered.join("healthy.jsonl").to_string_lossy())
        {
            recovered_progress = true;
            break;
        }
    }
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(progressed);
    assert!(recovered_progress);
    assert_eq!(
        finish(&mut conn, &root, &mut runtime, 64)
            .scan
            .sessions
            .len(),
        5
    );
}

#[test]
#[cfg(unix)]
fn non_utf8_filename_preserves_identity_and_healthy_progress_across_reopen() {
    use std::os::unix::ffi::OsStringExt;
    let (dir, root, mut conn) = fixture(1);
    let mut runtime = DiscoveryRuntime::default();
    finish(&mut conn, &root, &mut runtime, 64);
    let invalid = root
        .join("synthetic")
        .join(std::ffi::OsString::from_vec(b"\xff.jsonl".to_vec()));
    let _entry_guard = match std::fs::write(&invalid, "bad") {
        Ok(()) => None,
        Err(error) => {
            // APFS rejects these names before enumeration; inject only this
            // synthetic entry, preserving the rest of the real filesystem walk.
            assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
            let placeholder = root.join("synthetic/raw-name-fixture.jsonl");
            std::fs::write(&placeholder, "bad").unwrap();
            Some(super::super::corpus_probe::substitute_entry_path(
                placeholder,
                invalid.clone(),
            ))
        }
    };
    // This valid filename has the same lossy display but must remain distinct.
    std::fs::write(root.join("synthetic/�.jsonl"), "").unwrap();
    std::fs::create_dir(root.join("z-healthy")).unwrap();
    std::fs::write(root.join("z-healthy/later.jsonl"), "").unwrap();
    let mut saw_healthy = false;
    let mut saw_diagnostic = false;
    let mut saw_validated_membership = false;
    for admission in 0..40 {
        if admission == 10 {
            drop(conn);
            conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
            runtime = DiscoveryRuntime::default();
        }
        let slice = reconcile_with_limit(
            &mut conn,
            &root,
            &PopulationControl::discovery(),
            &mut runtime,
            8,
        )
        .unwrap();
        assert!(slice.scan.is_partial());
        assert!(slice.scan.sessions.iter().any(|s| s.session_id == "s-0000"));
        saw_healthy |= slice.scan.sessions.iter().any(|s| s.session_id == "later");
        saw_diagnostic |= !slice.scan.unreadable_files.is_empty();
        saw_validated_membership |= slice.authoritative;
    }
    assert!(saw_healthy);
    assert!(saw_diagnostic);
    assert!(saw_validated_membership);
    let scan = load_processing_scan(&conn, &root).unwrap();
    assert!(scan.sessions.iter().any(|s| s.session_id == "�"));
    assert!(scan.sessions.iter().any(|s| s.session_id == "later"));
    assert_eq!(scan.unreadable_files.len(), 1);
    assert!(scan.is_partial());
    assert_eq!(scan.sessions.len(), 3);
    super::super::search::index_pass_controlled(&mut conn, &scan, &PopulationControl::discovery())
        .unwrap();
    let indexed: i64 = conn
        .query_row("SELECT count(*) FROM claude_index_ledger", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        indexed, 3,
        "healthy sources actually enter the processing lane"
    );
    let certificate: String = conn.query_row("SELECT d.certificate FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.accepted WHERE d.path=?1", [root.join("synthetic").to_str().unwrap()], |r| r.get(0)).unwrap();
    assert!(certificate.contains(&serde_json::to_string(&(b"\xff.jsonl".to_vec(), 2)).unwrap()));
    let raw: Vec<u8> = conn
        .query_row(
            "SELECT path FROM claude_corpus_inventory WHERE typeof(path)='blob' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, invalid.as_os_str().as_encoded_bytes());
}

#[test]
fn unsupported_blob_sizes_follow_durable_directory_scope_across_reopen() {
    let (dir, root, mut conn) = fixture(1);
    finish(&mut conn, &root, &mut DiscoveryRuntime::default(), 64);
    let epoch: i64 = conn
        .query_row(
            "SELECT accepted FROM claude_corpus_epoch WHERE root=?1",
            [root.to_string_lossy()],
            |r| r.get(0),
        )
        .unwrap();
    let project = root.join("synthetic");
    let nested = project.join("subagents");
    conn.execute(
        "INSERT INTO claude_corpus_directory(root,generation,path,depth) VALUES(?1,?2,?3,2)",
        params![root.to_string_lossy(), epoch, nested.to_string_lossy()],
    )
    .unwrap();
    for (raw, directory, size) in [
        (vec![255u8], &project, 23i64),
        (vec![254u8], &nested, 47i64),
    ] {
        conn.execute("INSERT INTO claude_corpus_inventory(root,generation,path,directory,session_id,size_bytes,modified,issue) VALUES(?1,?2,?3,?4,NULL,?5,'recorded','unsupported native path')",params![root.to_string_lossy(),epoch,raw,directory.to_string_lossy(),size]).unwrap();
    }
    drop(conn);
    let conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
    let scan = load_processing_scan(&conn, &root).unwrap();
    assert_eq!(scan.session_bytes, 23);
    assert_eq!(scan.subagent_bytes, 47);
    assert_eq!(scan.subagent_files_skipped, 1);
    assert_eq!(scan.unsized_files, 0);
    assert_eq!(scan.unreadable_files.len(), 2);
    assert_eq!(scan.sessions.len(), 1);
    assert!(scan.is_partial());
    assert_eq!(
        conn.query_row(
            "SELECT accepted FROM claude_corpus_epoch WHERE root=?1",
            [root.to_string_lossy()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        epoch
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM claude_corpus_inventory WHERE typeof(path)='blob'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
#[cfg(unix)]
fn nested_non_utf8_filename_remains_skipped_subagent_across_reopen() {
    use std::os::unix::ffi::OsStringExt;
    let (dir, root, mut conn) = fixture(1);
    let nested = root.join("synthetic/subagents");
    std::fs::create_dir(&nested).unwrap();
    let invalid = nested.join(std::ffi::OsString::from_vec(b"\xff.jsonl".to_vec()));
    let body = b"unsupported nested synthetic transcript";
    let (guard, expected_bytes, expected_unsized) = match std::fs::write(&invalid, body) {
        Ok(()) => (None, body.len() as u64, 0),
        Err(error) => {
            assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
            let placeholder = nested.join("raw-name-fixture.jsonl");
            std::fs::write(&placeholder, body).unwrap();
            (
                Some(super::super::corpus_probe::substitute_entry_path(
                    placeholder,
                    invalid.clone(),
                )),
                0,
                1,
            )
        }
    };
    finish(&mut conn, &root, &mut DiscoveryRuntime::default(), 64);
    drop(guard);
    drop(conn);
    let conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
    let scan = load_processing_scan(&conn, &root).unwrap();
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.session_bytes, 0);
    assert_eq!(scan.subagent_bytes, expected_bytes);
    assert_eq!(scan.subagent_files_skipped, 1);
    assert_eq!(scan.unsized_files, expected_unsized);
    assert_eq!(scan.unreadable_files.len(), 1);
    assert!(scan.is_partial());
    let raw: Vec<u8> = conn
        .query_row(
            "SELECT path FROM claude_corpus_inventory WHERE typeof(path)='blob'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, invalid.as_os_str().as_encoded_bytes());
}

#[test]
#[cfg(unix)]
fn unsupported_raw_directories_do_not_alias_or_starve_healthy_sources() {
    use std::os::unix::ffi::OsStringExt;
    for nested in [false, true] {
        let (dir, root, mut conn) = fixture(1);
        let mut runtime = DiscoveryRuntime::default();
        finish(&mut conn, &root, &mut runtime, 64);
        let parent = if nested {
            root.join("synthetic")
        } else {
            root.clone()
        };
        let invalid = parent.join(std::ffi::OsString::from_vec(vec![255]));
        let (physical, guard) = match std::fs::create_dir(&invalid) {
            Ok(()) => (invalid.clone(), None),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
                let placeholder = parent.join("raw-directory-fixture");
                std::fs::create_dir(&placeholder).unwrap();
                (
                    placeholder.clone(),
                    Some(super::super::corpus_probe::substitute_entry_path(
                        placeholder,
                        invalid.clone(),
                    )),
                )
            }
        };
        std::fs::write(physical.join("hidden.jsonl"), "hidden bytes are unknown").unwrap();
        let replacement = parent.join("�");
        std::fs::create_dir(&replacement).unwrap();
        std::fs::write(replacement.join("valid-replacement.jsonl"), "").unwrap();
        std::fs::create_dir(root.join("z-healthy")).unwrap();
        std::fs::write(root.join("z-healthy/later.jsonl"), "").unwrap();
        let mut saw_healthy = false;
        for tick in 0..24 {
            if tick == 8 {
                drop(conn);
                conn = crate::store::open_db(&dir.path().join("inventory.db")).unwrap();
                runtime = DiscoveryRuntime::default();
            }
            let slice = reconcile_with_limit(
                &mut conn,
                &root,
                &PopulationControl::discovery(),
                &mut runtime,
                16,
            )
            .unwrap();
            assert!(
                slice.scan.sessions.iter().any(|s| s.session_id == "s-0000"),
                "accepted sibling retained"
            );
            if slice.scan.sessions.iter().any(|s| s.session_id == "later") {
                saw_healthy = true;
                assert!(
                    slice.scan.is_partial(),
                    "unsupported subtree stays qualified"
                );
                super::super::search::index_pass_controlled(
                    &mut conn,
                    &slice.scan,
                    &PopulationControl::discovery(),
                )
                .unwrap();
            }
        }
        assert!(
            saw_healthy,
            "healthy new sibling must publish despite raw directory"
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM claude_index_ledger WHERE session_id='later'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "healthy candidate reached actual stored index"
        );
        let scan = load_processing_scan(&conn, &root).unwrap();
        assert!(scan
            .unreadable_dirs
            .iter()
            .any(|d| d.contains("unsupported non-UTF-8 directory")));
        assert!(!scan.sessions.iter().any(|s| s.session_id == "hidden"));
        if !nested {
            assert!(scan
                .sessions
                .iter()
                .any(|s| s.session_id == "valid-replacement"));
        }
        let raw_count: i64 = conn.query_row("SELECT count(*) FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.accepted WHERE typeof(d.path)='blob'", [], |r| r.get(0)).unwrap();
        assert_eq!(raw_count, 1);
        let valid_count: i64 = conn.query_row("SELECT count(*) FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.accepted WHERE d.path=?1", [replacement.to_str().unwrap()], |r| r.get(0)).unwrap();
        assert_eq!(
            valid_count, 1,
            "replacement character directory has independent identity"
        );
        drop(guard);
        std::fs::rename(&physical, dir.path().join("retained-raw-directory")).unwrap();
        let recovered = finish(&mut conn, &root, &mut runtime, 128);
        assert!(!recovered.scan.is_partial());
        assert!(recovered
            .scan
            .sessions
            .iter()
            .any(|s| s.session_id == "later"));
    }
}

#[test]
#[cfg(unix)]
fn unreadable_root_is_partial_and_never_reported_as_absent() {
    crate::test_permissions::skip_unless_chmod_denies!();
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root, mut conn) = fixture(1);
    let permissions = std::fs::metadata(&root).unwrap().permissions();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let probe = std::fs::read_dir(&root);
    let result = reconcile_corpus(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut DiscoveryRuntime::default(),
    );
    std::fs::set_permissions(&root, permissions).unwrap();
    assert_eq!(
        probe.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let scan = result.unwrap().scan;
    assert!(scan.absent_root.is_none());
    assert!(scan.is_partial());
    assert!(!scan.unreadable_dirs.is_empty());
}

#[test]
fn cancelled_missing_root_probe_retains_cancellation_without_absence_authority() {
    let (dir, _, conn) = fixture(0);
    let root = dir.path().join("missing");
    let control = PopulationControl::discovery();
    control.cancel();
    let scan = load_population_scan(&conn, &root, &control).unwrap();
    assert!(scan.absent_root.is_none());
    assert!(scan.is_partial());
    assert_eq!(scan.stop_reason, control.stop_reason());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM claude_corpus_epoch", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn non_directory_root_is_partial_instead_of_absent() {
    let (dir, _, mut conn) = fixture(0);
    let root = dir.path().join("file-not-directory");
    std::fs::write(&root, "synthetic").unwrap();
    let result = reconcile_corpus(
        &mut conn,
        &root,
        &PopulationControl::discovery(),
        &mut DiscoveryRuntime::default(),
    )
    .unwrap();
    assert!(result.scan.absent_root.is_none());
    assert!(result.scan.is_partial());
    assert!(!result.authoritative);
}
