use super::*;
use crate::queue_scan::{self, Candidate, Commit, Loaded};

pub(super) struct Target<'a> {
    pub source: &'a Source,
    pub list: CachedList,
    pub owner: &'a str,
}
pub(super) async fn advance(
    program: &Path,
    target: Target<'_>,
    loaded: Loaded,
    previous: &[MergeRequest],
    budget: Duration,
    now: i64,
) -> Result<FetchedList, QueueError> {
    let Target {
        source,
        list,
        owner,
    } = target;
    let started = tokio::time::Instant::now();
    let mut state = loaded.state;
    let mut mrs = vec![];
    let mut removals = vec![];
    if state.done && now >= state.eligible_at {
        state.fresh_pass();
    }
    state.receipt_id = Some(queue_scan::new_receipt_id());
    let due = state.candidates.iter().position(|c| c.eligible_at <= now);
    if let Some(index) = due {
        let mut candidate = state.candidates.remove(index).unwrap();
        if let Some(row) = previous.iter().find(|r| r.identity() == candidate.identity) {
            let head = row.head_oid.clone().unwrap_or_default();
            if head != candidate.head
                || row.id.to_string() != candidate.id
                || row.created_at != candidate.created_at
            {
                candidate.negative_at = None;
                candidate.head = head;
                candidate.id = row.id.to_string();
                candidate.created_at = row.created_at;
            }
        }
        let filter = if list == CachedList::Reviewing {
            "reviewer_username"
        } else {
            "author_username"
        };
        let endpoint = format!(
            "projects/{}/merge_requests?scope=all&state=opened&iids[]={}&{}={}&per_page=25&page=1",
            crate::gitlab::detail::encode_project(&candidate.identity.repo),
            candidate.identity.number,
            filter,
            crate::gitlab::detail::encode_project(owner)
        );
        let proof = request(
            program,
            &source.host,
            &endpoint,
            budget
                .saturating_sub(started.elapsed())
                .min(REQUEST_TIMEOUT),
        )
        .await;
        match proof {
            Ok(page)
                if page.confirmation_terminal
                    && page.next.is_none()
                    && page.total == Some(page.rows.len() as u64)
                    && page.rows.len() <= 1 =>
            {
                let positive = page.rows.first().and_then(|v| map_row(v, source));
                if let Some(row) = positive.filter(|r| {
                    r.identity() == candidate.identity && r.id.to_string() == candidate.id
                }) {
                    // A matching requested membership cancels its negative sequence.
                    mrs.push(row);
                } else if page.rows.is_empty() {
                    if candidate
                        .negative_at
                        .is_some_and(|t| now >= t + queue_scan::CONFIRM_DELAY)
                    {
                        removals.push(candidate.identity);
                    } else {
                        candidate.negative_at.get_or_insert(now);
                        candidate.eligible_at = now + queue_scan::CONFIRM_DELAY;
                        candidate.failures = 0;
                        state.candidates.push_back(candidate);
                    }
                } else {
                    defer(&mut state, candidate, now);
                }
            }
            _ => defer(&mut state, candidate, now),
        }
    }
    let scope = if list == CachedList::Reviewing {
        "reviews_for_me"
    } else {
        "created_by_me"
    };
    let mut complete = false;
    if !state.done && now >= state.eligible_at {
        for _ in 0..2 {
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let page_number = state
                .after
                .as_deref()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            let endpoint = format!(
                "merge_requests?scope={scope}&state=opened&per_page={PAGE_SIZE}&page={page_number}"
            );
            let page = match request(
                program,
                &source.host,
                &endpoint,
                remaining.min(REQUEST_TIMEOUT),
            )
            .await
            {
                Ok(p) => p,
                Err(_) => {
                    state.failure(now);
                    break;
                }
            };
            state.observe_count(page.total);
            let raw_len = page.rows.len();
            let mapped = page
                .rows
                .iter()
                .filter_map(|r| map_row(r, source))
                .collect::<Vec<_>>();
            if mapped.len() != raw_len
                || mapped
                    .iter()
                    .map(|r| r.identity())
                    .collect::<HashSet<_>>()
                    .len()
                    != mapped.len()
            {
                state.tainted = true;
            }
            for row in mapped {
                state.candidates.retain(|c| c.identity != row.identity());
                if !state.seen.contains(&row.identity()) && state.seen.len() < queue_scan::MEMBERS {
                    state.seen.push(row.identity());
                }
                if let Some(old) = mrs.iter_mut().find(|r| r.identity() == row.identity()) {
                    *old = row;
                } else {
                    mrs.push(row);
                }
            }
            state.pages += 1;
            state.failures = 0;
            if page.terminal_known && page.next.is_none() {
                state.done = true;
                state.eligible_at = now + queue_scan::CONFIRM_DELAY;
                complete = state.pages == 1
                    && !state.tainted
                    && state.total == Some(state.seen.len() as u64);
                for row in previous {
                    if state.candidates.len() >= queue_scan::CANDIDATES {
                        break;
                    }
                    if !state.seen.contains(&row.identity())
                        && !state
                            .candidates
                            .iter()
                            .any(|c| c.identity == row.identity())
                    {
                        state.candidates.push_back(Candidate {
                            identity: row.identity(),
                            id: row.id.to_string(),
                            head: row.head_oid.clone().unwrap_or_default(),
                            created_at: row.created_at,
                            negative_at: None,
                            eligible_at: now,
                            failures: 0,
                        });
                    }
                }
                break;
            }
            if state.seen.len() >= queue_scan::MEMBERS {
                state.done = true;
                state.ceiling = true;
                state.tainted = true;
                state.eligible_at = now + queue_scan::CONFIRM_DELAY;
                break;
            }
            match page.next {
                Some(next) if next > page_number && page.terminal_known => {
                    state.after = Some(next.to_string())
                }
                _ => {
                    state.failure(now);
                    break;
                }
            }
        }
    }
    // With no confirmation due, the spare slot refreshes only the head.
    if due.is_none()
        && now >= state.eligible_at
        && !state.done
        && state.after.is_some()
        && !budget.saturating_sub(started.elapsed()).is_zero()
    {
        let endpoint =
            format!("merge_requests?scope={scope}&state=opened&per_page={PAGE_SIZE}&page=1");
        if let Ok(page) = request(
            program,
            &source.host,
            &endpoint,
            budget
                .saturating_sub(started.elapsed())
                .min(REQUEST_TIMEOUT),
        )
        .await
        {
            state.observe_count(page.total);
            for mut row in page.rows.iter().filter_map(|r| map_row(r, source)) {
                state.candidates.retain(|c| c.identity != row.identity());
                row.viewer = Some(owner.into());
                if let Some(old) = mrs.iter_mut().find(|r| r.identity() == row.identity()) {
                    *old = row;
                } else {
                    mrs.push(row);
                }
            }
        }
    }
    for row in &mut mrs {
        row.viewer = Some(owner.into());
    }
    removals.retain(|id| !mrs.iter().any(|r| &r.identity() == id));
    let total = state.total;
    Ok(FetchedList {
        viewer: Some(owner.into()),
        mrs,
        total,
        coverage: if complete {
            Coverage::Complete
        } else {
            Coverage::Partial { total }
        },
        scan: Some(Commit {
            expected_revision: loaded.revision,
            state,
            removals,
        }),
    })
}
fn defer(state: &mut queue_scan::State, mut candidate: Candidate, now: i64) {
    candidate.negative_at = None;
    candidate.failures = candidate.failures.saturating_add(1);
    candidate.eligible_at = now + queue_scan::backoff(candidate.failures);
    state.candidates.push_back(candidate);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    fn row(n: usize) -> Value {
        json!({"id":n,"iid":n,"title":"Synthetic change","web_url":format!("https://gitlab.com/group/project/-/merge_requests/{n}"),"author":{"username":"fixture"},"draft":false,"source_branch":"topic","target_branch":"main","created_at":"2026-09-01T10:00:00Z","updated_at":"2026-09-02T10:00:00Z","sha":format!("head-{n}"),"reviewers":[],"assignees":[],"labels":[],"user_notes_count":0})
    }
    fn source() -> Source {
        Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        }
    }
    #[tokio::test]
    async fn six_hundred_and_one_rows_continue_across_restart_with_three_data_calls() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        for page in 1..=7 {
            let end = (page * 100).min(601);
            let rows = ((page - 1) * 100 + 1..=end).map(row).collect::<Vec<_>>();
            let next = if page == 7 {
                String::new()
            } else {
                (page + 1).to_string()
            };
            std::fs::write(
                dir.path().join(format!("page{page}")),
                format!(
                    "HTTP/2 200\nx-total: 601\nx-next-page: {next}\n\n{}",
                    json!(rows)
                ),
            )
            .unwrap();
        }
        std::fs::write(&program,"#!/bin/sh\necho request >> \"$0.calls\"\npage=${5##*page=}\ncat \"${0%/*}/page$page\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let mut state = queue_scan::State::default();
            let mut inventory = vec![];
            for step in 0..4 {
                let before = std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap_or_default()
                    .lines()
                    .count();
                let result = advance(
                    &program,
                    Target {
                        source: &source(),
                        list: CachedList::Reviewing,
                        owner: "fixture",
                    },
                    Loaded {
                        revision: step,
                        state,
                    },
                    &inventory,
                    Duration::from_secs(3),
                    1000 + step * 60,
                )
                .await
                .unwrap();
                let after = std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count();
                assert!(after - before <= 3);
                state = result.scan.unwrap().state;
                state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
                inventory =
                    crate::inventory::reconcile(inventory, result.mrs, false, chrono::Utc::now());
            }
            assert_eq!(inventory.len(), 601);
            assert!(state.done);
            assert_eq!(state.seen.len(), 601);
        })
        .await;
    }
    #[tokio::test]
    async fn native_exact_membership_requires_two_spaced_complete_negatives() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program,"#!/bin/sh\ncase \"$5\" in projects/group%2Fproject/merge_requests?scope=all\\&state=opened\\&iids\\[\\]=7\\&reviewer_username=fixture\\&per_page=25\\&page=1) printf 'HTTP/2 200\\nx-total: 0\\nx-next-page:\\n\\n[]';; *) exit 91;; esac\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let mr = map_row(&row(7), &source()).unwrap();
            let candidate = Candidate {
                identity: mr.identity(),
                id: mr.id.to_string(),
                head: mr.head_oid.clone().unwrap(),
                created_at: mr.created_at,
                negative_at: None,
                eligible_at: 0,
                failures: 0,
            };
            let state = queue_scan::State {
                done: true,
                eligible_at: i64::MAX,
                candidates: [candidate].into(),
                ..queue_scan::State::default()
            };
            let first = advance(
                &program,
                Target {
                    source: &source(),
                    list: CachedList::Reviewing,
                    owner: "fixture",
                },
                Loaded { revision: 0, state },
                std::slice::from_ref(&mr),
                Duration::from_secs(2),
                1000,
            )
            .await
            .unwrap()
            .scan
            .unwrap();
            assert!(first.removals.is_empty());
            assert_eq!(first.state.candidates[0].negative_at, Some(1000));
            let second = advance(
                &program,
                Target {
                    source: &source(),
                    list: CachedList::Reviewing,
                    owner: "fixture",
                },
                Loaded {
                    revision: 1,
                    state: first.state.clone(),
                },
                std::slice::from_ref(&mr),
                Duration::from_secs(2),
                1060,
            )
            .await
            .unwrap()
            .scan
            .unwrap();
            assert_eq!(second.removals.len(), 1);
            assert!(second.state.candidates.is_empty());

            // Round1: a terminal-looking second response cannot remove through
            // contradictory or malformed pagination. Each is one actual proof call.
            for headers in [
                "x-total: 0\nx-next-page:\nx-page: 2",
                "x-total: 0\nx-next-page:\nLink: <https://gitlab.com/api/v4/projects/group%2Fproject/merge_requests?page=2>; rel=\"next\"",
                "x-total: 0\nx-next-page:\nx-total: 1",
                "x-total: 0\nx-next-page:\nx-next-page: 2",
                "x-total: 0\nx-next-page:\nx-page: invalid",
                "x-total: 0\nx-next-page:\nx-total-pages: 2",
                "x-total: 0\nx-next-page:\nx-prev-page: 1",
                "x-total: 0\nx-next-page:\nx-per-page: broken",
                "x-total: 0\nx-next-page:\nLink: broken",
                "x-total: 0\nx-next-page:\nLink: <https://gitlab.com/api/v4/merge_requests?page=1>; rel=\"first",
                "x-total: 0\nx-next-page:\nx-page: 1\nx-page: 2",
                "x-next-page:\nx-page: 1",
            ] {
                std::fs::write(&program,format!("#!/bin/sh\necho proof >> \"$0.proofs\"\nprintf '%s' 'HTTP/2 200\n{headers}\n\n[]'\n")).unwrap();
                let before=std::fs::read_to_string(program.with_extension("proofs")).unwrap_or_default().lines().count();
                let proof=advance(&program,Target{source:&source(),list:CachedList::Reviewing,owner:"fixture"},Loaded{revision:1,state:first.state.clone()},std::slice::from_ref(&mr),Duration::from_secs(2),1060).await.unwrap().scan.unwrap();
                assert!(proof.removals.is_empty(),"contradictory proof: {headers}");assert_eq!(proof.state.candidates[0].negative_at,None);assert!(proof.state.candidates[0].eligible_at>1060);
                assert_eq!(std::fs::read_to_string(program.with_extension("proofs")).unwrap().lines().count(),before+1);
            }

            std::fs::write(&program,"#!/bin/sh\necho proof >> \"$0.proofs\"\nprintf '%s' 'HTTP/2 200\nx-total: 0\nx-next-page:\nx-page: 1\nx-per-page: 25\nx-total-pages: 1\nLink: <https://gitlab.com/api/v4/merge_requests?page=1>; rel=\"first\", <https://gitlab.com/api/v4/merge_requests?page=1>; rel=\"last\"\n\n[]'\n").unwrap();
            let before=std::fs::read_to_string(program.with_extension("proofs")).unwrap().lines().count();
            let consistent=advance(&program,Target{source:&source(),list:CachedList::Reviewing,owner:"fixture"},Loaded{revision:1,state:first.state.clone()},std::slice::from_ref(&mr),Duration::from_secs(2),1060).await.unwrap().scan.unwrap();
            assert_eq!(consistent.removals.len(),1);assert_eq!(std::fs::read_to_string(program.with_extension("proofs")).unwrap().lines().count(),before+1);
            // Reopening/re-requesting is positive membership, never a tombstone.
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\nprintf 'HTTP/2 200\\nx-total: 1\\nx-next-page:\\n\\n%s' '{}'\n",
                    json!([row(7)])
                ),
            )
            .unwrap();
            let positive = advance(
                &program,
                Target {
                    source: &source(),
                    list: CachedList::Reviewing,
                    owner: "fixture",
                },
                Loaded {
                    revision: 1,
                    state: first.state.clone(),
                },
                std::slice::from_ref(&mr),
                Duration::from_secs(2),
                1060,
            )
            .await
            .unwrap();
            assert_eq!(positive.mrs.len(), 1);
            assert!(positive.scan.unwrap().state.candidates.is_empty());
            // Missing pagination does not constitute a second negative.
            std::fs::write(
                &program,
                "#!/bin/sh\nprintf 'HTTP/2 200\\nx-total: 0\\n\\n[]'\n",
            )
            .unwrap();
            let unknown = advance(
                &program,
                Target {
                    source: &source(),
                    list: CachedList::Reviewing,
                    owner: "fixture",
                },
                Loaded {
                    revision: 1,
                    state: first.state.clone(),
                },
                std::slice::from_ref(&mr),
                Duration::from_secs(2),
                1060,
            )
            .await
            .unwrap()
            .scan
            .unwrap();
            assert!(unknown.removals.is_empty());
            assert_eq!(unknown.state.candidates[0].negative_at, None);
            // A new known head clears the earlier head's negative sequence.
            std::fs::write(
                &program,
                "#!/bin/sh\nprintf 'HTTP/2 200\\nx-total: 0\\nx-next-page:\\n\\n[]'\n",
            )
            .unwrap();
            let mut changed = mr;
            changed.head_oid = Some("new-head".into());
            let renewed = advance(
                &program,
                Target {
                    source: &source(),
                    list: CachedList::Reviewing,
                    owner: "fixture",
                },
                Loaded {
                    revision: 1,
                    state: first.state,
                },
                &[changed],
                Duration::from_secs(2),
                1060,
            )
            .await
            .unwrap()
            .scan
            .unwrap();
            assert!(renewed.removals.is_empty());
            assert_eq!(renewed.state.candidates[0].negative_at, Some(1060));
        })
        .await;
    }
}
