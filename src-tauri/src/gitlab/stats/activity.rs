//! Comment participation is measured from notes, never inferred from reviewer
//! assignments or system-note prose. GitLab has no universal review verdict.
use super::*;
const MAX_MRS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub complete: bool,
    pub mrs_checked: usize,
    pub mrs_total: usize,
    pub comments: Option<usize>,
    pub participants: Vec<Participant>,
    pub mean_first_response_hours: Option<f64>,
    pub responded_mrs: usize,
    pub failures: Vec<String>,
    #[serde(default)]
    pub rate_limited: bool,
    pub rate_remaining: Option<u64>,
    pub rate_reset: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub username: String,
    pub comments: usize,
    pub mrs: usize,
}

pub(super) async fn load(program: &Path, report: &Report, budget: Duration) -> Activity {
    let started = tokio::time::Instant::now();
    let mut result = Activity {
        complete: report.coverage.complete,
        mrs_checked: 0,
        mrs_total: report.history.len(),
        comments: None,
        participants: vec![],
        mean_first_response_hours: None,
        responded_mrs: 0,
        failures: vec![],
        rate_limited: false,
        rate_remaining: None,
        rate_reset: None,
    };
    let mut people = BTreeMap::<String, (usize, usize)>::new();
    let mut times = Vec::new();
    for mr in report.history.iter().take(MAX_MRS) {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            result
                .failures
                .push("Load budget exhausted before requesting more comments".into());
            break;
        }
        // One 100-note page per MR; pagination evidence qualifies larger threads.
        let endpoint = format!(
            "projects/{}/merge_requests/{}/notes?sort=asc&order_by=created_at",
            encode(&mr.project),
            mr.iid
        );
        let (notes, coverage) =
            match pages_limited(program, &report.source.host, &endpoint, remaining, 1).await {
                Ok(value) => value,
                Err(failure) => {
                    result.failures.push(error(&failure.stop));
                    result.complete = false;
                    if failure.stop == Stop::RateLimited {
                        result.rate_limited = true;
                        result.rate_remaining = failure.remaining;
                        result.rate_reset = failure.reset;
                        break;
                    }
                    continue;
                }
            };
        result.rate_remaining = coverage.rate_remaining;
        result.rate_reset = coverage.rate_reset;
        let (valid, first) = absorb(
            &notes,
            mr,
            report.end,
            &mut people,
            result.comments.get_or_insert(0),
        );
        if valid && coverage.complete {
            result.mrs_checked += 1;
        } else {
            result.complete = false;
        }
        if let Some(first) = first {
            times.push((first - mr.created_at).num_seconds() as f64 / 3600.0);
        }
        if coverage.rate_limited() {
            result.rate_limited = true;
            result
                .failures
                .push("GitLab rate limit reached while reading comments".into());
            break;
        }
    }
    result.complete &= result.mrs_checked == result.mrs_total;
    if result.complete && result.mrs_total == 0 {
        result.comments = Some(0);
    }
    result.responded_mrs = times.len();
    result.mean_first_response_hours = (result.complete && !times.is_empty())
        .then(|| times.iter().sum::<f64>() / times.len() as f64);
    result.participants = people
        .into_iter()
        .map(|(username, (comments, mrs))| Participant {
            username,
            comments,
            mrs,
        })
        .collect();
    result
}

fn absorb(
    notes: &[Value],
    mr: &Record,
    end: DateTime<Utc>,
    people: &mut BTreeMap<String, (usize, usize)>,
    comments: &mut usize,
) -> (bool, Option<DateTime<Utc>>) {
    let mut valid = true;
    let mut seen = HashSet::new();
    let mut participants = HashSet::new();
    let mut first = None::<DateTime<Utc>>;
    for note in notes {
        let parsed = (|| {
            Some((
                note.get("id")?.as_u64()?,
                note.get("system")?.as_bool()?,
                note.pointer("/author/username")?.as_str()?,
                timestamp(note, "created_at")?,
            ))
        })();
        let Some((id, system, username, at)) = parsed else {
            valid = false;
            continue;
        };
        if !seen.insert(id) || at < mr.created_at {
            valid = false;
            continue;
        }
        if system || username == mr.author || at > end {
            continue;
        }
        *comments += 1;
        people.entry(username.into()).or_default().0 += 1;
        participants.insert(username.to_owned());
        first = Some(first.map_or(at, |prior| prior.min(at)));
    }
    for username in participants {
        people.entry(username).or_default().1 += 1;
    }
    (valid, first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_non_author_non_system_comments_count_and_missing_fields_qualify() {
        let mr = record(
            &super::super::tests::sample(1),
            &source("gitlab.com").unwrap(),
        )
        .unwrap();
        let note = |id, system, username| json!({"id":id,"system":system,"author":{"username":username},"created_at":"2026-09-01T12:00:00Z"});
        let mut people = BTreeMap::new();
        let mut count = 0;
        let (valid, first) = absorb(
            &[
                note(1, false, "reviewer"),
                note(2, true, "reviewer"),
                note(3, false, "author"),
                note(1, false, "reviewer"),
                json!({}),
            ],
            &mr,
            "2026-09-03T00:00:00Z".parse().unwrap(),
            &mut people,
            &mut count,
        );
        assert!(!valid);
        assert_eq!(count, 1);
        assert_eq!(people["reviewer"], (1, 1));
        assert!(first.is_some());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn exhausted_budget_is_unknown_and_first_429_stops_more_comments() {
        use std::os::unix::fs::PermissionsExt;
        let report = super::super::tests::report(vec![
            super::super::tests::sample(1),
            super::super::tests::sample(2),
        ]);
        let missing = load(Path::new("no-program"), &report, Duration::ZERO).await;
        assert_eq!(missing.comments, None);
        assert!(!missing.complete);
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, "#!/bin/sh\nprintf 'HTTP/2 429\\nratelimit-remaining: 0\\nratelimit-reset: 123\\n\\nlimited'\nexit 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let limited = crate::gitlab::test_support::scripted(
            &program,
            load(&program, &report, Duration::from_secs(1)),
        )
        .await;
        assert_eq!(limited.comments, None);
        assert_eq!(limited.failures.len(), 1);
        assert_eq!(limited.rate_remaining, Some(0));
        assert_eq!(limited.rate_reset, Some(123));
    }
}
