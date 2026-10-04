//! Oversized search traversal. Network admission and publication remain in scan.rs.
use super::*;
use crate::queue_scan::{BlockReason, GithubPartition, Leaf, PartitionPhase, Window};
use chrono::{DateTime, SecondsFormat, Utc};

pub(super) fn request(state: &mut State, list: CachedList) -> (String, u64, Option<String>) {
    let p = state.github_partition.as_mut().unwrap();
    let base = predicate(list);
    match p.phase {
        PartitionPhase::LowerBound => (format!("{base} sort:created-asc"), 1, None),
        PartitionPhase::UpperBound => (format!("{base} sort:created-desc"), 1, None),
        PartitionPhase::FinalCheck => (base.into(), 1, None),
        PartitionPhase::Leaves => {
            let leaf = p
                .active
                .get_or_insert_with(|| Leaf::new(p.pending.pop_front().unwrap()));
            let stamp = |t| {
                DateTime::from_timestamp(t, 0)
                    .unwrap()
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            };
            (
                format!(
                    "{base} created:{}..{}",
                    stamp(leaf.window.lo),
                    stamp(leaf.window.hi)
                ),
                25,
                leaf.after.clone(),
            )
        }
    }
}
pub(super) fn reset_leaf(state: &mut State) {
    if let Some(leaf) = state
        .github_partition
        .as_mut()
        .and_then(|p| p.active.as_mut())
    {
        *leaf = Leaf::new(leaf.window);
    }
}
fn retire(p: &mut GithubPartition) {
    p.active = None;
    if p.pending.is_empty() {
        p.phase = PartitionPhase::FinalCheck;
    }
}
fn finish(state: &mut State, now: i64, complete: bool) {
    state.done = true;
    state.finished_at = Some(now);
    state.eligible_at = now + state.pass_delay.max(queue_scan::CONFIRM_DELAY);
    state.coverage_valid = complete;
    if complete {
        state.completed_at = Some(now);
        state.completed_total = state.total;
    }
}
/// Every response contributes useful observations, even a split parent or refused leaf.
pub(super) fn accept(state: &mut State, raw: &Value, prs: &mut Vec<PullRequest>, now: i64) {
    let mut p = state.github_partition.take().unwrap();
    let page = &raw["authored"];
    let rows = map_list(raw, "authored");
    let total = page["issueCount"].as_u64();
    let valid = clean(raw)
        && total.is_some()
        && rows.len()
            <= if p.phase == PartitionPhase::Leaves {
                25
            } else {
                1
            }
        && page["nodes"]
            .as_array()
            .is_some_and(|n| n.len() == rows.len())
        && rows.iter().all(|r| !r.id.is_empty())
        && rows
            .iter()
            .map(|r| r.identity())
            .collect::<HashSet<_>>()
            .len()
            == rows.len()
        && p.active.as_ref().is_none_or(|leaf| {
            rows.iter().all(|row| {
                let time = row.created_at;
                time.timestamp() >= leaf.window.lo
                    && (time.timestamp() < leaf.window.hi
                        || time.timestamp() == leaf.window.hi && time.timestamp_subsec_nanos() == 0)
            })
        });
    state.received = true;
    state.failures = 0;
    if !valid {
        state.tainted = true;
        state.coverage_valid = false;
    }
    for row in &rows {
        let id = row.identity();
        state.candidates.retain(|c| c.identity != id);
        if !state.seen.contains(&id) {
            if state.seen.len() < queue_scan::MEMBERS {
                state.seen.push(id);
            } else {
                state.ceiling = true;
                state.tainted = true;
            }
        }
        merge_observation(prs, row.clone());
    }
    match p.phase {
        PartitionPhase::LowerBound | PartitionPhase::UpperBound => {
            state.observe_count(total);
            let timestamp = page["nodes"][0]["createdAt"]
                .as_str()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            if valid
                && total == Some(0)
                && rows.is_empty()
                && page["pageInfo"]["hasNextPage"] == false
            {
                finish(state, now, !state.tainted && state.seen.is_empty());
            } else if valid
                && total.is_some_and(|n| n > 0)
                && rows.len() == 1
                && timestamp.is_some()
            {
                let t = timestamp.unwrap();
                if p.phase == PartitionPhase::LowerBound {
                    p.lower = Some(t.timestamp());
                    p.phase = PartitionPhase::UpperBound;
                } else {
                    let hi = t.timestamp() + i64::from(t.timestamp_subsec_nanos() != 0);
                    if p.lower.is_some_and(|lo| lo <= hi)
                        && DateTime::from_timestamp(hi, 0).is_some()
                    {
                        p.upper = Some(hi);
                        p.pending.push_back(Window {
                            lo: p.lower.unwrap(),
                            hi,
                        });
                        p.windows_started = 1;
                        p.phase = PartitionPhase::Leaves;
                    } else {
                        state.failure(now);
                    }
                }
            } else {
                state.failure(now);
            }
        }
        PartitionPhase::Leaves => {
            let leaf = p.active.as_mut().unwrap();
            let window = leaf.window;
            if leaf.pages > 0 && leaf.total != total {
                state.tainted = true;
            }
            if total.is_some_and(|n| n > 1000) {
                if leaf.pages > 0 {
                    state.tainted = true;
                }
                if window.hi - window.lo <= 1 {
                    p.blocked.push((window, BlockReason::TimestampResolution));
                    state.ceiling = true;
                } else if p.windows_started + 2 > queue_scan::PARTITION_WINDOWS {
                    p.blocked.push((window, BlockReason::WindowLimit));
                    state.ceiling = true;
                } else {
                    let mid = window.lo + (window.hi - window.lo) / 2;
                    p.pending.push_back(Window {
                        lo: window.lo,
                        hi: mid,
                    });
                    p.pending.push_back(Window {
                        lo: mid,
                        hi: window.hi,
                    });
                    p.windows_started += 2;
                }
                retire(&mut p);
            } else if raw["__queue_cursor_invalid"] == true && leaf.after.is_some() {
                *leaf = Leaf::new(window);
                state.failure(now);
            } else {
                leaf.total = total;
                leaf.pages += 1;
                for row in rows {
                    let id = row.identity();
                    if !leaf.seen.contains(&id) && leaf.seen.len() < 1000 {
                        leaf.seen.push(id);
                    }
                }
                let next = page["pageInfo"]["hasNextPage"].as_bool();
                let cursor = page["pageInfo"]["endCursor"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 4096);
                if next == Some(false) {
                    if !valid || total != Some(leaf.seen.len() as u64) {
                        p.blocked.push((window, BlockReason::InvalidMetadata));
                    }
                    retire(&mut p);
                } else if leaf.pages >= 40 || leaf.seen.len() >= 1000 {
                    p.blocked.push((window, BlockReason::MembershipLimit));
                    state.ceiling = true;
                    retire(&mut p);
                } else if let Some(cursor) = cursor.filter(|c| {
                    next == Some(true)
                        && Some(*c) != leaf.after.as_deref()
                        && !leaf.cursors.iter().any(|old| old == c)
                }) {
                    let cursor = cursor.to_string();
                    leaf.cursors.push(cursor.clone());
                    leaf.after = Some(cursor);
                } else {
                    // Retire an unusable leaf so it cannot starve valid siblings.
                    p.blocked.push((window, BlockReason::InvalidMetadata));
                    state.tainted = true;
                    retire(&mut p);
                }
            }
        }
        PartitionPhase::FinalCheck => {
            state.observe_count(total);
            let complete = valid
                && !state.tainted
                && !state.ceiling
                && p.blocked.is_empty()
                && total == Some(state.seen.len() as u64);
            finish(state, now, complete);
        }
    }
    if state.ceiling || !p.blocked.is_empty() {
        state.coverage_valid = false;
    }
    state.github_partition = Some(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn leaf_state(window: Window) -> State {
        State {
            github_partition: Some(GithubPartition {
                phase: PartitionPhase::Leaves,
                lower: Some(window.lo),
                upper: Some(window.hi),
                active: Some(Leaf::new(window)),
                windows_started: 1,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    fn page(total: Value, next: bool, cursor: Option<&str>) -> Value {
        json!({"viewer":{"login":"fixture"},"authored":{"issueCount":total,"nodes":[],"pageInfo":{"hasNextPage":next,"endCursor":cursor}}})
    }
    #[test]
    fn inclusive_windows_serialize_as_documented_and_fifo_split_does_not_sum_counts() {
        let mut state = leaf_state(Window {
            lo: 1577836800,
            hi: 1672531200,
        });
        let (q, first, after) = request(&mut state, CachedList::Reviewing);
        assert_eq!(
            q,
            "is:pr is:open review-requested:@me created:2020-01-01T00:00:00Z..2023-01-01T00:00:00Z"
        );
        assert_eq!((first, after), (25, None));
        state.total = Some(2750);
        state.count_seen = true;
        accept(
            &mut state,
            &page(json!(2000), true, Some("unused")),
            &mut vec![],
            1000,
        );
        assert_eq!(
            state.total,
            Some(2750),
            "leaf count must not replace the global total"
        );
        let p = state.github_partition.unwrap();
        assert_eq!(p.pending.len(), 2);
        assert_eq!(
            p.pending[0].hi, p.pending[1].lo,
            "inclusive midpoint overlap"
        );
    }
    #[test]
    fn window_and_leaf_page_caps_finish_partial_without_dropping_pending_siblings() {
        let window = Window { lo: 0, hi: 8 };
        let mut state = leaf_state(window);
        state.github_partition.as_mut().unwrap().windows_started = 1023;
        accept(
            &mut state,
            &page(json!(1001), true, Some("tail")),
            &mut vec![],
            1000,
        );
        let p = state.github_partition.as_ref().unwrap();
        assert_eq!(p.windows_started, 1023);
        assert_eq!(p.blocked[0].1, BlockReason::WindowLimit);
        assert_eq!(p.phase, PartitionPhase::FinalCheck);
        assert!(!state.coverage_valid);
        let mut state = leaf_state(window);
        let p = state.github_partition.as_mut().unwrap();
        p.active.as_mut().unwrap().pages = 39;
        p.pending.push_back(Window { lo: 8, hi: 9 });
        p.windows_started = 2;
        accept(
            &mut state,
            &page(Value::Null, true, Some("tail")),
            &mut vec![],
            1000,
        );
        let p = state.github_partition.as_ref().unwrap();
        assert_eq!(p.blocked[0].1, BlockReason::MembershipLimit);
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.phase, PartitionPhase::Leaves);
        assert!(state.tainted && !state.coverage_valid);
    }
    #[test]
    fn final_global_count_and_every_leaf_proof_are_required() {
        for (tainted, blocked, total) in [(true, false, 0), (false, true, 0), (false, false, 1)] {
            let mut state = leaf_state(Window { lo: 0, hi: 1 });
            state.total = Some(0);
            state.count_seen = true;
            state.tainted = tainted;
            let p = state.github_partition.as_mut().unwrap();
            p.active = None;
            p.phase = PartitionPhase::FinalCheck;
            if blocked {
                p.blocked
                    .push((Window { lo: 0, hi: 1 }, BlockReason::InvalidMetadata));
            }
            accept(
                &mut state,
                &page(json!(total), false, None),
                &mut vec![],
                1000,
            );
            assert!(state.done && !state.coverage_valid);
        }
    }
    #[test]
    fn invalid_leaf_timestamp_cannot_validate_a_leaf() {
        for stamp in ["invalid", "2020-01-01T00:00:00Z"] {
            let mut node =
                serde_json::from_str::<Value>(include_str!("../../../tests/fixtures/search.json"))
                    .unwrap()["authored"]["nodes"][0]
                    .clone();
            node["createdAt"] = json!(stamp);
            let raw = json!({"authored":{"issueCount":1,"nodes":[node],"pageInfo":{"hasNextPage":false}}});
            let mut state = leaf_state(Window { lo: 0, hi: 1 });
            accept(&mut state, &raw, &mut vec![], 1000);
            assert!(
                state.tainted,
                "unknown/out-of-window creation time cannot validate membership traversal"
            );
        }
    }
    #[test]
    fn pass_identity_cap_preserves_new_observations_without_certifying_truncated_proof() {
        let mut node =
            serde_json::from_str::<Value>(include_str!("../../../tests/fixtures/search.json"))
                .unwrap()["authored"]["nodes"][0]
                .clone();
        node["number"] = json!(20000);
        let mut state = State {
            github_partition: Some(GithubPartition::default()),
            ..Default::default()
        };
        let id = identity(&node).unwrap();
        state.seen = (0..10000)
            .map(|number| PrIdentity {
                number,
                ..id.clone()
            })
            .collect();
        let raw =
            json!({"authored":{"issueCount":10001,"nodes":[node],"pageInfo":{"hasNextPage":true}}});
        let mut rows = vec![];
        accept(&mut state, &raw, &mut rows, 1000);
        assert_eq!(rows.len(), 1);
        assert_eq!(state.seen.len(), 10000);
        assert!(state.ceiling && state.tainted && !state.coverage_valid);
        assert!(state.partition_reason().is_some());
    }
    #[test]
    fn unknown_probe_timestamp_and_count_keep_valid_rows_without_inventing_bounds() {
        let node =
            serde_json::from_str::<Value>(include_str!("../../../tests/fixtures/search.json"))
                .unwrap()["authored"]["nodes"][0]
                .clone();
        for missing_time in [true, false] {
            let mut incoming = node.clone();
            if missing_time {
                incoming["createdAt"] = json!("invalid");
            }
            let raw = json!({"authored":{"issueCount":if missing_time { json!(1025) } else { Value::Null },"nodes":[incoming],"pageInfo":{"hasNextPage":true}}});
            let mut state = State {
                github_partition: Some(GithubPartition::default()),
                ..Default::default()
            };
            // Existing valid observations survive a later malformed row. The mapper
            // intentionally refuses a row whose required creation time is invalid.
            let mut rows = map_list(&json!({"authored":{"nodes":[node.clone()]}}), "authored");
            accept(&mut state, &raw, &mut rows, 1000);
            assert_eq!(rows.len(), 1);
            assert_eq!(
                state.github_partition.unwrap().phase,
                PartitionPhase::LowerBound
            );
            assert!(!state.done && state.tainted && state.eligible_at > 1000);
        }
    }
}
