use super::{accounting, discovery, measure, model::*, platform};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};
fn observation() -> Observation {
    serde_json::from_value(serde_json::json!({"id":"one","volume":{"id":"stable","identity_stable":true,"mount":"/fixture","label":"Fixture","scope":"volume","capacity":"214748364800","used":"107374182400","available":"107374182400","method":"fixture","limitation":null},"method":METHOD,"configuration":"same","started_at":100,"finished_at":110,"status":"complete","visited":1,"locations":[{"id":"file-generation-1","identity_stable":true,"path":"/fixture/output","aliases":[],"owners":[],"category":"Build output","evidence":[],"logical":"75161927680","allocated":"75161927680","reclaimable":null,"complete":true,"active":false,"review_only":true}],"categories":[],"coverage":[],"measured":null,"remainder":null,"accounting_note":null,"approximate":true,"comparison":{"baseline_at":null,"reason":"No baseline","changes":[]},"history_error":null})).unwrap()
}
#[test]
fn same_scope_accounting_preserves_difference_and_overallocation() {
    let mut o = observation();
    accounting::summarize(&mut o);
    assert_eq!(o.measured.as_deref(), Some("75161927680"));
    assert_eq!(o.remainder.as_deref(), Some("32212254720"));
    o.volume.used = Some("1".into());
    accounting::summarize(&mut o);
    assert_eq!(o.remainder.as_deref(), Some("-75161927679"));
    assert!(o.accounting_note.unwrap().contains("exceeds"));
}
#[test]
fn unknown_location_does_not_erase_known_measurements_or_become_zero() {
    let mut o = observation();
    let mut unknown = o.locations[0].clone();
    unknown.id = "unknown".into();
    unknown.path = "/fixture/denied".into();
    unknown.logical = None;
    unknown.allocated = None;
    unknown.complete = false;
    o.locations.push(unknown);
    o.status = "partial".into();
    accounting::summarize(&mut o);
    assert_eq!(o.measured.as_deref(), Some("75161927680"));
    assert!(o.locations[1].allocated.is_none());
}
#[test]
fn growth_requires_stable_identical_scope_coverage_and_location_generation() {
    let mut a = observation();
    a.locations[0].allocated = Some((10u64 * 1024 * 1024 * 1024).to_string());
    let mut b = a.clone();
    b.finished_at = 210;
    b.locations[0].allocated = Some((14u64 * 1024 * 1024 * 1024).to_string());
    assert_eq!(
        accounting::compare(&b, None).reason.as_deref(),
        Some("No baseline")
    );
    let change = accounting::compare(&b, Some(&a));
    assert!(change.reason.is_none());
    assert_eq!(change.changes[0].bytes.as_deref(), Some("4294967296"));
    assert_eq!(change.baseline_at, Some(110));
    for reason in 0..7 {
        let mut altered = b.clone();
        match reason {
            0 => altered.configuration = "new roots".into(),
            1 => altered.status = "partial".into(),
            2 => altered.volume.identity_stable = false,
            3 => altered.locations[0].id = "replacement-generation".into(),
            4 => altered.locations[0].path = "/moved".into(),
            5 => altered.method = "changed".into(),
            _ => altered.volume.scope = "another container".into(),
        };
        assert!(
            accounting::compare(&altered, Some(&a)).reason.is_some(),
            "case {reason}"
        );
    }
}
#[test]
fn apfs_adapter_uses_volume_usage_not_shared_container_usage() {
    let mut v = observation().volume;
    v.mount = "/fixture".into();
    v.used = None;
    platform::apply_apfs(
        std::slice::from_mut(&mut v),
        &serde_json::json!({"Containers":[{"CapacityCeiling":1000,"CapacityFree":100,"Volumes":[{"MountPoint":"/fixture","APFSVolumeUUID":"uuid-a","CapacityInUse":200},{"MountPoint":"/other","APFSVolumeUUID":"uuid-b","CapacityInUse":700}]}]}),
    );
    assert_eq!(v.used.as_deref(), Some("200"));
    assert_eq!(v.available.as_deref(), Some("100"));
    assert_eq!(v.id, "apfs:uuid-a");
    assert!(v.identity_stable);
    assert!(v.scope.contains("shared"));
}
#[test]
fn manifestless_and_custom_shared_targets_remain_review_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let a = root.join("a");
    let b = root.join("b");
    let target = root.join("shared-output");
    for repo in [&a, &b] {
        std::fs::create_dir_all(repo.join(".cargo")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(
            repo.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}\n", target.display().to_string()),
        )
        .unwrap();
    }
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::create_dir_all(target.join("debug/.fingerprint")).unwrap();
    std::fs::write(target.join(".rustc_info.json"), "{}").unwrap();
    let settings = Settings {
        external_roots: vec![a.display().to_string(), b.display().to_string()],
        additional_locations: false,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(&[], &settings, &stop, Instant::now());
    let shared: Vec<_> = found
        .locations
        .iter()
        .filter(|l| l.path == target.canonicalize().unwrap().display().to_string())
        .collect();
    assert_eq!(
        shared.len(),
        1,
        "coverage={:?} paths={:?}",
        found.coverage,
        found.locations.iter().map(|l| &l.path).collect::<Vec<_>>()
    );
    assert_eq!(shared[0].owners.len(), 2);
    assert!(shared[0].review_only);
    std::fs::remove_file(a.join("Cargo.toml")).unwrap();
    std::fs::remove_file(b.join("Cargo.toml")).unwrap();
    let again = discovery::discover(&[], &settings, &stop, Instant::now());
    assert!(again
        .locations
        .iter()
        .any(|l| l.path == target.canonicalize().unwrap().display().to_string() && l.review_only));
}
#[cfg(unix)]
#[test]
fn hardlinks_overlaps_and_alias_roots_do_not_inflate_allocation() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("child")).unwrap();
    std::fs::write(root.join("original"), "fixture").unwrap();
    std::fs::hard_link(root.join("original"), root.join("child/linked")).unwrap();
    let alias = dir.path().join("alias");
    symlink(&root, &alias).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let settings = Settings {
        external_roots: vec![
            root.display().to_string(),
            root.join("child").display().to_string(),
            alias.display().to_string(),
        ],
        additional_locations: false,
    };
    let found = discovery::discover(&[], &settings, &stop, Instant::now());
    let facts = platform::file_facts(
        &root.join("original"),
        &std::fs::metadata(root.join("original")).unwrap(),
    )
    .unwrap();
    let mut volume = observation().volume;
    volume.device = facts.volume;
    let out = measure::measure(
        found,
        vec![volume],
        "same".into(),
        "run",
        &stop,
        Instant::now(),
        |_, _| {},
    );
    let logical: u64 = out
        .iter()
        .flat_map(|o| &o.locations)
        .filter_map(|l| l.logical.as_ref())
        .map(|n| n.parse::<u64>().unwrap())
        .sum();
    // Legacy cleanup lists sum logical file lengths for both names. Their
    // totals overlap and must not be presented as allocated/reclaimable bytes.
    assert_eq!(crate::artifacts::measure(&root).0, 14);
    assert_eq!(logical, 7);
    assert_eq!(
        out[0].measured.as_ref().unwrap().parse::<u64>().unwrap(),
        facts.allocated.unwrap()
    );
}
#[test]
fn canceled_scan_preserves_unknown_instead_of_zero() {
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(true));
    let settings = Settings {
        external_roots: vec![dir.path().display().to_string()],
        additional_locations: false,
    };
    let found = discovery::discover(&[], &settings, &stop, Instant::now());
    let out = measure::measure(
        found,
        vec![],
        "same".into(),
        "canceled",
        &stop,
        Instant::now(),
        |_, _| {},
    );
    assert_eq!(out[0].status, "canceled");
    assert!(out[0].locations[0].allocated.is_none());
    assert!(!out[0].coverage.is_empty());
}
#[test]
fn history_retention_is_bounded_and_does_not_replace_complete_baseline_with_partial() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(crate::store::disk_observations::MIGRATION_SQL)
        .unwrap();
    let mut o = observation();
    crate::store::disk_observations::save(&mut conn, &o).unwrap();
    o.id = "partial".into();
    o.status = "partial".into();
    o.finished_at = 120;
    crate::store::disk_observations::save(&mut conn, &o).unwrap();
    assert_eq!(
        crate::store::disk_observations::latest(&conn, "stable")
            .unwrap()
            .unwrap()
            .id,
        "one"
    );
    for n in 0..40 {
        o.id = format!("later{n}");
        o.finished_at = 200 + n;
        crate::store::disk_observations::save(&mut conn, &o).unwrap();
    }
    assert!(
        crate::store::disk_observations::history(&conn)
            .unwrap()
            .len()
            <= 32
    );
    assert!(
        crate::store::disk_observations::latest(&conn, "stable")
            .unwrap()
            .is_some(),
        "partial history must retain last complete baseline"
    );
}

#[test]
fn apfs_real_shape_matches_device_not_label_or_guessed_snapshot_suffix() {
    let mut v = observation().volume;
    v.used = None;
    v.identity_stable = false;
    let data = serde_json::json!({"Containers":[{"CapacityCeiling":1000,"CapacityFree":100,"Volumes":[{"DeviceIdentifier":"disk3s1","APFSVolumeUUID":"stable-a","CapacityInUse":200}]}]});
    platform::apply_apfs_devices(std::slice::from_mut(&mut v), &data, |_| {
        Some("disk3s1s1".into())
    });
    assert!(v.used.is_none());
    assert!(!v.identity_stable);
    platform::apply_apfs_devices(std::slice::from_mut(&mut v), &data, |_| {
        Some("disk3s1".into())
    });
    assert_eq!(v.used.as_deref(), Some("200"));
    assert_eq!(v.id, "apfs:stable-a");
}
#[test]
fn coverage_error_population_is_bounded_and_never_complete() {
    let mut notes = Vec::new();
    for _ in 0..10_000 {
        notes.note(Coverage {
            affects_completeness: true,
            path: "fixture".into(),
            reason: "denied".into(),
        });
    }
    assert_eq!(notes.len(), 1000);
    assert!(notes.last().unwrap().affects_completeness);
    assert!(notes.last().unwrap().reason.contains("limit"));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Explicit read-only host volume adapter probe; no recursive file walk"]
fn host_volume_adapter_matches_reported_apfs_volume_metadata() {
    let stop = Arc::new(AtomicBool::new(false));
    let raw = platform::apfs_metadata(&stop).unwrap();
    let mut volumes = platform::volumes(&stop);
    platform::apply_apfs(&mut volumes, &raw);
    let verified: Vec<_> = volumes
        .iter()
        .filter(|v| v.method == "apfs-volume-capacity-in-use")
        .collect();
    assert!(
        !verified.is_empty(),
        "Host probe requires at least one verified mounted APFS volume"
    );
    for volume in &verified {
        assert!(volume.identity_stable);
        assert!(volume.id.starts_with("apfs:"));
        assert!(volume.used.as_ref().unwrap().parse::<u64>().is_ok());
        assert!(!volume.device.is_empty());
        let expected = raw["Containers"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|container| container["Volumes"].as_array().unwrap())
            .find(|member| member["APFSVolumeUUID"].as_str() == volume.id.strip_prefix("apfs:"))
            .expect("same UUID in actual diskutil metadata");
        assert_eq!(
            volume.used.as_ref().unwrap(),
            &expected["CapacityInUse"].as_u64().unwrap().to_string()
        );
    }
    println!("mounted_scopes={} apfs_volume_uuid_and_usage_verified={} unverified_scopes={}; no recursive scan",volumes.len(),verified.len(),volumes.iter().filter(|v|!v.identity_stable).count());
}

#[test]
fn independent_repository_and_linked_worktree_are_discovered_without_builds() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("independent");
    let linked = dir.path().join("linked");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@users.noreply.github.com",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&["commit", "--allow-empty", "-m", "fixture", "--quiet"]);
    git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![repo.display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let linked = linked.canonicalize().unwrap();
    let row = found
        .locations
        .iter()
        .find(|l| l.path == linked.display().to_string())
        .expect("linked location");
    assert!(row.evidence.iter().any(|e| e.contains("linked worktree")));
    assert!(row.review_only);
}

#[test]
fn cooperative_cancel_keeps_measured_bytes_and_marks_remaining_unknown() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    for i in 0..256 {
        std::fs::write(dir.path().join(format!("file-{i:04}")), [0u8; 32]).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let output = measure::measure(
        found,
        vec![],
        "same".into(),
        "canceled",
        &stop,
        Instant::now(),
        |count, _| {
            if count >= 128 {
                stop.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(output[0].status, "canceled");
    let bytes = output[0].locations[0]
        .logical
        .as_ref()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(bytes > 0 && bytes < 256 * 32);
    assert!(!output[0].locations[0].complete);
    assert!(output[0]
        .coverage
        .iter()
        .any(|c| c.reason.contains("canceled")));
}

#[test]
fn replaced_location_during_walk_cannot_claim_comparable_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![root.display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    std::fs::rename(&root, dir.path().join("old-root")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("new-file"), "new").unwrap();
    let output = measure::measure(
        found,
        vec![],
        "same".into(),
        "replacement",
        &stop,
        Instant::now(),
        |_, _| {},
    );
    assert_eq!(output[0].status, "partial");
    assert!(!output[0].locations[0].identity_stable);
    assert!(output[0]
        .coverage
        .iter()
        .any(|c| c.reason.contains("identity changed")));
}

#[test]
fn failed_history_write_preserves_existing_baseline_and_current_measurement() {
    use crate::store::disk_observations as history;
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(history::MIGRATION_SQL).unwrap();
    let first = observation();
    history::save(&mut conn, &first).unwrap();
    conn.execute_batch("PRAGMA query_only=ON").unwrap();
    let mut current = first.clone();
    current.id = "new".into();
    current.finished_at += 1;
    assert!(history::save(&mut conn, &current).is_err());
    assert_eq!(history::latest(&conn, "stable").unwrap().unwrap().id, "one");
    assert_eq!(current.locations[0].allocated, first.locations[0].allocated);
}

#[test]
fn different_device_child_is_not_charged_to_parent_scope() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("mounted")).unwrap();
    std::fs::write(dir.path().join("mounted/foreign"), [0u8; 4096]).unwrap();
    std::fs::write(dir.path().join("local"), "local").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let output = measure::measure_with_facts(
        found,
        vec![],
        "same".into(),
        "mount",
        &stop,
        Instant::now(),
        (
            |_, _| {},
            |path, metadata| {
                let mut facts = platform::file_facts(path, metadata)?;
                if path.file_name().is_some_and(|name| name == "mounted") {
                    facts.volume = "other-device".into();
                }
                Ok(facts)
            },
        ),
    );
    assert_eq!(output[0].locations[0].logical.as_deref(), Some("5"));
    assert_eq!(output[0].status, "partial");
    assert!(output[0]
        .coverage
        .iter()
        .any(|c| c.reason.contains("Another filesystem")));
}

#[cfg(unix)]
#[test]
fn fixed_metadata_child_does_not_wait_for_descendant_inherited_pipe() {
    use std::ffi::OsStr;
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let bytes = super::process::read(
        "/bin/sh",
        &[OsStr::new("-c"), OsStr::new("sleep 10 & printf fixture")],
        None,
        &stop,
    )
    .unwrap();
    assert_eq!(bytes, b"fixture");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn discovery_rejects_foreign_child_before_probes_but_explicit_root_allows_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let foreign = root.join("foreign");
    std::fs::create_dir_all(foreign.join(".cargo")).unwrap();
    std::fs::write(
        foreign.join(".cargo/config.toml"),
        "[build]\ntarget-dir='output'\n",
    )
    .unwrap();
    std::fs::create_dir(foreign.join("output")).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    for explicit in [false, true] {
        let mut probes = Vec::new();
        let mut roots = vec![root.display().to_string()];
        if explicit {
            roots.push(foreign.display().to_string());
        }
        let found = discovery::discover_with(
            &[],
            &Settings {
                external_roots: roots,
                additional_locations: false,
            },
            &stop,
            Instant::now(),
            (
                |path, _| {
                    Ok(if path.starts_with(&foreign) {
                        "foreign"
                    } else {
                        "parent"
                    }
                    .into())
                },
                |path, kind| probes.push((path.to_path_buf(), kind.to_owned())),
            ),
        );
        if explicit {
            assert!(probes
                .iter()
                .any(|(path, kind)| path == &foreign && kind == "read_dir"));
        } else {
            assert!(
                !probes.iter().any(|(path, _)| path.starts_with(&foreign)),
                "foreign probes={probes:?}"
            );
            assert!(found
                .coverage
                .iter()
                .any(|c| c.reason.contains("filesystem")));
            assert!(!found.roots.contains(&foreign));
        }
    }
}

#[test]
fn partial_history_across_scopes_cannot_evict_complete_baseline() {
    use crate::store::disk_observations as history;
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(history::MIGRATION_SQL).unwrap();
    let first = observation();
    history::save(&mut conn, &first).unwrap();
    for i in 0..256 {
        let mut partial = first.clone();
        partial.id = format!("partial-{i}");
        partial.finished_at = 1000 + i;
        partial.status = "partial".into();
        partial.volume.id = format!("scope-{}", i % 9);
        let _ = history::save(&mut conn, &partial);
    }
    assert!(
        history::latest(&conn, "stable").unwrap().is_some(),
        "global partial pressure removed baseline"
    );
    assert!(history::history(&conn).unwrap().len() <= 256);
}

#[test]
fn partial_history_byte_pressure_preserves_complete_baseline_and_refuses_unsaved_current() {
    use crate::store::disk_observations as history;
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(history::MIGRATION_SQL).unwrap();
    let mut first = observation();
    first.configuration = "x".repeat(7 * 1024 * 1024);
    history::save(&mut conn, &first).unwrap();
    let mut second = first.clone();
    second.id = "other-complete".into();
    second.volume.id = "other".into();
    second.finished_at += 1;
    history::save(&mut conn, &second).unwrap();
    let mut partial = first.clone();
    partial.id = "partial".into();
    partial.finished_at += 2;
    partial.status = "partial".into();
    assert!(
        history::save(&mut conn, &partial).is_err(),
        "partial that cannot fit must explicitly refuse persistence"
    );
    assert!(history::latest(&conn, "stable").unwrap().is_some());
    assert!(history::latest(&conn, "other").unwrap().is_some());
    assert!(partial.locations[0].allocated.is_some());
    let bytes: i64 = conn
        .query_row(
            "SELECT SUM(length(CAST(payload AS BLOB))) FROM disk_observations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= 16 * 1024 * 1024);
}

#[test]
fn linked_worktrees_require_cross_volume_selection_and_enumerate_common_directory_once() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    // Use ordinary paths for Git: Windows canonical paths have a verbatim prefix.
    // Resolve afterward for the production discovery identity comparisons.
    let root = dir.path();
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let linked1 = root.join("foreign-one");
    let linked2 = root.join("foreign-two");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@users.noreply.github.com",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&["commit", "--allow-empty", "-m", "fixture", "--quiet"]);
    for linked in [&linked1, &linked2] {
        git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
    }
    let repo = repo.canonicalize().unwrap();
    let linked1 = linked1.canonicalize().unwrap();
    let linked2 = linked2.canonicalize().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    for explicit in [false, true] {
        let mut roots = vec![repo.display().to_string()];
        if explicit {
            roots.extend([linked1.display().to_string(), linked2.display().to_string()]);
        }
        let mut probes = Vec::new();
        let found = discovery::discover_with(
            &[],
            &Settings {
                external_roots: roots,
                additional_locations: false,
            },
            &stop,
            Instant::now(),
            (
                |path, _| {
                    Ok(
                        if path.starts_with(&linked1) || path.starts_with(&linked2) {
                            "foreign"
                        } else {
                            "parent"
                        }
                        .into(),
                    )
                },
                |path, kind| probes.push((path.to_path_buf(), kind.to_owned())),
            ),
        );
        assert_eq!(
            probes.iter().filter(|(_, kind)| kind == "git").count(),
            1,
            "one actual listing per common directory"
        );
        for linked in [&linked1, &linked2] {
            assert_eq!(found.roots.contains(linked), explicit);
            if !explicit {
                assert!(!probes.iter().any(|(path, _)| path.starts_with(linked)));
            }
        }
        if !explicit {
            assert!(found
                .coverage
                .iter()
                .any(|c| c.reason.contains("filesystem")));
        }
    }
}

#[test]
fn configured_cargo_target_on_another_volume_requires_explicit_selection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let target = root.join("foreign-target");
    std::fs::create_dir_all(repo.join(".cargo")).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(
        repo.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir={:?}\n", target.display().to_string()),
    )
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    for explicit in [false, true] {
        let mut roots = vec![repo.display().to_string()];
        if explicit {
            roots.push(target.display().to_string());
        }
        let found = discovery::discover_with(
            &[],
            &Settings {
                external_roots: roots,
                additional_locations: false,
            },
            &stop,
            Instant::now(),
            (
                |path, _| {
                    Ok(if path.starts_with(&target) {
                        "foreign"
                    } else {
                        "parent"
                    }
                    .into())
                },
                |_, _| {},
            ),
        );
        assert_eq!(found.roots.contains(&target), explicit);
        assert_eq!(
            found
                .locations
                .iter()
                .any(|l| l.path == target.display().to_string()),
            explicit
        );
    }
}

#[test]
fn same_scope_partial_byte_pressure_prunes_partial_before_its_complete_baseline() {
    use crate::store::disk_observations as history;
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(history::MIGRATION_SQL).unwrap();
    let mut first = observation();
    first.configuration = "x".repeat(7 * 1024 * 1024);
    history::save(&mut conn, &first).unwrap();
    for i in 0..2 {
        let mut partial = first.clone();
        partial.id = format!("large-partial-{i}");
        partial.finished_at += i + 1;
        partial.status = "partial".into();
        history::save(&mut conn, &partial).unwrap();
    }
    assert_eq!(history::latest(&conn, "stable").unwrap().unwrap().id, "one");
    let saved = history::history(&conn).unwrap();
    assert_eq!(saved.len(), 2);
    assert!(saved.iter().any(|o| o.id == "large-partial-1"));
    let bytes: i64 = conn
        .query_row(
            "SELECT SUM(length(CAST(payload AS BLOB))) FROM disk_observations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= 16 * 1024 * 1024);
    let mut newer = first.clone();
    newer.id = "new-complete".into();
    newer.volume.id = "another-scope".into();
    newer.finished_at += 3;
    history::save(&mut conn, &newer).unwrap();
    assert!(history::latest(&conn, "stable").unwrap().is_some());
    assert!(history::latest(&conn, "another-scope").unwrap().is_some());
    let mut newest = newer.clone();
    newest.id = "newest-complete".into();
    newest.volume.id = "third-scope".into();
    newest.finished_at += 1;
    history::save(&mut conn, &newest).unwrap();
    assert!(
        history::latest(&conn, "stable").unwrap().is_none(),
        "only competing newer complete baselines retire an old complete baseline"
    );
    assert!(history::latest(&conn, "another-scope").unwrap().is_some());
    assert!(history::latest(&conn, "third-scope").unwrap().is_some());
}

#[test]
fn allocation_floor_survives_unknown_and_overflow_and_history_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["known", "unknown", "overflow"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let output = measure::measure_with_facts(
        found,
        vec![],
        "same".into(),
        "floor",
        &stop,
        Instant::now(),
        (
            |_, _| {},
            |path, metadata| {
                let mut facts = platform::file_facts(path, metadata)?;
                if metadata.is_file() {
                    facts.allocated = match path.file_name().unwrap().to_str().unwrap() {
                        "unknown" => None,
                        "overflow" => Some(u64::MAX),
                        _ => Some(4096),
                    };
                }
                Ok(facts)
            },
        ),
    );
    let row = &output[0].locations[0];
    assert!(row.allocated.is_none());
    assert!(
        row.known_allocated
            .as_deref()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 4096
    );
    assert_eq!(output[0].known_allocated, row.known_allocated);
    assert!(!output[0].allocation_complete);
    let decoded: Observation =
        serde_json::from_str(&serde_json::to_string(&output[0]).unwrap()).unwrap();
    assert_eq!(decoded.locations, output[0].locations);
    assert_eq!(decoded.known_allocated, output[0].known_allocated);
    assert!(!decoded.allocation_complete);
}

#[test]
fn bounded_measurement_resumes_without_duplicate_bytes_and_restarts_on_change() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..30 {
        std::fs::write(dir.path().join(format!("file-{i}")), [1u8; 32]).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let mut scan = measure::Scan::new(found.clone(), vec![], "same".into(), "batches");
    let mut batches = 0;
    while scan.remaining() > 0 {
        scan.batch(
            &stop,
            11,
            std::time::Duration::from_secs(10),
            |_, _| {},
            platform::file_facts,
        );
        batches += 1;
        assert!(batches < 1000);
        if scan.remaining() > 0 {
            scan.resume();
        }
    }
    assert!(batches > 1);
    let result = scan.observations(&stop, platform::file_facts);
    assert_eq!(result[0].locations[0].logical.as_deref(), Some("960"));
    let once = measure::measure(
        found.clone(),
        vec![],
        "same".into(),
        "single",
        &stop,
        Instant::now(),
        |_, _| {},
    );
    assert_eq!(result[0].known_allocated, once[0].known_allocated);
    let mut scan = measure::Scan::new(found, vec![], "same".into(), "changed");
    scan.batch(
        &stop,
        15,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    std::fs::write(dir.path().join("new-file"), [1u8; 40]).unwrap();
    scan.resume();
    scan.batch(
        &stop,
        1000,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    let changed = scan.observations(&stop, platform::file_facts);
    assert_eq!(changed[0].locations[0].logical.as_deref(), Some("1000"));
    assert!(changed[0]
        .coverage
        .iter()
        .any(|n| n.reason.contains("restarted")));
}

#[test]
fn partial_growth_retains_persistent_delta_and_labels_new_and_missing_locations() {
    let before = observation();
    let mut after = before.clone();
    after.status = "partial".into();
    after.locations[0].allocated = Some("75161927690".into());
    let mut added = after.locations[0].clone();
    added.id = "new".into();
    added.path = "/fixture/new".into();
    after.locations.push(added);
    let comparison = accounting::compare(&after, Some(&before));
    assert!(comparison.reason.is_some());
    assert_eq!(
        comparison
            .changes
            .iter()
            .find(|c| c.kind == "persistent")
            .unwrap()
            .bytes
            .as_deref(),
        Some("10")
    );
    assert!(comparison
        .changes
        .iter()
        .find(|c| c.kind == "added")
        .unwrap()
        .bytes
        .is_none());
    after.locations.remove(0);
    let missing = accounting::compare(&after, Some(&before));
    assert!(missing
        .changes
        .iter()
        .any(|c| c.kind == "incomparable" && c.bytes.is_none()));
}

#[cfg(unix)]
#[test]
fn intentional_symlinks_do_not_invalidate_measured_growth() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::os::unix::fs::symlink("missing", dir.path().join("node_modules/link")).unwrap();
    let target = dir.path().join("payload");
    std::fs::write(&target, [1u8; 4096]).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let scan = || {
        let found = discovery::discover(
            &[],
            &Settings {
                external_roots: vec![dir.path().display().to_string()],
                additional_locations: false,
            },
            &stop,
            Instant::now(),
        );
        let facts =
            platform::file_facts(dir.path(), &std::fs::metadata(dir.path()).unwrap()).unwrap();
        let mut volume = observation().volume;
        volume.device = facts.volume;
        measure::measure(
            found,
            vec![volume],
            "same".into(),
            "symlinks",
            &stop,
            Instant::now(),
            |_, _| {},
        )
    };
    let first = scan();
    std::fs::write(&target, [1u8; 32768]).unwrap();
    let next = scan();
    let comparison = accounting::compare(&next[0], Some(&first[0]));
    assert!(
        comparison.changes.iter().any(|c| c
            .bytes
            .as_deref()
            .is_some_and(|s| s.parse::<i128>().unwrap() > 0)),
        "{comparison:?}"
    );
}

#[test]
fn aggregate_overflow_is_a_qualified_floor_and_changed_exclusions_suppress_delta() {
    let mut before = observation();
    before.locations[0].allocated = Some(u64::MAX.to_string());
    let mut extra = before.locations[0].clone();
    extra.id = "extra".into();
    extra.path = "/other".into();
    extra.allocated = Some("1".into());
    before.locations.push(extra);
    accounting::summarize(&mut before);
    assert_eq!(
        before.known_allocated.as_deref(),
        Some(u64::MAX.to_string().as_str())
    );
    assert!(!before.allocation_complete);
    assert!(!before.categories[0].allocation_complete);
    assert!(before.remainder.is_none());
    assert!(before.accounting_note.as_deref().unwrap().contains("range"));
    let mut after = before.clone();
    after.coverage.push(Coverage {
        affects_completeness: false,
        path: "/fixture/output/link".into(),
        reason: "Symbolic link intentionally excluded from allocated totals".into(),
    });
    let changes = accounting::compare(&after, Some(&before));
    assert!(changes
        .changes
        .iter()
        .any(|c| c.location_id == "file-generation-1"
            && c.kind == "incomparable"
            && c.bytes.is_none()));
}

#[test]
fn verified_absence_reports_removed_but_replacement_is_incomparable() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent-target");
    let mut before = observation();
    before.locations[0].path = absent.display().to_string();
    let mut after = before.clone();
    after.locations.clear();
    let removed = accounting::compare(&after, Some(&before));
    assert_eq!(removed.changes[0].kind, "removed");
    assert_eq!(removed.changes[0].bytes.as_deref(), Some("-75161927680"));
    let mut replacement = before.locations[0].clone();
    replacement.id = "replacement".into();
    after.locations.push(replacement);
    let replaced = accounting::compare(&after, Some(&before));
    assert!(replaced.changes.iter().all(|c| c.bytes.is_none()));
}

#[test]
fn many_locations_overlap_and_cancellation_preserve_allocation_across_batches() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..128 {
        let target = dir.path().join(format!("project-{i}")).join("node_modules");
        std::fs::create_dir_all(&target).unwrap();
        for j in 0..16 {
            std::fs::write(target.join(format!("file-{j}")), [0u8; 8]).unwrap();
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![
                dir.path().display().to_string(),
                dir.path()
                    .join("project-0/node_modules")
                    .display()
                    .to_string(),
            ],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    assert!(found.locations.len() >= 129);
    let mut scan = measure::Scan::new(found, vec![], "same".into(), "many");
    scan.batch(
        &stop,
        500,
        std::time::Duration::from_secs(5),
        |_, _| {},
        platform::file_facts,
    );
    let partial = scan.observations(&stop, platform::file_facts);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    scan.batch(
        &stop,
        500,
        std::time::Duration::from_secs(5),
        |_, _| {},
        platform::file_facts,
    );
    assert_eq!(
        scan.observations(&stop, platform::file_facts)[0].known_allocated,
        partial[0].known_allocated
    );
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    for _ in 0..200 {
        if scan.remaining() == 0 {
            break;
        }
        scan.resume();
        scan.batch(
            &stop,
            500,
            std::time::Duration::from_secs(5),
            |_, _| {},
            platform::file_facts,
        );
    }
    assert_eq!(scan.remaining(), 0);
    let result = scan.observations(&stop, platform::file_facts);
    let logical: u64 = result
        .iter()
        .flat_map(|o| &o.locations)
        .filter_map(|l| l.logical.as_deref())
        .map(|s| s.parse::<u64>().unwrap())
        .sum();
    assert_eq!(logical, 128 * 16 * 8);
    assert!(result.iter().flat_map(|o| &o.locations).all(|l| l.complete));
}

#[test]
fn resumed_validation_does_not_consume_the_new_work_quota() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..64 {
        std::fs::write(dir.path().join(format!("file-{i}")), [0u8; 8]).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let mut scan = measure::Scan::new(found, vec![], "same".into(), "nonstarving");
    let mut measurement_batches = 0;
    for _ in 0..200 {
        if scan.remaining() == 0 {
            break;
        }
        let before = scan.visited;
        scan.resume();
        scan.batch(
            &stop,
            8,
            std::time::Duration::from_secs(10),
            |_, _| {},
            platform::file_facts,
        );
        let advanced = scan.visited - before;
        if advanced > 0 {
            measurement_batches += 1;
            assert!(
                scan.remaining() == 0 || advanced >= 3,
                "validation consumed new-work quota: advanced only {advanced}"
            );
        }
    }
    assert_eq!(
        scan.remaining(),
        0,
        "scan larger than the quota must finish"
    );
    assert!(
        measurement_batches <= 18,
        "new traversal must make linear progress"
    );
    assert_eq!(
        scan.observations(&stop, platform::file_facts)[0].locations[0]
            .logical
            .as_deref(),
        Some("512")
    );
}

#[test]
fn completed_validation_survives_cancellation_before_new_traversal() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(dir.path().join(format!("file-{i}")), [0u8; 8]).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let mut scan = measure::Scan::new(found, vec![], "same".into(), "canceled-validation");
    scan.batch(
        &stop,
        8,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    let previously_visited = scan.visited;
    let mut checks = 0;
    scan.resume();
    scan.batch(
        &stop,
        8,
        std::time::Duration::from_secs(10),
        |_, _| {},
        |path, metadata| {
            checks += 1;
            if checks == previously_visited {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            platform::file_facts(path, metadata)
        },
    );
    assert_eq!(scan.visited, previously_visited);
    assert_eq!(
        scan.phase(),
        "measurement",
        "completed validation must remain completed"
    );
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    scan.resume();
    assert_eq!(
        scan.phase(),
        "measurement",
        "Continue must not reset finished validation"
    );
    scan.batch(
        &stop,
        8,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    assert!(scan.visited >= previously_visited + 3);
}

#[test]
fn checkpoint_limit_retains_known_floor_and_ends_continuation_truthfully() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(dir.path().join(format!("file-{i}")), [0u8; 4096]).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let found = discovery::discover(
        &[],
        &Settings {
            external_roots: vec![dir.path().display().to_string()],
            additional_locations: false,
        },
        &stop,
        Instant::now(),
    );
    let mut scan = measure::Scan::new(found, vec![], "same".into(), "memory-limited");
    scan.batch(
        &stop,
        8,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    let before = scan.observations(&stop, platform::file_facts);
    assert!(
        before[0]
            .known_allocated
            .as_deref()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    scan.cap_state_at_current_usage();
    scan.resume();
    scan.batch(
        &stop,
        100,
        std::time::Duration::from_secs(10),
        |_, _| {},
        platform::file_facts,
    );
    let terminal = scan.observations(&stop, platform::file_facts);
    assert_eq!(scan.remaining(), 0);
    assert_eq!(terminal[0].status, "partial");
    assert_eq!(terminal[0].known_allocated, before[0].known_allocated);
    assert!(!terminal[0].allocation_complete);
    assert!(terminal[0].coverage.iter().any(|c| c.affects_completeness
        && c.reason.contains("checkpoint")
        && c.reason.contains("narrower roots")));
}
