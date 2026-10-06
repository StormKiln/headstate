use super::*;
use std::future::Future;
use std::sync::atomic::AtomicU64;
struct TestClock(AtomicU64, AtomicU64);
impl Clock for TestClock {
    fn now(&self) -> (u64, u64) {
        (
            self.0.load(Ordering::Relaxed),
            self.1.load(Ordering::Relaxed),
        )
    }
}
fn fixture(caps: Caps) -> (tempfile::TempDir, Recorder, Arc<TestClock>) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock(AtomicU64::new(1), AtomicU64::new(1000)));
    let recorder = Recorder::with_clock(
        Config {
            directory: dir.path().canonicalize().unwrap().join("journal"),
            epoch: [7; 16],
            role: Role::Desktop,
            platform: Platform::Macos,
            build: "9.2.0-test".into(),
        },
        caps,
        clock.clone(),
    )
    .unwrap();
    (dir, recorder, clock)
}
fn event(owner: &OpaqueId, revision: u64) -> Event {
    Event::QueueReceipt {
        receipt: None,
        owner: owner.clone(),
        list: List::Reviewing,
        operation: None,
        revision,
        receipt_revision: Some(revision),
        phase: Phase::Ready,
        coverage: Coverage::Partial,
        rows: 130,
        receipt_age_ms: None,
        outcome: Acceptance::Accepted,
    }
}
#[test]
fn persisted_opt_in_opens_admission_and_disable_closes_it() {
    let (_dir, r, _) = fixture(Caps::default());
    assert!(!r.enabled());
    assert!(r.intern(Key::Owner("PRIVATE_OWNER")).is_none());
    assert!(r.state.lock().unwrap().keys.is_empty());
    r.set_enabled(true);
    assert!(r.enabled());
    r.set_enabled(false);
    assert!(!r.enabled());
}
#[tokio::test]
async fn private_keys_never_leave_memory_and_disabled_retained_export_is_complete() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let id = r
        .intern(Key::Owner(
            "octocat/hello-world#12345 SYNTHETIC_KEY_SENTINEL",
        ))
        .unwrap();
    assert!(Recorder::record(&r, event(&id, 1)));
    assert!(!Recorder::record(&r, event(&id, 1)));
    r.set_enabled(false);
    let path = dir.path().canonicalize().unwrap().join("report.jsonl");
    let receipt = r.export_to(path.clone()).await.unwrap();
    assert_eq!(receipt.records, 1);
    assert!(!receipt.incomplete);
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("SYNTHETIC_KEY_SENTINEL"));
    assert!(!text.contains("octocat/hello-world#12345"));
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["records"], 1);
    assert_eq!(records[1]["event"]["rows"], 130);
    assert_eq!(records[2]["complete"], true);
    assert_eq!(r.status().durable_records, 1);
    assert_eq!(r.status().loss.coalesced, 1);
}
#[test]
fn capture_parent_retirement_and_capacity_do_not_alias_or_exhaust_operation_lifetime() {
    let (_dir, r, _) = fixture(Caps {
        keys: 2,
        operations: 2,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let operation = r.next_operation(Some(&owner)).unwrap();
    let child = r.next_operation(Some(&operation)).unwrap();
    assert!(r.next_operation(None).is_none());
    r.retire(&owner);
    assert!(r.next_operation(Some(&child)).is_none());
    assert!(!Recorder::record(&r, event(&owner, 1)));
    for _ in 0..5000 {
        let op = r.next_operation(None).unwrap();
        r.retire(&op);
    }
    r.set_enabled(false);
    r.set_enabled(true);
    let new = r.intern(Key::Owner("a")).unwrap();
    assert_ne!(owner, new);
    assert!(!Recorder::record(&r, event(&owner, 2)));
    assert!(Recorder::record(&r, event(&new, 2)));
}
#[test]
fn interner_key_bytes_and_slots_refuse_without_reuse() {
    let (_dir, r, _) = fixture(Caps {
        keys: 1,
        key_bytes: 50,
        ..Caps::default()
    });
    r.set_enabled(true);
    assert!(r.intern(Key::Owner(&"p".repeat(60))).is_none());
    let a = r.intern(Key::Owner("a")).unwrap();
    assert_eq!(r.intern(Key::Owner("a")), Some(a));
    assert!(r.intern(Key::Session("b")).is_none());
    assert_eq!(r.status().loss.cardinality, 2);
}
#[test]
fn quotas_are_independent_clock_regression_is_visible_and_parent_is_validated() {
    let (_dir, r, clock) = fixture(Caps {
        transitions_per_minute: 1,
        failures_per_minute: 1,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let session = r.intern(Key::Session("s")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    assert!(!Recorder::record(&r, event(&owner, 2)));
    assert!(Recorder::record(
        &r,
        Event::StopFailureMatch {
            session,
            observed_boundary: None,
            observation: FailureObservation::IngestedHook,
            delta_ms: None,
            observed_lag_ms: None,
            hook_age_ms: None,
            outcome: MatchOutcome::Unpaired
        }
    ));
    clock.0.store(60001, Ordering::Relaxed);
    clock.1.store(900, Ordering::Relaxed);
    assert!(Recorder::record(&r, event(&owner, 3)));
    assert_eq!(r.status().loss.budget, 1);
    assert_eq!(r.status().loss.clock_anomaly, 1);
}
#[tokio::test]
async fn rotation_restart_and_malformed_tail_are_qualified_without_copying_private_bytes() {
    let (dir, r, _) = fixture(Caps {
        segments: 2,
        segment_bytes: 1024,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    for n in 0..8 {
        assert!(Recorder::record(&r, event(&owner, n)));
    }
    // This fixture checks rotation contents, not storage throughput under the
    // production export deadline. Deadline/cancellation have separate controls.
    assert!(
        flush_fixture(&r, std::time::Duration::from_secs(30)).await,
        "rotation fixture writer did not flush its accepted cutoff within 30s"
    );
    let path = dir.path().canonicalize().unwrap().join("report.jsonl");
    let result = r.export_to(path.clone()).await.unwrap();
    assert!(result.records < 8);
    assert!(result.incomplete);
    assert!(r.status().rotated_out > 0);
    let config = r.config.clone();
    drop(r);
    let journal = &config.directory;
    let last = std::fs::read_dir(journal)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("segment-"))
        .max_by_key(|e| e.file_name())
        .unwrap()
        .path();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(last)
        .unwrap()
        .write_all(b"PRIVATE_BROKEN_TAIL")
        .unwrap();
    let r = Recorder::new(config).unwrap();
    let result = r.export_to(path.clone()).await.unwrap();
    assert!(result.incomplete);
    assert!(!std::fs::read_to_string(path)
        .unwrap()
        .contains("PRIVATE_BROKEN"));
}
#[test]
fn malformed_unknown_fields_and_old_optional_client_counts_remain_distinct() {
    let old =
        r#"{"kind":"stats_view","scope":null,"outcome":"unknown","elapsed_ms":null,"rows":null}"#;
    let parsed: ClientMeasurement = serde_json::from_str(old).unwrap();
    assert!(matches!(
        parsed,
        ClientMeasurement::StatsView { rows: None, .. }
    ));
    assert!(serde_json::from_str::<ClientMeasurement>(
        &old.replace("\"rows\":null", "\"rows\":null,\"private\":\"SECRET\"")
    )
    .is_err());
    assert!(serde_json::from_str::<ClientMeasurement>(
        &old.replace("\"rows\":null", "\"rows\":-1")
    )
    .is_err());
}
#[tokio::test]
async fn destination_refusal_preserves_original_and_writer_remains_usable() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    let destination = dir.path().canonicalize().unwrap().join("folder");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("keep"), "keep").unwrap();
    assert_eq!(
        r.export_to(destination.clone()).await.unwrap_err(),
        ExportError::Destination
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("keep")).unwrap(),
        "keep"
    );
    assert_eq!(
        r.export_to(dir.path().canonicalize().unwrap().join("okay"))
            .await
            .unwrap()
            .records,
        1
    );
}
fn pause(r: &Recorder) -> std::sync::mpsc::Sender<()> {
    let (entered, wait) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    r.control
        .send(writer::Control::Pause {
            entered,
            release: held,
        })
        .unwrap();
    wait.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    release
}
#[tokio::test]
async fn saturated_writer_counts_loss_disable_is_immediate_and_refused_sequence_never_stalls_cutoff(
) {
    let (dir, r, _) = fixture(Caps {
        data: 1,
        control: 1,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let release = pause(&r);
    assert!(Recorder::record(&r, event(&owner, 1)));
    assert!(!Recorder::record(&r, event(&owner, 2)));
    r.set_enabled(false);
    assert!(!r.enabled());
    assert!(!Recorder::record(&r, event(&owner, 3)));
    r.set_enabled(true);
    assert!(!Recorder::record(&r, event(&owner, 4)));
    release.send(()).unwrap();
    let path = dir.path().canonicalize().unwrap().join("report");
    let mut result = r.export_to(path.clone()).await;
    for _ in 0..5 {
        if !matches!(result, Err(ExportError::Busy)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        result = r.export_to(path.clone()).await;
    }
    let result = result.unwrap();
    assert_eq!(result.records, 1);
    assert!(result.incomplete);
    assert_eq!(r.status().durable_seq, 1);
}
#[tokio::test]
async fn export_freezes_cutoff_and_loss_even_when_later_producers_overflow() {
    let (dir, r, _) = fixture(Caps {
        data: 1,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let release = pause(&r);
    assert!(Recorder::record(&r, event(&owner, 1)));
    let path = dir.path().canonicalize().unwrap().join("report");
    let export = r.export_to(path.clone());
    tokio::pin!(export);
    assert!(matches!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(export.as_mut().poll(cx))).await,
        std::task::Poll::Pending
    ));
    assert!(!Recorder::record(&r, event(&owner, 2)));
    release.send(()).unwrap();
    let result = export.await.unwrap();
    assert_eq!(result.records, 1);
    assert!(
        !result.incomplete,
        "post-cutoff loss belongs to a later report"
    );
    assert_eq!(r.status().loss.dropped, 1);
}
#[tokio::test]
async fn writer_failure_is_latched_without_affecting_producer_caller() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let release = pause(&r);
    std::fs::create_dir(r.config.directory.join("segment-0.jsonl")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    release.send(()).unwrap();
    assert_eq!(
        r.export_to(dir.path().canonicalize().unwrap().join("report"))
            .await
            .unwrap_err(),
        ExportError::Unavailable
    );
    assert_eq!(r.status().writer_state, WriterState::Unavailable);
    assert!(!Recorder::record(&r, event(&owner, 2)));
}
#[tokio::test]
async fn stale_parent_and_wrong_kind_client_reference_are_rejected() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let scope = r
        .intern(Key::StatsScope {
            owner: "a",
            scope: "PRIVATE_SCOPE",
            from_day: 1,
            to_day: 7,
        })
        .unwrap();
    let operation = r.next_operation(Some(&owner)).unwrap();
    let receipt = r.receipt_reference(&owner).unwrap();
    assert!(!Recorder::record(
        &r,
        Event::Client {
            observation: ClientMeasurement::StatsView {
                scope: Some(owner.clone()),
                outcome: StatsOutcome::Accepted,
                elapsed_ms: None,
                rows: Some(0)
            }
        }
    ));
    assert!(Recorder::record(
        &r,
        Event::Client {
            observation: ClientMeasurement::StatsView {
                scope: Some(scope.clone()),
                outcome: StatsOutcome::Accepted,
                elapsed_ms: None,
                rows: Some(0)
            }
        }
    ));
    r.retire(&owner);
    assert!(!Recorder::record(
        &r,
        Event::Operation {
            operation_class: None,
            operation,
            parent: Some(owner),
            domain: Domain::Queue,
            stage: Stage::Completed,
            outcome: Outcome::Success,
            elapsed_ms: None,
            affected_fields: None
        }
    ));
    assert!(r.next_operation(Some(&receipt)).is_none());
    assert!(!Recorder::record(
        &r,
        Event::Client {
            observation: ClientMeasurement::StatsView {
                scope: Some(scope),
                outcome: StatsOutcome::Accepted,
                elapsed_ms: None,
                rows: None
            }
        }
    ));
    let saved = r
        .export_to(dir.path().canonicalize().unwrap().join("report"))
        .await
        .unwrap();
    assert_eq!(saved.records, 1);
    assert!(saved.incomplete);
}
#[tokio::test]
async fn routine_budget_retains_deferred_totals_and_separate_failure_budget() {
    let (dir, r, clock) = fixture(Caps {
        routine_per_minute: 1,
        ..Caps::default()
    });
    r.set_enabled(true);
    for metric in [AggregateKind::Read, AggregateKind::CacheReuse] {
        r.aggregate(AggregateDelta {
            domain: Domain::Transcript,
            metric,
            work: WorkClass::Foreground,
            count: 2,
        });
    }
    clock.0.store(60001, Ordering::Relaxed);
    for metric in [AggregateKind::Read, AggregateKind::CacheReuse] {
        r.aggregate(AggregateDelta {
            domain: Domain::Transcript,
            metric,
            work: WorkClass::Foreground,
            count: 3,
        });
    }
    assert_eq!(r.state.lock().unwrap().aggregates.len(), 1);
    clock.0.store(120001, Ordering::Relaxed);
    r.aggregate(AggregateDelta {
        domain: Domain::Transcript,
        metric: AggregateKind::CacheReuse,
        work: WorkClass::Foreground,
        count: 4,
    });
    let path = dir.path().canonicalize().unwrap().join("report");
    assert_eq!(r.export_to(path.clone()).await.unwrap().records, 2);
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("\"count\":9"));
}
#[test]
fn client_batch_cap_counts_refusal_but_opt_out_retains_nothing() {
    let (_dir, r, _) = fixture(Caps::default());
    let event = ClientMeasurement::StatsView {
        scope: None,
        outcome: StatsOutcome::Unknown,
        elapsed_ms: None,
        rows: None,
    };
    assert!(r.client_events(vec![event.clone(); 33]).is_ok());
    assert_eq!(r.status().invalid, 0);
    r.set_enabled(true);
    assert!(r.client_events(vec![event; 33]).is_err());
    assert_eq!(r.status().invalid, 1);
}
#[cfg(unix)]
#[test]
fn journal_and_export_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let (dir, r, _) = fixture(Caps::default());
    let config = r.config.clone();
    drop(r);
    let link = dir.path().canonicalize().unwrap().join("link");
    symlink(&config.directory, &link).unwrap();
    assert!(Recorder::new(Config {
        directory: link,
        ..config
    })
    .is_err());
}
#[tokio::test]
async fn bounded_barrier_timeout_and_busy_never_publish_a_canceled_destination() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let release = pause(&r);
    assert!(Recorder::record(&r, event(&owner, 1)));
    let path = dir.path().canonicalize().unwrap().join("report");
    {
        let first = r.export_to(path.clone());
        tokio::pin!(first);
        assert!(matches!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(first.as_mut().poll(cx))).await,
            std::task::Poll::Pending
        ));
        assert_eq!(
            r.export_to(dir.path().canonicalize().unwrap().join("second"))
                .await
                .unwrap_err(),
            ExportError::Busy
        );
        assert_eq!(first.await.unwrap_err(), ExportError::Timeout);
    }
    assert!(!path.exists());
    release.send(()).unwrap();
    drop(r);
    assert!(!path.exists());
}
#[tokio::test]
async fn lifecycle_and_deferred_counts_survive_disabled_export_without_claiming_exact_events() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    for _ in 0..2000 {
        r.aggregate(AggregateDelta {
            domain: Domain::Queue,
            metric: AggregateKind::NoWork,
            work: WorkClass::Background,
            count: 1,
        });
    }
    assert_eq!(r.state.lock().unwrap().aggregates.len(), 1);
    r.set_enabled(false);
    let path = dir.path().canonicalize().unwrap().join("report");
    assert_eq!(r.export_to(path.clone()).await.unwrap().records, 0);
    let text = std::fs::read_to_string(path).unwrap();
    let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["deferred"][0]["count"], 2000);
    assert_eq!(header["lifecycle"]["captures_closed"], 1);
    assert_eq!(
        header["lifecycle"]["active_capture"],
        serde_json::Value::Null
    );
    assert_eq!(r.measured_count(u64::MAX), None);
}
#[tokio::test]
async fn queued_flush_controls_cannot_write_post_export_cutoff_records_into_snapshot() {
    let (dir, r, _) = fixture(Caps {
        data: 2,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    let release = pause(&r);
    r.control
        .send(writer::Control::Flush { cutoff: 0 })
        .unwrap();
    r.control
        .send(writer::Control::Flush { cutoff: 0 })
        .unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    let path = dir.path().canonicalize().unwrap().join("report");
    let export = r.export_to(path.clone());
    tokio::pin!(export);
    assert!(matches!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(export.as_mut().poll(cx))).await,
        std::task::Poll::Pending
    ));
    assert!(Recorder::record(&r, event(&owner, 2)));
    release.send(()).unwrap();
    assert_eq!(
        export.await.unwrap().records,
        1,
        "later accepted record must remain outside the frozen export"
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("\"revision\":2"));
}
#[tokio::test]
async fn restart_of_unclosed_capture_marks_unknown_tail_instead_of_inventing_zero_loss() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    let path = dir.path().canonicalize().unwrap().join("report");
    assert!(!r.export_to(path.clone()).await.unwrap().incomplete);
    let config = r.config.clone();
    drop(r);
    // An active capture in the durable manifest provides no proof that all
    // admitted records reached disk before process termination.
    let restarted = Recorder::new(config).unwrap();
    assert_eq!(restarted.status().loss.unclean_capture, 1);
    assert!(restarted.export_to(path).await.unwrap().incomplete);
}

async fn closed_two_records() -> (tempfile::TempDir, Config, PathBuf) {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("PRIVATE_FIX_OWNER")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    assert!(Recorder::record(&r, event(&owner, 2)));
    r.set_enabled(false);
    let report = dir.path().canonicalize().unwrap().join("report");
    assert_eq!(r.export_to(report).await.unwrap().records, 2);
    let config = r.config.clone();
    drop(r);
    let segment = writer::segment_path(&config.directory, 0);
    (dir, config, segment)
}
#[tokio::test]
async fn fix1_closed_valid_line_truncation_is_an_explicit_durable_gap() {
    let (dir, config, segment) = closed_two_records().await;
    let bytes = std::fs::read(&segment).unwrap();
    let first = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
    std::fs::write(segment, &bytes[..first]).unwrap();
    let r = Recorder::new(config).unwrap();
    let result = r
        .export_to(dir.path().canonicalize().unwrap().join("after"))
        .await
        .unwrap();
    assert_eq!(result.records, 1);
    assert_eq!(r.status().loss.durable_gap_records, 1);
    assert_eq!(
        r.status().loss.durable_gap_bytes,
        (bytes.len() - first) as u64
    );
    assert_eq!(r.status().loss.durable_gap_segments, 1);
    assert!(
        result.incomplete,
        "a shortened closed journal must not claim complete coverage"
    );
}
#[tokio::test]
async fn fix1_missing_closed_segment_is_an_explicit_durable_gap() {
    let (dir, config, segment) = closed_two_records().await;
    std::fs::remove_file(segment).unwrap();
    let r = Recorder::new(config).unwrap();
    let result = r
        .export_to(dir.path().canonicalize().unwrap().join("after"))
        .await
        .unwrap();
    assert_eq!(result.records, 0);
    assert_eq!(r.status().loss.durable_gap_records, 2);
    assert_eq!(r.status().loss.durable_gap_segments, 1);
    assert!(
        result.incomplete,
        "missing durable rows must not look like a measured zero"
    );
}
#[tokio::test]
async fn fix1_clean_disabled_restart_preserves_deferred_totals_or_durable_omission() {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    assert!(r.intern(Key::Owner("PRIVATE_DEFERRED_OWNER")).is_some());
    for _ in 0..2000 {
        r.aggregate(AggregateDelta {
            domain: Domain::Queue,
            metric: AggregateKind::NoWork,
            work: WorkClass::Background,
            count: 1,
        });
    }
    r.set_enabled(false);
    let path = dir.path().canonicalize().unwrap().join("before");
    r.export_to(path.clone()).await.unwrap();
    let before = std::fs::read_to_string(path).unwrap();
    assert!(before.contains("\"count\":2000"));
    let config = r.config.clone();
    drop(r);
    let r = Recorder::new(config.clone()).unwrap();
    let path = dir.path().canonicalize().unwrap().join("after");
    let result = r.export_to(path.clone()).await.unwrap();
    let after = std::fs::read_to_string(path).unwrap();
    assert!(
        after.contains("\"count\":2000") || result.incomplete,
        "clean disabled restart cannot silently erase deferred totals"
    );
    assert!(!after.contains("PRIVATE"));
    assert_eq!(r.status().loss.deferred_aggregate_gaps, 1);
    assert_eq!(result.records, 0);
    assert!(result.incomplete);
    drop(r);
    for entry in std::fs::read_dir(&config.directory).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE_DEFERRED_OWNER"));
    }
    let r = Recorder::new(config).unwrap();
    r.export_to(dir.path().canonicalize().unwrap().join("again"))
        .await
        .unwrap();
    assert_eq!(r.status().loss.deferred_aggregate_gaps, 1);
}
#[test]
fn fix1_inflight_aggregate_reserves_its_slot_during_refusal_and_merges_concurrent_delta() {
    let (_dir, r, clock) = fixture(Caps {
        aggregates: 1,
        routine_per_minute: 0,
        ..Caps::default()
    });
    r.set_enabled(true);
    let delta = AggregateDelta {
        domain: Domain::Queue,
        metric: AggregateKind::NoWork,
        work: WorkClass::Background,
        count: 2,
    };
    r.aggregate(delta);
    clock.0.store(60001, Ordering::Relaxed);
    let (entered, wait) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let r = &r;
        scope.spawn(move || {
            r.aggregate_with(AggregateDelta { count: 3, ..delta }, || {
                entered.send(()).unwrap();
                held.recv().unwrap();
            })
        });
        wait.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        r.aggregate(AggregateDelta {
            metric: AggregateKind::Candidates,
            count: 1,
            ..delta
        });
        r.aggregate(AggregateDelta { count: 4, ..delta });
        release.send(()).unwrap();
    });
    let state = r.state.lock().unwrap();
    assert_eq!(
        state.aggregates.len(),
        1,
        "an in-flight refusal must not restore beyond the declared cap"
    );
    assert_eq!(
        state.aggregates[&(Domain::Queue, AggregateKind::NoWork, WorkClass::Background)].0,
        9
    );
    assert_eq!(
        state.aggregates[&(Domain::Queue, AggregateKind::NoWork, WorkClass::Background)].1,
        1
    );
}
#[tokio::test]
async fn fix1_extra_flushed_records_beyond_manifest_are_retained_without_false_gap() {
    use std::io::Write;
    let (dir, config, segment) = closed_two_records().await;
    let content = std::fs::read_to_string(&segment).unwrap();
    let mut extra: serde_json::Value =
        serde_json::from_str(content.lines().last().unwrap()).unwrap();
    extra["seq"] = 3.into();
    extra["event"]["revision"] = 3.into();
    extra["event"]["receipt_revision"] = 3.into();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(segment)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&extra).unwrap()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let r = Recorder::new(config).unwrap();
    let result = r
        .export_to(dir.path().canonicalize().unwrap().join("after"))
        .await
        .unwrap();
    assert_eq!(result.records, 3);
    assert!(!result.incomplete);
    assert_eq!(r.status().loss.durable_gap_records, 0);
}
#[tokio::test]
async fn fix1_recorded_rotation_and_repeated_restart_do_not_double_count_gaps() {
    let (dir, r, _) = fixture(Caps {
        segments: 2,
        segment_bytes: 1024,
        ..Caps::default()
    });
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    for n in 0..8 {
        assert!(Recorder::record(&r, event(&owner, n)));
    }
    r.set_enabled(false);
    let path = dir.path().canonicalize().unwrap().join("report");
    r.export_to(path.clone()).await.unwrap();
    let loss = r.status().loss;
    let config = r.config.clone();
    drop(r);
    let r = Recorder::new(config.clone()).unwrap();
    r.export_to(path.clone()).await.unwrap();
    assert_eq!(r.status().loss.durable_gap_segments, 0);
    assert_eq!(r.status().loss.rotated_out, loss.rotated_out);
    drop(r);
    let r = Recorder::new(config).unwrap();
    r.export_to(path).await.unwrap();
    assert_eq!(r.status().loss.durable_gap_segments, 0);
    assert_eq!(r.status().loss.rotated_out, loss.rotated_out);
}
#[tokio::test]
async fn fix1_concurrent_success_keeps_only_new_delta_deferred_and_reuses_completed_slot() {
    let (dir, r, clock) = fixture(Caps {
        aggregates: 1,
        ..Caps::default()
    });
    r.set_enabled(true);
    let delta = AggregateDelta {
        domain: Domain::Queue,
        metric: AggregateKind::Candidates,
        work: WorkClass::Background,
        count: 2,
    };
    r.aggregate(delta);
    clock.0.store(60001, Ordering::Relaxed);
    let (entered, wait) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let r = &r;
        scope.spawn(move || {
            r.aggregate_with(AggregateDelta { count: 3, ..delta }, || {
                entered.send(()).unwrap();
                held.recv().unwrap();
            })
        });
        wait.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        r.aggregate(AggregateDelta { count: 4, ..delta });
        release.send(()).unwrap();
    });
    let path = dir.path().canonicalize().unwrap().join("report");
    assert_eq!(r.export_to(path.clone()).await.unwrap().records, 1);
    let text = std::fs::read_to_string(path).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["deferred"][0]["count"], 4);
    assert_eq!(lines[1]["event"]["count"], 5);
    assert_eq!(r.state.lock().unwrap().aggregates.len(), 1);
    clock.0.store(120001, Ordering::Relaxed);
    r.aggregate(AggregateDelta { count: 0, ..delta });
    assert!(r.state.lock().unwrap().aggregates.is_empty());
    r.aggregate(AggregateDelta {
        metric: AggregateKind::NoWork,
        ..delta
    });
    assert_eq!(r.state.lock().unwrap().aggregates.len(), 1);
}

#[test]
fn queue_receipt_reference_must_belong_to_its_live_owner_and_old_records_remain_readable() {
    let (_dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("owner-A")).unwrap();
    let other = r.intern(Key::Owner("owner-B")).unwrap();
    let receipt = r.receipt_reference(&owner).unwrap();
    let mut observation = event(&other, 1);
    if let Event::QueueReceipt { receipt: value, .. } = &mut observation {
        *value = Some(receipt.clone());
    }
    assert!(!r.record(observation));
    let legacy = serde_json::to_value(event(&owner, 1)).unwrap();
    assert!(legacy.get("receipt").is_none());
    assert!(serde_json::from_value::<Event>(legacy).is_ok());
    assert!(r.is_live_receipt(&receipt));
    r.retire(&owner);
    assert!(!r.is_live_receipt(&receipt));
}

#[test]
fn legacy_five_domain_loss_remains_readable_after_shared_transport_classification() {
    let loss: Loss = serde_json::from_value(serde_json::json!({"by_domain":[1,2,3,4,5]})).unwrap();
    assert_eq!(loss.by_domain, [1, 2, 3, 4, 5, 0]);
    assert!(
        serde_json::from_value::<Loss>(serde_json::json!({"by_domain":[0,0,0,0,0,0,0]})).is_err()
    );
}

async fn completion_allows_immediate_export(destination_refused: bool) {
    let (dir, r, _) = fixture(Caps::default());
    r.set_enabled(true);
    let owner = r.intern(Key::Owner("a")).unwrap();
    assert!(Recorder::record(&r, event(&owner, 1)));
    let root = dir.path().canonicalize().unwrap();
    let destination = root.join("first");
    if destination_refused {
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("keep"), "keep").unwrap();
    }
    let (release, gate) = std::sync::mpsc::channel();
    *r.shared.export_reply_gate.lock().unwrap() = Some(gate);
    let first = r.export_to(destination.clone()).await;
    let second = r.export_to(root.join("second"));
    tokio::pin!(second);
    let polled = std::future::poll_fn(|cx| std::task::Poll::Ready(second.as_mut().poll(cx))).await;
    // Always release before assertions/drop, including the intentional RED path.
    let released = release.send(());
    assert!(
        matches!(polled, std::task::Poll::Pending),
        "completed export retained admission: {polled:?}"
    );
    released.unwrap();
    if destination_refused {
        assert_eq!(first.unwrap_err(), ExportError::Destination);
        assert_eq!(
            std::fs::read_to_string(destination.join("keep")).unwrap(),
            "keep"
        );
    } else {
        assert_eq!(first.unwrap().records, 1);
    }
    assert_eq!(second.await.unwrap().records, 1);
}
#[tokio::test]
async fn ci_export_success_releases_admission_before_reply() {
    completion_allows_immediate_export(false).await;
}
#[tokio::test]
async fn ci_export_refusal_releases_admission_before_reply() {
    completion_allows_immediate_export(true).await;
}

async fn flush_fixture(r: &Recorder, bound: std::time::Duration) -> bool {
    let (reply, receive) = tokio::sync::oneshot::channel();
    let cutoff = r.state.lock().unwrap().seq;
    if r.control
        .try_send(writer::Control::FlushAcknowledged { cutoff, reply })
        .is_err()
    {
        return false;
    }
    matches!(tokio::time::timeout(bound, receive).await, Ok(Ok(true)))
}
#[tokio::test]
async fn ci_stalled_fixture_flush_is_bounded_and_does_not_claim_readiness() {
    let (_dir, r, _) = fixture(Caps::default());
    let release = pause(&r);
    let flushed = flush_fixture(&r, std::time::Duration::from_millis(20)).await;
    release.send(()).unwrap();
    assert!(!flushed);
    assert!(flush_fixture(&r, std::time::Duration::from_secs(2)).await);
}

#[test]
fn operation_affected_fields_preserves_numeric_counts_and_omits_unknown() {
    let (_dir, recorder, _) = fixture(Caps::default());
    recorder.set_enabled(true);
    let operation = recorder.next_operation(None).unwrap();
    let event = Event::Operation {
        operation_class: Some(OperationClass::Detail),
        operation,
        parent: None,
        domain: Domain::Queue,
        stage: Stage::Published,
        outcome: Outcome::Success,
        elapsed_ms: None,
        affected_fields: None,
    };
    let mut wire = serde_json::to_value(&event).unwrap();
    assert!(wire.get("affected_fields").is_none());
    assert!(matches!(
        serde_json::from_value::<Event>(wire.clone()).unwrap(),
        Event::Operation {
            affected_fields: None,
            ..
        }
    ));
    for count in [0, 3, u16::MAX] {
        wire["affected_fields"] = serde_json::json!(count);
        let legacy = serde_json::from_value::<Event>(wire.clone()).unwrap();
        assert!(
            matches!(&legacy, Event::Operation { affected_fields: Some(value), .. } if *value == count)
        );
        assert_eq!(
            serde_json::to_value(legacy).unwrap()["affected_fields"],
            count
        );
    }
    wire["affected_fields"] = serde_json::json!(65536);
    assert!(serde_json::from_value::<Event>(wire).is_err());
}
