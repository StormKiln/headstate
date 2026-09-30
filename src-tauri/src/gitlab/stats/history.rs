//! Explicit, incremental backfill. One closed UTC day per user request; no
//! background API spend. Completed days are retained and skipped on resume.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backfill {
    pub source: Source,
    pub viewer: String,
    pub scope: Scope,
    pub requested_days: u32,
    pub complete_days: usize,
    pub attempted_days: usize,
    pub slices: Vec<HistorySlice>,
    pub error: Option<String>,
}

// Return bounded day summaries to the phone; full MR records remain in the
// persisted receipt instead of retransmitting up to 180,000 rows on each click.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySlice {
    pub start: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub counts: Counts,
    pub coverage: Coverage,
    pub merged_window: Option<HistoryMerged>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMerged {
    pub count: usize,
    pub coverage: Coverage,
    pub fetched_at: DateTime<Utc>,
}
impl From<Report> for HistorySlice {
    fn from(report: Report) -> Self {
        Self {
            start: report.start,
            fetched_at: report.fetched_at,
            counts: report.counts,
            coverage: report.coverage,
            merged_window: report.merged_window.map(|m| HistoryMerged {
                count: m.count,
                coverage: m.coverage,
                fetched_at: m.fetched_at,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryReceipt {
    pub attempted_at: DateTime<Utc>,
    pub report: Option<Report>,
    pub error: Option<String>,
}

impl HistoryReceipt {
    /// Keep useful cohorts from a prior receipt. Called under the SQLite write
    /// transaction as well as after a read failure, so stale callers cannot
    /// replace a completed day with their older partial/failed attempt.
    pub fn retain_prior(&mut self, prior: &Self) {
        if prior.attempted_at > self.attempted_at {
            self.attempted_at = prior.attempted_at;
            self.error = prior.error.clone();
        }
        let Some(old) = &prior.report else {
            return;
        };
        let Some(report) = &mut self.report else {
            self.report = Some(old.clone());
            return;
        };
        if old.coverage.complete
            || (!report.coverage.complete && old.counts.created > report.counts.created)
        {
            let merged = report.merged_window.take();
            let error = report.merged_error.take();
            *report = old.clone();
            report.merged_window = merged;
            report.merged_error = error;
        }
        if old.merged_window.as_ref().is_some_and(|m| {
            m.coverage.complete
                || report
                    .merged_window
                    .as_ref()
                    .is_none_or(|fresh| !fresh.coverage.complete && m.count > fresh.count)
        }) {
            report.merged_window = old.merged_window.clone();
        }
    }
}

pub async fn backfill(
    host: &str,
    scope: Scope,
    days: u32,
    db: std::path::PathBuf,
) -> Result<Backfill, String> {
    let source = source(host)?;
    endpoint(&scope)?;
    if !(1..=90).contains(&days) {
        return Err("GitLab history window must be 1 to 90 closed days".into());
    }
    let program =
        super::super::auth::find_glab().ok_or("GitLab CLI (glab) was not found on the desktop")?;
    let viewer = viewer(&program, host).await?;
    backfill_with(
        &program,
        source,
        viewer,
        scope,
        days,
        db,
        Utc::now().date_naive(),
    )
    .await
}

fn complete(r: &Report) -> bool {
    r.coverage.complete
        && r.merged_window
            .as_ref()
            .is_some_and(|m| m.coverage.complete)
}

#[allow(clippy::too_many_arguments)]
async fn backfill_with(
    program: &Path,
    source: Source,
    viewer: String,
    scope: Scope,
    days: u32,
    db: std::path::PathBuf,
    today: chrono::NaiveDate,
) -> Result<Backfill, String> {
    let partition = serde_json::to_string(&("gitlab", &source.host, &viewer, &scope))
        .map_err(|e| e.to_string())?;
    let mut result = Backfill {
        source: source.clone(),
        viewer: viewer.clone(),
        scope: scope.clone(),
        requested_days: days,
        complete_days: 0,
        attempted_days: 0,
        slices: vec![],
        error: None,
    };

    let mut slots = Vec::new();
    for offset in 1..=days {
        let day = today - chrono::Duration::days(i64::from(offset));
        let path = db.clone();
        let key = partition.clone();
        let date = day.to_string();
        let cached = tokio::task::spawn_blocking(move || {
            crate::store::gitlab_stats::history_get(&path, &key, &date)
        })
        .await
        .map_err(|_| "Could not read GitLab history".to_string())??;
        slots.push((day, cached));
    }
    // Record unsuccessful attempts too: a denied or dense day must not starve
    // untouched dates. After visiting them, retry oldest incomplete evidence.
    let next = slots
        .iter()
        .position(|(_, value)| value.is_none())
        .or_else(|| {
            slots
                .iter()
                .enumerate()
                .filter_map(|(index, (_, value))| {
                    value
                        .as_ref()
                        .filter(|r| !r.report.as_ref().is_some_and(complete))
                        .map(|r| (index, r.attempted_at))
                })
                .min_by_key(|(_, at)| *at)
                .map(|(index, _)| index)
        });
    if let Some(index) = next {
        let (day, prior) = &mut slots[index];
        let start = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = (day
            .succ_opt()
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc())
            - chrono::Duration::nanoseconds(1);
        let mut receipt = HistoryReceipt {
            attempted_at: Utc::now(),
            report: prior.as_ref().and_then(|r| r.report.clone()),
            error: None,
        };
        match load_window(program, source.clone(), viewer.clone(), scope, start, end).await {
            Ok(report) => {
                receipt.report = Some(report);
            }
            Err(error) => {
                receipt.error = Some(error);
            }
        }
        if super::viewer(program, &source.host).await? != viewer {
            return Err(
                "GitLab account changed while loading history. Refresh to try again.".into(),
            );
        }
        if let Some(old) = prior {
            receipt.retain_prior(old);
        }
        result.error = receipt.error.clone();
        let path = db.clone();
        let key = partition.clone();
        let date = day.to_string();
        let saved = receipt.clone();
        let saved_result = tokio::task::spawn_blocking(move || {
            crate::store::gitlab_stats::history_put(&path, &key, &date, &saved)
        })
        .await;
        match saved_result {
            Ok(Ok(saved)) => {
                receipt = saved;
                result.error = receipt.error.clone();
            }
            _ => {
                result.error = Some(
                    "Retrieved history could not be saved; retry may request this day again".into(),
                )
            }
        }
        *prior = Some(receipt);
    }
    for (_, receipt) in slots {
        if let Some(receipt) = receipt {
            result.attempted_days += 1;
            if let Some(slice) = receipt.report {
                if complete(&slice) {
                    result.complete_days += 1;
                }
                result.slices.push(slice.into());
            }
        }
    }
    Ok(result)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn program(dir: &Path, body: &str) -> std::path::PathBuf {
        let program = dir.join("glab");
        std::fs::write(&program, format!("#!/bin/sh\nif [ \"$5\" = user ]; then printf 'HTTP/2 200\\n\\n{{\"id\":1}}'; exit 0; fi\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        program
    }
    #[tokio::test]
    async fn resume_skips_complete_days_and_partitions_accounts_hosts_and_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.sqlite");
        let program = program(
            dir.path(),
            "printf 'HTTP/2 200\\nx-next-page: \\nx-total: 0\\n\\n[]'",
        );
        let source = source("gitlab.com").unwrap();
        let today = "2026-09-04".parse().unwrap();
        let first = backfill_with(
            &program,
            source.clone(),
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(first.complete_days, 1);
        assert_eq!(first.slices.len(), 1);
        let second = backfill_with(
            &program,
            source.clone(),
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(second.complete_days, 2);
        // A complete ledger does not invoke the program again.
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        let third = backfill_with(
            &program,
            source,
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(third.complete_days, 2);
        assert!(third.error.is_none());
        for key in [
            r#"["gitlab","gitlab.com","2",{"kind":"mine"}]"#,
            r#"["gitlab","other.example","1",{"kind":"mine"}]"#,
            r#"["github","github.com","1",{"kind":"mine"}]"#,
            r#"["gitlab","gitlab.com","1",{"kind":"group","path":"g"}]"#,
        ] {
            assert!(
                crate::store::gitlab_stats::history_get(&db, key, "2026-09-03")
                    .unwrap()
                    .is_none()
            );
        }
    }
    #[tokio::test]
    async fn failed_day_is_recorded_without_zero_and_does_not_starve_older_dates() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.sqlite");
        let program = program(dir.path(), "exit 1");
        let source = source("gitlab.com").unwrap();
        let today = "2026-09-04".parse().unwrap();
        let first = backfill_with(
            &program,
            source.clone(),
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(first.attempted_days, 1);
        assert!(first.slices.is_empty());
        assert!(first.error.is_some());
        let second = backfill_with(
            &program,
            source,
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(second.attempted_days, 2);
        assert!(second.slices.is_empty());
    }
    #[tokio::test]
    async fn capped_day_does_not_starve_untouched_older_day() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.sqlite");
        let program = program(dir.path(), "printf 'HTTP/2 200\\n\\n[]'");
        let source = source("gitlab.com").unwrap();
        let today = "2026-09-04".parse().unwrap();
        let first = backfill_with(
            &program,
            source.clone(),
            "1".into(),
            Scope::Mine,
            2,
            db.clone(),
            today,
        )
        .await
        .unwrap();
        assert_eq!(first.complete_days, 0);
        assert_eq!(first.attempted_days, 1);
        let second = backfill_with(&program, source, "1".into(), Scope::Mine, 2, db, today)
            .await
            .unwrap();
        assert_eq!(second.complete_days, 0);
        assert_eq!(second.attempted_days, 2);
        assert!(second.slices.iter().all(|r| !r.coverage.complete));
    }
    #[tokio::test]
    async fn account_change_discards_history_before_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.sqlite");
        let cli = program(dir.path(), "printf 'HTTP/2 200\\nx-next-page: \\n\\n[]'");
        let script = std::fs::read_to_string(&cli)
            .unwrap()
            .replace("\"id\":1", "\"id\":2");
        std::fs::write(&cli, script).unwrap();
        let result = backfill_with(
            &cli,
            source("gitlab.com").unwrap(),
            "1".into(),
            Scope::Mine,
            1,
            db.clone(),
            "2026-09-04".parse().unwrap(),
        )
        .await;
        assert!(result.unwrap_err().contains("account changed"));
        assert!(crate::store::gitlab_stats::history_get(
            &db,
            r#"["gitlab","gitlab.com","1",{"kind":"mine"}]"#,
            "2026-09-03"
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn slower_concurrent_failure_cannot_erase_completed_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.sqlite");
        let partition = "partition";
        let day = "2026-09-01";
        // Both callers initially observe no receipt; hold the failing writer
        // until the successful writer commits, reproducing the lost update.
        assert!(crate::store::gitlab_stats::history_get(&db, partition, day)
            .unwrap()
            .is_none());
        let failed = HistoryReceipt {
            attempted_at: Utc::now(),
            report: None,
            error: Some("request failed".into()),
        };
        let path = db.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let slow = std::thread::spawn(move || {
            wait.recv().unwrap();
            crate::store::gitlab_stats::history_put(&path, partition, day, &failed).unwrap()
        });
        let mut report = super::super::tests::report(vec![super::super::tests::sample(1)]);
        report.merged_window = Some(summarize_merged(
            &report,
            vec![super::super::tests::sample(1)],
            super::super::tests::coverage(),
        ));
        let success = HistoryReceipt {
            attempted_at: Utc::now(),
            report: Some(report),
            error: None,
        };
        crate::store::gitlab_stats::history_put(&db, partition, day, &success).unwrap();
        release.send(()).unwrap();
        let returned = slow.join().unwrap();
        assert!(complete(returned.report.as_ref().unwrap()));
        assert!(returned.error.is_none());
        let saved = crate::store::gitlab_stats::history_get(&db, partition, day)
            .unwrap()
            .unwrap();
        assert!(complete(saved.report.as_ref().unwrap()));
        assert_eq!(saved.report.unwrap().counts.created, 1);
    }
}
