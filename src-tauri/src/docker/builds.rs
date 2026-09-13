//! Build history: what was built, how long it took, and from where.
//!
//! Docker Desktop has a Builds page showing durations. What it does not
//! do -- and the reason this earns its place -- is connect a build to the
//! images it produced or the worktree it came from.

use super::cli::docker;
use super::origin::parse_build_inspect;
use serde::Serialize;
use serde_json::Value;

/// One build.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Build {
    /// The opaque ref buildx identifies it by.
    pub reference: String,
    /// The build target, e.g. `octocat-api/docker`.
    pub name: String,
    /// Buildx's own word for the outcome: `Completed`, `Error`, and so on.
    ///
    /// `Option`, not `String` (#963). `.unwrap_or_default()` gave an empty
    /// string for a record missing the field, and `failed()` is
    /// `status != "Completed"` -- so a MISSING status was reported as a
    /// FAILED BUILD. `None` means we do not know what happened, which is
    /// not the same claim.
    pub status: Option<String>,
    /// RFC 3339.
    pub started: String,
    pub duration_secs: f64,
    /// Step counts, or `None` when buildx did not report them.
    ///
    /// `Option`, not `u64` (#963). `.unwrap_or(0)` made a missing field
    /// indistinguishable from a genuine 0% cache hit -- and `cache_percent`
    /// returns 0 for `total_steps == 0`, so an ABSENT field fabricated
    /// exactly the alarm the field exists to raise: this module's own doc
    /// says a sudden return to a cold build "means something invalidated
    /// it".
    pub total_steps: Option<u64>,
    pub cached_steps: Option<u64>,
    /// The build context directory, and the revision it built. Resolved
    /// lazily via `inspect`, which is slower than the listing.
    pub context: Option<String>,
    pub revision: Option<String>,
}

impl Build {
    /// What fraction of steps came from cache, 0-100, or `None` when
    /// there is no fraction to give (#963).
    ///
    /// Shown WITH the duration rather than instead of it: duration alone
    /// says "slow", but duration plus cache ratio says why. Real data
    /// shows the same target going 7m8s -> 1m20s -> 56.9s as the cache
    /// warms, and a sudden return to 7 minutes means something
    /// invalidated it.
    ///
    /// Three cases, and the middle one is the bug that was here: buildx
    /// did not report the counts (`None`), buildx reported zero total
    /// steps (nothing to cache, so there is no ratio), or a real ratio.
    /// `.unwrap_or(0)` upstream collapsed the first into "0% cached",
    /// which is the strongest alarm this number can raise -- a build that
    /// used to be warm going cold -- fabricated from an absent field.
    pub fn cache_percent(&self) -> Option<u64> {
        let (total, cached) = (self.total_steps?, self.cached_steps?);
        if total == 0 {
            // Nothing to cache. Not a 0% hit rate -- there was no rate.
            return None;
        }
        Some(cached * 100 / total)
    }

    /// Whether the build failed. Failures are as interesting as
    /// successes -- arguably more -- so they are never filtered out.
    ///
    /// `None` when there is no status to judge (#963). A missing field
    /// used to read as `""`, which is `!= "Completed"` and therefore a
    /// FAILED build -- inventing the most alarming reading available out
    /// of an absence. Callers get to say "unknown" instead.
    pub fn failed(&self) -> Option<bool> {
        self.status.as_deref().map(|s| s != "Completed")
    }
}

/// Parse `docker buildx history ls --format '{{json .}}'`.
pub fn parse_history(out: &str) -> Vec<Build> {
    let mut all: Vec<Build> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let created = v["created_at"].as_str()?;
            let completed = v["completed_at"].as_str().unwrap_or(created);
            Some(Build {
                reference: v["ref"].as_str()?.to_string(),
                name: v["name"].as_str().unwrap_or_default().to_string(),
                // NOT `.unwrap_or_default()` (#963): an empty string is
                // `!= "Completed"`, so a missing status read as a failed
                // build.
                status: v["status"].as_str().map(str::to_string),
                started: created.to_string(),
                duration_secs: duration_between(created, completed),
                // NOT `.unwrap_or(0)` (#963): a zero total makes
                // `cache_percent` answer 0, so an absent field fabricated
                // a 0%-cached alarm.
                total_steps: v["total_steps"].as_u64(),
                cached_steps: v["cached_steps"].as_u64(),
                context: None,
                revision: None,
            })
        })
        .collect();
    all.sort_by(|a, b| b.started.cmp(&a.started));
    all
}

fn duration_between(start: &str, end: &str) -> f64 {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    match (parse(start), parse(end)) {
        (Some(a), Some(b)) => (b - a).num_milliseconds() as f64 / 1000.0,
        // An unparsable timestamp yields zero rather than a guess: a
        // fabricated duration is worse than an obviously absent one.
        _ => 0.0,
    }
}

/// Fill in the build context and revision for one build.
///
/// Separate from the listing because `inspect` is a subprocess per build
/// -- fetching it for 50 builds up front would make the page slow for
/// data only the selected build needs.
pub fn enrich(build: &mut Build) {
    // `history ls` reports a namespaced ref; `inspect` wants the last
    // segment.
    let id = build
        .reference
        .rsplit('/')
        .next()
        .unwrap_or(&build.reference);
    if let Ok(out) = docker(&["buildx", "history", "inspect", id]) {
        if let Some((context, revision)) = parse_build_inspect(&out) {
            build.context = Some(context);
            build.revision = Some(revision);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real `docker buildx history ls`, with target names
    /// replaced. 50 builds including two failures.
    const HISTORY: &str = include_str!("../../tests/fixtures/buildx_history.jsonl");

    /// Duration alone says "slow". Duration WITH the cache ratio says
    /// why -- and a target that suddenly returns to its cold time means
    /// something invalidated the cache.
    #[test]
    fn cache_percentage_comes_from_the_step_counts() {
        let builds = parse_history(HISTORY);
        let b = builds
            .iter()
            .find(|b| b.total_steps == Some(43))
            .expect("fixture has a 43-step build");
        assert_eq!(b.cached_steps, Some(21));
        assert_eq!(b.cache_percent(), Some(48));
    }

    /// A build with no steps must not divide by zero.
    #[test]
    fn a_build_with_no_steps_reports_no_cache_rather_than_panicking() {
        let b = Build {
            total_steps: Some(0),
            cached_steps: Some(0),
            ..fixture()
        };
        // `None`, not `Some(0)` (#963): zero steps means there was no
        // ratio, which is not the same claim as a 0% hit rate.
        assert_eq!(b.cache_percent(), None);
    }

    /// A `Build` with every field measured, for the cases below to vary
    /// one thing from.
    fn fixture() -> Build {
        Build {
            reference: "r".into(),
            name: "n".into(),
            status: Some("Completed".into()),
            started: String::new(),
            duration_secs: 0.0,
            total_steps: Some(10),
            cached_steps: Some(5),
            context: None,
            revision: None,
        }
    }

    /// #963. `.unwrap_or(0)` on the step counts made a MISSING field
    /// indistinguishable from a genuine 0% cache hit -- and 0% cached is
    /// the strongest alarm this figure can raise, since this module's own
    /// doc says a build returning to its cold time "means something
    /// invalidated it". The alarm was fabricated out of an absence.
    ///
    /// Fed through `parse_history` rather than built as a literal, because
    /// the coercion was in the PARSER: a struct literal would pass whatever
    /// the parser did.
    #[test]
    fn a_build_missing_its_step_counts_does_not_report_zero_percent_cached() {
        let line = r#"{"ref":"abc","name":"api","status":"Completed","created_at":"2026-09-01T10:00:00Z","completed_at":"2026-09-01T10:01:00Z"}"#;
        let builds = parse_history(line);
        assert_eq!(builds.len(), 1, "the record itself is still parsed");
        let b = &builds[0];

        assert_eq!(b.total_steps, None);
        assert_eq!(b.cached_steps, None);
        assert_eq!(
            b.cache_percent(),
            None,
            "an absent step count must not answer 0% cached"
        );
        // The rest of the record survived: a partial answer labelled
        // partial beats dropping the row.
        assert_eq!(b.name, "api");
        assert_eq!(b.failed(), Some(false));
    }

    /// #963. `status` via `.unwrap_or_default()` gave `""`, and `failed()`
    /// is `status != "Completed"` -- so a record missing its status was
    /// reported as a FAILED BUILD. The most alarming available reading,
    /// invented from an absence.
    #[test]
    fn a_build_missing_its_status_is_not_reported_as_failed() {
        let line = r#"{"ref":"abc","name":"api","created_at":"2026-09-01T10:00:00Z","total_steps":10,"cached_steps":5}"#;
        let builds = parse_history(line);
        assert_eq!(builds.len(), 1);
        let b = &builds[0];

        assert_eq!(b.status, None);
        assert_ne!(b.failed(), Some(true), "an absent status is not a failure");
        assert_eq!(b.failed(), None);
        // And a real status is still judged, in both directions.
        assert_eq!(
            Build {
                status: Some("Error".into()),
                ..fixture()
            }
            .failed(),
            Some(true)
        );
        assert_eq!(fixture().failed(), Some(false));
    }

    /// Failed builds are as interesting as successful ones -- arguably
    /// more, since a failing build is what the user is investigating.
    /// The real fixture contains two.
    #[test]
    fn failed_builds_are_kept_not_filtered() {
        let builds = parse_history(HISTORY);
        let failed: Vec<&Build> = builds.iter().filter(|b| b.failed() == Some(true)).collect();
        assert!(
            !failed.is_empty(),
            "the fixture has Error builds; they must survive parsing"
        );
        assert!(builds.len() > failed.len(), "not everything failed");
    }

    /// Durations come from the timestamp pair, and the real data spans
    /// sub-second to multi-minute.
    #[test]
    fn durations_are_computed_from_the_timestamps() {
        let builds = parse_history(HISTORY);
        let longest = builds
            .iter()
            .max_by(|a, b| a.duration_secs.total_cmp(&b.duration_secs))
            .unwrap();
        assert!(
            longest.duration_secs > 60.0,
            "expected a multi-minute build, got {}",
            longest.duration_secs
        );
        assert!(builds.iter().all(|b| b.duration_secs >= 0.0));
    }

    /// Newest first, which is the order the page wants.
    #[test]
    fn builds_are_newest_first() {
        let builds = parse_history(HISTORY);
        for pair in builds.windows(2) {
            assert!(pair[0].started >= pair[1].started);
        }
    }

    /// A malformed line must not hide every other build.
    #[test]
    fn a_broken_row_is_skipped_not_fatal() {
        let mut input = String::from("{ not json\n");
        input.push_str(HISTORY);
        assert_eq!(parse_history(&input).len(), parse_history(HISTORY).len());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// `cargo test --lib live_builds -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_builds_with_context() {
        let out = docker(&["buildx", "history", "ls", "--format", "{{json .}}"]).unwrap();
        let mut builds = parse_history(&out);
        println!("{} builds", builds.len());
        for b in builds.iter_mut().take(4) {
            enrich(b);
            println!(
                "  {:24} {:>7.1}s  {:>3} cached  {}  ctx={:?}",
                b.name,
                b.duration_secs,
                // `Option`, so both are printed as themselves rather than
                // one masquerading as the other (#963).
                b.cache_percent()
                    .map_or_else(|| "  ?".to_string(), |p| format!("{p}%")),
                b.status.as_deref().unwrap_or("(no status)"),
                b.context
                    .as_deref()
                    .map(|c| c.rsplit('/').next().unwrap_or(c))
            );
        }
    }
}
