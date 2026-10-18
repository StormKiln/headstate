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
