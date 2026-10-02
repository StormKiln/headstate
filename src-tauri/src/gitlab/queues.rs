//! Bounded GitLab merge-request lists. `glab` owns credentials; neither
//! the credential nor raw CLI diagnostics leave this module.
mod scan;
use crate::{
    github::model::{CiState, Label, ReviewState},
    identity::{PrIdentity, Provider, Source},
    store::{source_cache::Coverage, CachedList},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, path::Path, process::Stdio, time::Duration};

const PAGE_SIZE: usize = 100;
#[cfg(test)]
const MAX_PAGES: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

// A read started after a mutation must never join the pre-mutation read.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn invalidate_identity(identity: &PrIdentity) {
    super::enrichment::invalidate(identity);
}
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}
pub fn invalidate() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}

/// A list receipt, with absent detail measures left absent. It is never
/// deserialized as a GitHub PullRequest or sent through GitHub row events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<crate::inventory::RowObservation>,
    #[serde(default)]
    pub viewer: Option<String>,
    pub source: Source,
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub author: String,
    pub is_draft: bool,
    pub head_ref: String,
    pub head_oid: Option<String>,
    pub base_ref: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub labels: Vec<Label>,
    pub reviewers: Vec<String>,
    pub assignees: Vec<String>,
    pub comment_count: u64,
    pub detailed_merge_status: Option<String>,
    pub ci: Option<CiState>,
    pub review: Option<ReviewState>,
    pub unresolved_threads: Option<u64>,
    #[serde(default)]
    pub unresolved_threads_floor: bool,
    #[serde(default)]
    pub needs_my_review: Option<bool>,
    #[serde(default)]
    pub in_merge_queue: Option<bool>,
    #[serde(default)]
    pub can_enqueue_train: Option<bool>,
}

impl MergeRequest {
    pub fn identity(&self) -> PrIdentity {
        PrIdentity {
            source: self.source.clone(),
            repo: self.repo.clone(),
            number: self.number,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedList {
    pub scan: Option<crate::queue_scan::Commit>,
    pub viewer: Option<String>,
    pub mrs: Vec<MergeRequest>,
    pub total: Option<u64>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum QueueError {
    #[error("GitLab CLI (glab) was not found on the desktop")]
    MissingCli,
    #[error(
        "GitLab rejected the configured host's credential; check glab authentication for that host"
    )]
    Unauthorized,
    #[error("GitLab denied this merge-request list request")]
    Forbidden,
    #[error("GitLab rate limit reached; try again later")]
    RateLimited,
    #[error("GitLab request timed out")]
    Timeout,
    #[error("GitLab request failed")]
    Request,
    #[error("GitLab returned an unreadable merge-request page")]
    InvalidPage,
    #[error("this GitLab host is not configured for fetching")]
    UnsupportedHost,
}

impl QueueError {
    pub fn transient(&self) -> bool {
        matches!(self, Self::Timeout | Self::Request)
    }
}

#[derive(Debug)]
struct Page {
    rows: Vec<Value>,
    total: Option<u64>,
    next: Option<usize>,
    terminal_known: bool,
    confirmation_terminal: bool,
}

pub async fn fetch<F: std::future::Future<Output = bool>>(
    source: &Source,
    list: CachedList,
    db_path: &Path,
    progress: impl FnOnce(FetchedList) -> F,
) -> Result<FetchedList, QueueError> {
    if source.provider != Provider::Gitlab || super::host::validate(&source.host).is_err() {
        return Err(QueueError::UnsupportedHost);
    }
    let program = super::auth::find_glab().ok_or(QueueError::MissingCli)?;
    static READS: std::sync::LazyLock<super::coalesce::Reads<Result<FetchedList, QueueError>>> =
        std::sync::LazyLock::new(super::coalesce::Reads::default);
    let deadline = tokio::time::Instant::now() + FETCH_TIMEOUT;
    let viewer = tokio::time::timeout_at(deadline, super::auth::viewer(&program, &source.host))
        .await
        .map_err(|_| QueueError::Timeout)?
        .map_err(|_| QueueError::Unauthorized)?;
    let key = format!(
        "{}:{viewer}:{list:?}:{}",
        source.host,
        GENERATION.load(std::sync::atomic::Ordering::Acquire)
    );
    READS
        .run(key, async {
            let generation = GENERATION.load(std::sync::atomic::Ordering::Acquire);
            let path = db_path.to_path_buf();
            let owned_source = source.clone();
            let owner = viewer.clone();
            let (loaded, previous) = tauri::async_runtime::spawn_blocking(
                move || -> Result<_, crate::store::StoreError> {
                    let conn = crate::store::open_db(&path)?;
                    let conn = conn.unchecked_transaction()?;
                    let loaded = crate::queue_scan::load(&conn, &owned_source, list, &owner)?;
                    let rows =
                        if crate::store::source_cache::snapshot_owner(&conn, &owned_source, list)?
                            .as_deref()
                            == Some(&owner)
                        {
                            match crate::store::source_cache::load_source_snapshot(
                                &conn,
                                &owned_source,
                                list,
                            )?
                            .data
                            {
                                crate::store::source_cache::SnapshotData::GitLabAvailable {
                                    mrs,
                                    ..
                                } => mrs,
                                _ => vec![],
                            }
                        } else {
                            vec![]
                        };
                    Ok((loaded, rows))
                },
            )
            .await
            .map_err(|_| QueueError::Request)?
            .map_err(|_| QueueError::Request)?;
            let mut result = scan::advance(
                &program,
                scan::Target {
                    source,
                    list,
                    owner: &viewer,
                },
                loaded,
                &previous,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                Utc::now().timestamp(),
            )
            .await?;
            result.viewer = Some(viewer.clone());
            for row in &mut result.mrs {
                row.viewer = Some(viewer.clone());
            }
            if tokio::time::timeout_at(deadline, super::auth::viewer(&program, &source.host))
                .await
                .map_err(|_| QueueError::Timeout)?
                .map_err(|_| QueueError::Unauthorized)?
                != viewer
            {
                return Err(QueueError::Unauthorized);
            }
            if generation != GENERATION.load(std::sync::atomic::Ordering::Acquire) {
                return Err(QueueError::Request);
            }
            super::enrichment::fill_cached(&mut result.mrs, generation);
            if progress(result.clone()).await {
                if let Some(scan) = result.scan.as_mut() {
                    scan.expected_revision += 1;
                }
            }
            super::enrichment::enrich(
                &program,
                &mut result.mrs,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                generation,
            )
            .await;
            if tokio::time::timeout_at(deadline, super::auth::viewer(&program, &source.host))
                .await
                .map_err(|_| QueueError::Timeout)?
                .map_err(|_| QueueError::Unauthorized)?
                != viewer
            {
                return Err(QueueError::Unauthorized);
            }
            if generation != GENERATION.load(std::sync::atomic::Ordering::Acquire) {
                return Err(QueueError::Request);
            }
            Ok(result)
        })
        .await
}

#[cfg(test)]
async fn fetch_with_program(
    program: &Path,
    source: &Source,
    list: CachedList,
    budget: Duration,
) -> Result<FetchedList, QueueError> {
    let started = tokio::time::Instant::now();
    let scope = match list {
        CachedList::Authored => "created_by_me",
        CachedList::Reviewing => "reviews_for_me",
    };
    let mut mrs = Vec::new();
    let mut total = None;
    let mut total_conflict = false;
    let mut page_number = 1;
    let mut seen = HashSet::new();
    let mut incomplete = false;
    let mut complete = false;
    let mut pages_received = false;
    for _ in 0..MAX_PAGES {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            incomplete = true;
            break;
        }
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
            Ok(page) => page,
            Err(error) if !pages_received => return Err(error),
            Err(_) => {
                incomplete = true;
                break;
            }
        };
        pages_received = true;
        if let Some(n) = page.total {
            if total.is_some_and(|prior| prior != n) {
                total_conflict = true;
                incomplete = true;
            }
            if !total_conflict {
                total = Some(n);
            } else {
                total = None;
            }
        }
        let raw_len = page.rows.len();
        for row in page.rows {
            match map_row(&row, source) {
                Some(mr) if seen.insert(mr.identity()) => mrs.push(mr),
                Some(_) => incomplete = true,
                None => incomplete = true,
            }
        }
        if page.terminal_known && page.next.is_none() {
            complete = true;
            break;
        }
        if let Some(next) = page.next {
            if next <= page_number {
                incomplete = true;
                break;
            }
            page_number = next;
        } else if raw_len < PAGE_SIZE {
            // A short page without pagination headers suggests completion,
            // but does not prove it. Preserve rows with unknown coverage.
            break;
        } else {
            page_number += 1;
        }
    }
    if page_number > MAX_PAGES {
        incomplete = true;
    }
    if total.is_some_and(|n| n < mrs.len() as u64) {
        incomplete = true;
        // The header contradicts the rows already received. It cannot be
        // presented as a measured total or even a useful lower bound.
        total = None;
    }
    let coverage = if incomplete || (total.is_some_and(|n| n > mrs.len() as u64)) {
        Coverage::Partial { total }
    } else if complete {
        Coverage::Complete
    } else {
        Coverage::Unknown
    };
    Ok(FetchedList {
        scan: None,
        viewer: None,
        mrs,
        total,
        coverage,
    })
}

async fn request(
    program: &Path,
    host: &str,
    endpoint: &str,
    timeout: Duration,
) -> Result<Page, QueueError> {
    let mut command =
        super::host::constrained_command(program, host).map_err(|_| QueueError::UnsupportedHost)?;
    command
        .args(["api", "--hostname", host, "-i", endpoint])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let (raw, status) = tokio::time::timeout(timeout, super::transport::output(&mut command))
        .await
        .map_err(|_| QueueError::Timeout)?
        .map_err(|error| match error {
            super::transport::Error::TooLarge => QueueError::InvalidPage,
            super::transport::Error::Io => QueueError::Request,
        })?;
    parse_page(&raw, status.success())
}

fn parse_page(raw: &[u8], successful: bool) -> Result<Page, QueueError> {
    let unreadable = if successful {
        QueueError::InvalidPage
    } else {
        QueueError::Request
    };
    let text = std::str::from_utf8(raw).map_err(|_| unreadable)?;
    let normalized = text.replace("\r\n", "\n");
    // glab can fail before it prints an HTTP response. In that case the
    // transport failed, rather than GitLab returning malformed JSON.
    let explicit_status = normalized
        .lines()
        .filter(|line| line.starts_with("HTTP/"))
        .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u16>().ok())
        .next_back();
    match explicit_status {
        Some(401) => return Err(QueueError::Unauthorized),
        Some(403) => return Err(QueueError::Forbidden),
        Some(429) => return Err(QueueError::RateLimited),
        None if !successful => return Err(QueueError::Request),
        _ => {}
    }
    let (headers, body) = normalized.rsplit_once("\n\n").ok_or(if successful {
        QueueError::InvalidPage
    } else {
        QueueError::Request
    })?;
    let headers = headers
        .rsplit_once("\n\n")
        .map_or(headers, |(_, last)| last);
    let mut lines = headers.lines();
    let status = lines
        .next()
        .filter(|line| line.starts_with("HTTP/"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|n| n.parse::<u16>().ok())
        .ok_or(QueueError::InvalidPage)?;
    if !successful || !(200..300).contains(&status) {
        return Err(QueueError::Request);
    }
    let mut total = None;
    let mut next = None;
    let mut next_seen = false;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            if key.eq_ignore_ascii_case("x-total") {
                total = value.parse().ok();
            }
            if key.eq_ignore_ascii_case("x-next-page") {
                next_seen = true;
                if !value.is_empty() {
                    next = Some(value.parse().map_err(|_| QueueError::InvalidPage)?);
                }
            }
        }
    }
    let rows =
        serde_json::from_str::<Vec<Value>>(body.trim()).map_err(|_| QueueError::InvalidPage)?;
    Ok(Page {
        rows,
        total,
        next,
        terminal_known: next_seen,
        confirmation_terminal: confirmation_terminal(headers),
    })
}

/// Stronger than normal traversal metadata: only an unambiguous first-page
/// terminal envelope may support exact-IID absence. Missing totals remain unknown.
fn confirmation_terminal(headers: &str) -> bool {
    let mut evidence = std::collections::HashMap::new();
    for line in headers.lines().skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            return false;
        };
        if key.trim() != key {
            return false;
        }
        let key = key.to_ascii_lowercase();
        if !matches!(
            key.as_str(),
            "x-total"
                | "x-total-pages"
                | "x-page"
                | "x-next-page"
                | "x-prev-page"
                | "x-per-page"
                | "link"
        ) {
            continue;
        }
        let value = value.trim();
        if value.len() > 4096 {
            return false;
        }
        if evidence.insert(key, value).is_some_and(|old| old != value) {
            return false;
        }
    }
    let number = |value: &str| -> Option<u64> {
        (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    };
    let Some(total) = evidence.get("x-total").and_then(|s| number(s)) else {
        return false;
    };
    if evidence.get("x-next-page") != Some(&"") {
        return false;
    }
    for (key, value) in &evidence {
        let valid = match key.as_str() {
            "x-page" => number(value) == Some(1),
            "x-total-pages" => number(value).is_some_and(|n| n == 1 || (n == 0 && total == 0)),
            "x-prev-page" => value.is_empty(),
            "x-per-page" => number(value).is_some_and(|n| (1..=25).contains(&n)),
            "link" => value.split(',').all(|link| {
                let Some((url, attributes)) = link.trim().split_once('>') else {
                    return false;
                };
                let Some(url) = url
                    .strip_prefix('<')
                    .and_then(|s| reqwest::Url::parse(s).ok())
                else {
                    return false;
                };
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return false;
                }
                let pages: Vec<_> = url.query_pairs().filter(|(key, _)| key == "page").collect();
                if pages.len() != 1 || pages[0].1 != "1" {
                    return false;
                }
                let Some(relation) = attributes
                    .trim()
                    .strip_prefix(';')
                    .map(str::trim)
                    .and_then(|s| s.strip_prefix("rel="))
                else {
                    return false;
                };
                let relation = if relation.starts_with('"') {
                    let Some(relation) =
                        relation.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
                    else {
                        return false;
                    };
                    relation
                } else {
                    relation
                };
                matches!(relation, "first" | "last" | "self")
            }),
            _ => true,
        };
        if !valid {
            return false;
        }
    }
    true
}

fn map_row(v: &Value, source: &Source) -> Option<MergeRequest> {
    let number = v.get("iid")?.as_u64()?;
    let url = v.get("web_url")?.as_str()?;
    let prefix = format!("https://{}/", source.host);
    let path = url.strip_prefix(&prefix)?;
    let suffix = format!("/-/merge_requests/{number}");
    let repo = path.strip_suffix(&suffix)?;
    if repo.is_empty()
        || repo
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    let labels = v
        .get("labels")?
        .as_array()?
        .iter()
        .map(|label| {
            Some(Label {
                name: label.as_str()?.to_owned(),
                color: String::new(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let reviewers = names(v.get("reviewers")?)?;
    let assignees = names(v.get("assignees")?)?;
    let created_at = timestamp(v.get("created_at")?)?;
    let updated_at = timestamp(v.get("updated_at")?)?;
    let is_draft = v.get("draft")?.as_bool()?;
    let comment_count = v.get("user_notes_count")?.as_u64()?;
    let id = v.get("id")?.as_u64()?;
    Some(MergeRequest {
        observation: None,
        viewer: None,
        source: source.clone(),
        id,
        number,
        title: v.get("title")?.as_str()?.to_owned(),
        url: url.to_owned(),
        repo: repo.to_owned(),
        author: v.get("author")?.get("username")?.as_str()?.to_owned(),
        is_draft,
        head_ref: v.get("source_branch")?.as_str()?.to_owned(),
        head_oid: v.get("sha").and_then(Value::as_str).map(str::to_owned),
        base_ref: v.get("target_branch")?.as_str()?.to_owned(),
        created_at,
        updated_at,
        labels,
        comment_count,
        reviewers,
        assignees,
        detailed_merge_status: v
            .get("detailed_merge_status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ci: None,
        unresolved_threads: None,
        unresolved_threads_floor: true,
        needs_my_review: None,
        in_merge_queue: None,
        can_enqueue_train: None,
        review: None,
    })
}

fn names(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|person| Some(person.get("username")?.as_str()?.to_owned()))
        .collect()
}

fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(host: &str) -> Source {
        Source {
            provider: Provider::Gitlab,
            host: host.into(),
        }
    }

    fn row(host: &str, repo: &str, iid: u64) -> Value {
        json!({
            "id": 481, "iid": iid, "title": "Synthetic change",
            "web_url": format!("https://{host}/{repo}/-/merge_requests/{iid}"),
            "author": {"username": "fixture"}, "draft": true,
            "source_branch": "topic", "target_branch": "main",
            "created_at": "2026-09-01T10:00:00Z", "updated_at": "2026-09-02T10:00:00Z",
            "labels": ["area::ui"], "reviewers": [{"username": "reviewer"}],
            "assignees": [], "user_notes_count": 2,
            "detailed_merge_status": "draft_status", "sha": "deadbeef"
        })
    }

    #[test]
    fn unread_mr_head_preserves_confirmed_review_until_a_real_change() {
        use crate::inventory::{
            gitlab_observation, reconcile, ConfirmedReview, ObservationState, ReadinessField,
        };
        let source = Source {
            provider: crate::identity::Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let mut raw = row("gitlab.com", "group/project", 7);
        raw["sha"] = json!("head-a");
        let mut old = map_row(&raw, &source).unwrap();
        old.observation = Some(gitlab_observation(&old));
        old.needs_my_review = Some(false);
        old.observation.as_mut().unwrap().confirmed_review = Some(ConfirmedReview {
            head_oid: "head-a".into(),
            review: crate::github::model::ReviewState::Approved,
            confirmed_at: chrono::Utc::now(),
            receipt: None,
            unresolved: false,
            confirmed_by_read: false,
        });
        for head in [None, Some(Value::Null), Some(json!(""))] {
            let mut unread = raw.clone();
            if let Some(head) = head {
                unread["sha"] = head;
            } else {
                unread.as_object_mut().unwrap().remove("sha");
            }
            let mapped = map_row(&unread, &source).unwrap();
            let fresh = reconcile(vec![], vec![mapped.clone()], false, chrono::Utc::now());
            assert!(fresh[0]
                .observation
                .as_ref()
                .unwrap()
                .unknown_fields
                .contains(&ReadinessField::Head));
            let retained = reconcile(vec![old.clone()], vec![mapped], false, chrono::Utc::now());
            assert_eq!(retained[0].head_oid.as_deref(), Some("head-a"));
            assert_eq!(
                retained[0].observation.as_ref().unwrap().state,
                ObservationState::Retained
            );
            assert_eq!(retained[0].needs_my_review, Some(false));
            let mut lagging = map_row(&raw, &source).unwrap();
            lagging.needs_my_review = Some(true);
            let same = reconcile(retained, vec![lagging.clone()], true, chrono::Utc::now());
            assert!(same[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_some());
            assert_eq!(same[0].needs_my_review, Some(false));
            lagging.head_oid = Some("head-b".into());
            let changed = reconcile(same, vec![lagging], true, chrono::Utc::now());
            assert!(changed[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_none());
            assert_eq!(changed[0].needs_my_review, Some(true));
        }
    }

    #[test]
    fn nested_paths_and_hosts_remain_distinct() {
        let first = map_row(
            &row("gitlab.com", "group/subgroup/project", 7),
            &source("gitlab.com"),
        )
        .unwrap();
        let second = map_row(
            &row("gitlab.example", "group/subgroup/project", 7),
            &source("gitlab.example"),
        )
        .unwrap();
        assert_ne!(first.identity(), second.identity());
        assert_eq!(first.repo, "group/subgroup/project");
        assert_eq!(first.number, 7);
        assert_eq!(first.labels[0].name, "area::ui");
        assert_eq!(first.comment_count, 2);
        assert_eq!(first.reviewers, vec!["reviewer"]);
        assert_eq!(first.ci, None);
        assert_eq!(first.unresolved_threads, None);
        assert_eq!(first.review, None);
        assert!(map_row(
            &row("gitlab.example", "group/subgroup/project", 7),
            &source("gitlab.com"),
        )
        .is_none());
    }

    #[test]
    fn headers_control_total_and_terminal_state() {
        let body = json!([row("gitlab.com", "group/subgroup/project", 7)]);
        let raw = format!("HTTP/2 200\r\nx-total: 2\r\nx-next-page: 2\r\n\r\n{body}");
        let page = parse_page(raw.as_bytes(), true).unwrap();
        assert_eq!(page.total, Some(2));
        assert_eq!(page.next, Some(2));
        assert_eq!(page.rows.len(), 1);
        assert!(page.terminal_known);
        let empty =
            parse_page(b"HTTP/2 200\r\nx-total: 0\r\nx-next-page:\r\n\r\n[]", true).unwrap();
        assert_eq!(empty.total, Some(0));
        assert!(empty.terminal_known);
        assert!(empty.rows.is_empty());
        assert!(matches!(
            parse_page(b"HTTP/2 401\n\n{}", false),
            Err(QueueError::Unauthorized)
        ));
        assert!(matches!(
            parse_page(b"HTTP/2 403\n\n{}", false),
            Err(QueueError::Forbidden)
        ));
        assert!(matches!(
            parse_page(b"HTTP/2 429\n\n{}", false),
            Err(QueueError::RateLimited)
        ));
        assert!(matches!(parse_page(b"", false), Err(QueueError::Request)));
        assert!(matches!(
            parse_page(b"not HTTP", false),
            Err(QueueError::Request)
        ));
        let no_headers = parse_page(b"HTTP/2 200\n\n[]", true).unwrap();
        assert_eq!(no_headers.total, None);
        assert!(!no_headers.terminal_known);
        assert!(matches!(
            parse_page(b"HTTP/2 200\nx-next-page: bad\n\n[]", true),
            Err(QueueError::InvalidPage)
        ));
    }

    #[test]
    fn missing_required_fields_are_withheld_instead_of_zeroed() {
        let mut missing = row("gitlab.com", "group/subgroup/project", 7);
        missing.as_object_mut().unwrap().remove("user_notes_count");
        assert!(map_row(&missing, &source("gitlab.com")).is_none());
        let mut wrong_path = row("gitlab.com", "group/subgroup/project", 7);
        wrong_path["web_url"] =
            json!("https://gitlab.com/group/subgroup/project/-/merge_requests/8");
        assert!(map_row(&wrong_path, &source("gitlab.com")).is_none());
    }

    #[cfg(unix)]
    async fn scripted(
        script: &str,
        list: CachedList,
        budget: Duration,
    ) -> Result<FetchedList, QueueError> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(
            &program,
            fetch_with_program(&program, &source("gitlab.com"), list, budget),
        )
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pages_both_scopes_and_preserves_rows_after_later_timeout() {
        let first = json!([row("gitlab.com", "group/subgroup/project", 7)]);
        let script = format!(
            r#"
test "$1" = api && test "$2" = --hostname && test "$3" = gitlab.com && test "$4" = -i || exit 90
case "$5" in
  *scope=created_by_me*page=1) printf 'HTTP/2 200\nx-total: 2\nx-next-page: 2\n\n%s' '{first}' ;;
  *scope=created_by_me*page=2) touch "$0.timeout"; exec sleep 60 ;;
  *scope=reviews_for_me*) printf 'HTTP/2 200\nx-total: 0\nx-next-page:\n\n[]' ;;
  *) exit 91 ;;
esac
"#
        );
        let result = scripted(&script, CachedList::Authored, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(result.mrs.len(), 1);
        assert_eq!(result.total, Some(2));
        assert_eq!(result.coverage, Coverage::Partial { total: Some(2) });
        let reviewing = scripted(&script, CachedList::Reviewing, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(reviewing.mrs.is_empty());
        assert_eq!(reviewing.total, Some(0));
        assert_eq!(reviewing.coverage, Coverage::Complete);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_first_page_is_not_a_measured_empty_queue() {
        let result = scripted("exit 1", CachedList::Authored, Duration::from_secs(1)).await;
        assert!(matches!(result, Err(QueueError::Request)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_total_smaller_than_received_rows_is_withheld() {
        let rows = json!([
            row("gitlab.com", "group/subgroup/project", 7),
            row("gitlab.com", "group/subgroup/project", 8)
        ]);
        let script = format!("printf 'HTTP/2 200\\nx-total: 1\\nx-next-page:\\n\\n%s' '{rows}'");
        let result = scripted(&script, CachedList::Authored, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result.mrs.len(), 2);
        assert_eq!(result.total, None);
        assert_eq!(result.coverage, Coverage::Partial { total: None });
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_empty_first_page_with_a_reported_total_is_still_partial_evidence() {
        let script = r#"
case "$5" in
  *page=1) printf 'HTTP/2 200\nx-total: 2\nx-next-page: 2\n\n[]' ;;
  *page=2) touch "$0.timeout"; exec sleep 60 ;;
  *) exit 91 ;;
esac
"#;
        let result = scripted(script, CachedList::Authored, Duration::from_millis(500))
            .await
            .unwrap();
        assert!(result.mrs.is_empty());
        assert_eq!(result.total, Some(2));
        assert_eq!(result.coverage, Coverage::Partial { total: Some(2) });
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_valid_page_is_rejected_before_retention() {
        let mut large = row("gitlab.com", "group/subgroup/project", 7);
        large["description"] = json!("x".repeat(4 * 1024 * 1024));
        let script = format!(
            "printf 'HTTP/2 200\\nx-next-page:\\n\\n%s' '{}'",
            json!([large])
        );
        assert!(matches!(
            scripted(&script, CachedList::Authored, Duration::from_secs(30)).await,
            Err(QueueError::InvalidPage)
        ));
    }
}
