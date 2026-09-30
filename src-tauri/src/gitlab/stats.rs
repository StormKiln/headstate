//! GitLab-only created-cohort statistics. Bounds and coverage are part of the
//! answer, including after persistence. A reviewer assignment is not a review.
mod activity;
mod history;
mod review_evidence;

use crate::identity::{Provider, Source};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    process::Stdio,
    time::Duration,
};

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const BUDGET: Duration = Duration::from_secs(45);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Scope {
    Mine,
    Project(String),
    Group(String),
    Person(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    Complete,
    PageLimit,
    Timeout,
    RateLimited,
    Unauthorized,
    Forbidden,
    RequestFailed,
    InvalidData,
    UnknownPagination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage {
    pub complete: bool,
    pub stop: Stop,
    pub pages: usize,
    pub received: usize,
    pub total: Option<u64>,
    pub rate_remaining: Option<u64>,
    pub rate_reset: Option<u64>,
}

impl Coverage {
    fn empty(stop: Stop) -> Self {
        Self {
            complete: false,
            stop,
            pages: 0,
            received: 0,
            total: None,
            rate_remaining: None,
            rate_reset: None,
        }
    }

    fn rate_limited(&self) -> bool {
        self.stop == Stop::RateLimited || self.rate_remaining == Some(0)
    }
    fn invalidate(&mut self) {
        self.complete = false;
        // Invalid rows do not revoke the server's instruction to stop requests.
        if self.stop != Stop::RateLimited {
            self.stop = Stop::InvalidData;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub path: String,
    pub namespace: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    #[serde(default)]
    pub account: Option<String>,
    pub source: Source,
    pub viewer: String,
    pub projects: Vec<Project>,
    pub groups: Vec<String>,
    pub group_coverage: Option<Coverage>,
    pub group_error: Option<String>,
    pub coverage: Coverage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub source: Source,
    pub project: String,
    pub iid: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    /// Missing reviewer arrays stay unknown, including on the leaderboard.
    pub reviewers: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counts {
    pub created: usize,
    pub merged: usize,
    pub closed: usize,
    pub opened: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Day {
    pub day: String,
    pub created: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub username: String,
    pub created: usize,
    pub merged: usize,
    pub mean_merge_hours: Option<f64>,
    pub timed_merges: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reviewer {
    pub username: String,
    pub assigned: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub source: Source,
    pub viewer: String,
    pub scope: Scope,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub coverage: Coverage,
    pub counts: Counts,
    pub series: Vec<Day>,
    pub authors: Vec<Author>,
    pub reviewers: Vec<Reviewer>,
    pub reviewer_rows_measured: usize,
    pub review_activity: Option<usize>,
    pub history: Vec<Record>,
    #[serde(default)]
    pub merged_window: Option<MergedWindow>,
    #[serde(default)]
    pub merged_error: Option<String>,
    #[serde(default)]
    pub activity: Option<activity::Activity>,
    #[serde(default)]
    pub review_evidence: Option<review_evidence::ReviewEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedWindow {
    pub fetched_at: DateTime<Utc>,
    pub coverage: Coverage,
    pub count: usize,
    pub series: Vec<MergedDay>,
    pub authors: Vec<MergedAuthor>,
    pub history: Vec<Record>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedDay {
    pub day: String,
    pub merged: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedAuthor {
    pub username: String,
    pub merged: usize,
    pub mean_merge_hours: Option<f64>,
}

fn source(host: &str) -> Result<Source, String> {
    let host = super::host::validate(host).map_err(|error| error.to_string())?;
    Ok(Source {
        provider: Provider::Gitlab,
        host,
    })
}
fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}
fn encode(path: &str) -> String {
    path.replace('/', "%2F")
}
fn endpoint(scope: &Scope) -> Result<String, String> {
    match scope {
        Scope::Mine => Ok("merge_requests?scope=created_by_me".into()),
        Scope::Project(path) if path_ok(path) => Ok(format!(
            "projects/{}/merge_requests?scope=all",
            encode(path)
        )),
        Scope::Group(path) if path_ok(path) => {
            Ok(format!("groups/{}/merge_requests?scope=all", encode(path)))
        }
        Scope::Person(username) if path_ok(username) && !username.contains('/') => Ok(format!(
            "merge_requests?scope=all&author_username={username}"
        )),
        _ => Err("invalid GitLab project, group or username".into()),
    }
}
struct Response {
    bytes: usize,
    body: Value,
    next: Option<usize>,
    terminal: bool,
    total: Option<u64>,
    remaining: Option<u64>,
    reset: Option<u64>,
}
#[derive(Debug)]
struct RequestFailure {
    stop: Stop,
    remaining: Option<u64>,
    reset: Option<u64>,
}
impl From<Stop> for RequestFailure {
    fn from(stop: Stop) -> Self {
        Self {
            stop,
            remaining: None,
            reset: None,
        }
    }
}
fn parse(raw: &[u8], success: bool) -> Result<Response, RequestFailure> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| Stop::InvalidData)?
        .replace("\r\n", "\n");
    let (headers, body) = text.split_once("\n\n").ok_or(Stop::RequestFailed)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(Stop::InvalidData)?;
    // Failure responses carry the newest rate-limit evidence too. Read these
    // headers before interpreting the status or attempting to decode its body.
    let mut remaining = None;
    let mut reset = None;
    for line in headers.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            match key.to_ascii_lowercase().as_str() {
                "ratelimit-remaining" => remaining = value.trim().parse().ok(),
                "ratelimit-reset" => reset = value.trim().parse().ok(),
                _ => {}
            }
        }
    }
    let failure = match status {
        401 => Some(Stop::Unauthorized),
        403 => Some(Stop::Forbidden),
        429 => Some(Stop::RateLimited),
        200..=299 if success => None,
        _ => Some(Stop::RequestFailed),
    };
    if let Some(stop) = failure {
        return Err(RequestFailure {
            stop,
            remaining,
            reset,
        });
    }
    let mut out = Response {
        bytes: raw.len(),
        body: serde_json::from_str(body).map_err(|_| Stop::InvalidData)?,
        next: None,
        terminal: false,
        total: None,
        remaining,
        reset,
    };
    for line in headers.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            match key.to_ascii_lowercase().as_str() {
                "x-next-page" => {
                    out.terminal = true;
                    if !value.is_empty() {
                        out.next = Some(value.parse().map_err(|_| Stop::InvalidData)?);
                    }
                }
                "x-total" => out.total = value.parse().ok(),
                "ratelimit-remaining" => out.remaining = value.parse().ok(),
                "ratelimit-reset" => out.reset = value.parse().ok(),
                _ => {}
            }
        }
    }
    Ok(out)
}
async fn request(
    program: &Path,
    host: &str,
    endpoint: &str,
    budget: Duration,
) -> Result<Response, RequestFailure> {
    let mut command =
        super::host::constrained_command(program, host).map_err(|_| Stop::InvalidData)?;
    command
        .args(["api", "--hostname", host, "-i", endpoint])
        .stdin(Stdio::null());
    let (raw, status) = tokio::time::timeout(
        budget.min(REQUEST_TIMEOUT),
        super::transport::output(&mut command),
    )
    .await
    .map_err(|_| Stop::Timeout)?
    .map_err(|error| match error {
        super::transport::Error::TooLarge => Stop::InvalidData,
        super::transport::Error::Io => Stop::RequestFailed,
    })?;
    parse(&raw, status.success())
}
fn error(stop: &Stop) -> String {
    match stop {
        Stop::Unauthorized => "GitLab authentication expired; sign in again on the desktop",
        Stop::Forbidden => "GitLab denied access to this statistics scope",
        Stop::RateLimited => "GitLab rate limit reached; try again later",
        Stop::Timeout => "GitLab statistics request timed out",
        Stop::InvalidData => "GitLab returned unreadable statistics",
        _ => "GitLab statistics request failed",
    }
    .into()
}
async fn viewer(program: &Path, host: &str) -> Result<String, String> {
    let out = request(program, host, "user", REQUEST_TIMEOUT)
        .await
        .map_err(|failure| error(&failure.stop))?;
    // Numeric id is stable across username changes and isolates accounts.
    out.body
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        .ok_or_else(|| error(&Stop::InvalidData))
}
async fn pages(
    program: &Path,
    host: &str,
    base: &str,
    budget: Duration,
) -> Result<(Vec<Value>, Coverage), RequestFailure> {
    pages_limited(program, host, base, budget, MAX_PAGES).await
}
async fn pages_limited(
    program: &Path,
    host: &str,
    base: &str,
    budget: Duration,
    max_pages: usize,
) -> Result<(Vec<Value>, Coverage), RequestFailure> {
    let started = tokio::time::Instant::now();
    let mut rows = Vec::new();
    let mut retained_bytes = 0;
    let mut page_number = 1;
    let mut total_conflict = false;
    let separator = if base.contains('?') { '&' } else { '?' };
    let mut coverage = Coverage::empty(Stop::PageLimit);
    for _ in 0..max_pages {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            coverage.stop = Stop::Timeout;
            break;
        }
        let out = match request(
            program,
            host,
            &format!("{base}{separator}per_page={PAGE_SIZE}&page={page_number}"),
            remaining,
        )
        .await
        {
            Ok(out) => out,
            Err(failure) if coverage.pages == 0 => return Err(failure),
            Err(failure) => {
                if failure.stop == Stop::RateLimited {
                    // Missing headers on a 429 invalidate prior positive rate
                    // readings; absence is unknown, never an invented zero.
                    coverage.rate_remaining = failure.remaining;
                    coverage.rate_reset = failure.reset;
                }
                coverage.stop = failure.stop;
                break;
            }
        };
        let Value::Array(page_rows) = out.body else {
            if coverage.pages == 0 {
                return Err(Stop::InvalidData.into());
            }
            coverage.stop = Stop::InvalidData;
            break;
        };
        retained_bytes += out.bytes;
        if retained_bytes > 16 * 1024 * 1024 || page_rows.len() > PAGE_SIZE {
            if coverage.pages == 0 {
                return Err(Stop::InvalidData.into());
            }
            coverage.stop = Stop::InvalidData;
            break;
        }
        coverage.pages += 1;
        coverage.rate_remaining = out.remaining;
        coverage.rate_reset = out.reset;
        if let Some(total) = out.total {
            if coverage.total.is_some_and(|prior| prior != total) {
                total_conflict = true;
            }
            coverage.total = if total_conflict { None } else { Some(total) };
        }
        let page_len = page_rows.len();
        rows.extend(page_rows);
        if out.terminal && out.next.is_none() {
            coverage.complete = true;
            coverage.stop = Stop::Complete;
            break;
        }
        if out.remaining == Some(0) {
            coverage.stop = Stop::RateLimited;
            break;
        }
        match out.next {
            Some(next) if next == page_number + 1 => page_number = next,
            Some(_) => {
                coverage.stop = Stop::InvalidData;
                break;
            }
            None if page_len < PAGE_SIZE => {
                coverage.stop = Stop::UnknownPagination;
                break;
            }
            None => page_number += 1,
        }
    }
    coverage.received = rows.len();
    if total_conflict
        || coverage
            .total
            .is_some_and(|n| n != rows.len() as u64 && coverage.complete || n < rows.len() as u64)
    {
        coverage.invalidate();
        coverage.total = None;
    }
    Ok((rows, coverage))
}

pub async fn tree(host: &str) -> Result<Tree, String> {
    let source = source(host)?;
    let program =
        super::auth::find_glab().ok_or("GitLab CLI (glab) was not found on the desktop")?;
    let viewer = viewer(&program, host).await?;
    let account = super::auth::viewer(&program, host)
        .await
        .map_err(|_| "GitLab account unavailable")?;
    static READS: std::sync::LazyLock<super::coalesce::Reads<Result<Tree, String>>> =
        std::sync::LazyLock::new(super::coalesce::Reads::default);
    let mut tree = READS
        .run(
            format!("{host}:{viewer}"),
            discover_tree(&program, source, viewer, BUDGET),
        )
        .await?;
    if super::auth::viewer(&program, host)
        .await
        .map_err(|_| "GitLab account unavailable")?
        != account
    {
        return Err("GitLab account changed while loading scopes. Refresh to try again.".into());
    }
    tree.account = Some(account);
    Ok(tree)
}

async fn discover_tree(
    program: &Path,
    source: Source,
    viewer: String,
    budget: Duration,
) -> Result<Tree, String> {
    let started = tokio::time::Instant::now();
    let (rows, mut coverage) = match pages(
        program,
        &source.host,
        "projects?membership=true&simple=true&order_by=id&sort=asc",
        budget,
    )
    .await
    {
        Ok(result) => result,
        // Project discovery is optional for the personal scope. A timeout
        // before page one means unknown scopes, not an empty project list.
        Err(failure) if failure.stop == Stop::Timeout => {
            (Vec::new(), Coverage::empty(Stop::Timeout))
        }
        Err(failure) => return Err(error(&failure.stop)),
    };
    let mut projects = BTreeMap::new();
    for row in rows {
        if let Some(path) = row
            .get("path_with_namespace")
            .and_then(Value::as_str)
            .filter(|p| path_ok(p))
        {
            let namespace =
                if row.pointer("/namespace/kind").and_then(Value::as_str) == Some("group") {
                    path.rsplit_once('/')
                        .map(|(ns, _)| ns)
                        .unwrap_or("")
                        .to_owned()
                } else {
                    String::new()
                };
            if projects
                .insert(
                    path.to_owned(),
                    Project {
                        path: path.into(),
                        namespace,
                    },
                )
                .is_none()
            {
                continue;
            }
        }
        coverage.invalidate();
    }
    coverage.received = projects.len();
    let mut groups = Vec::new();
    let mut group_coverage = None;
    let mut group_error = None;
    if coverage.rate_limited() || started.elapsed() >= budget {
        group_error = Some("Request budget exhausted before requesting groups".into());
    } else {
        match pages(
            program,
            &source.host,
            // all_available=true scans every accessible group on GitLab.com;
            // it can return HTTP 500 after our request deadline. Member groups
            // are the scopes this account can act on, and project namespaces
            // still supply groups reached through project membership.
            "groups?all_available=false&order_by=id&sort=asc",
            budget.saturating_sub(started.elapsed()),
        )
        .await
        {
            Ok((rows, mut measured)) => {
                let mut seen = HashSet::new();
                for row in rows {
                    if let Some(path) = row
                        .get("full_path")
                        .and_then(Value::as_str)
                        .filter(|p| path_ok(p))
                    {
                        if seen.insert(path.to_owned()) {
                            groups.push(path.to_owned());
                            continue;
                        }
                    }
                    measured.invalidate();
                }
                measured.received = groups.len();
                groups.sort();
                group_coverage = Some(measured);
            }
            Err(failure) => group_error = Some(error(&failure.stop)),
        }
    }
    Ok(Tree {
        account: None,
        source,
        viewer,
        projects: projects.into_values().collect(),
        groups,
        group_coverage,
        group_error,
        coverage,
    })
}
fn timestamp(row: &Value, field: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(row.get(field)?.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn record(row: &Value, source: &Source) -> Option<Record> {
    let iid = row.get("iid")?.as_u64()?;
    let url = row.get("web_url")?.as_str()?;
    let prefix = format!("https://{}/", source.host);
    let suffix = format!("/-/merge_requests/{iid}");
    let project = url.strip_prefix(&prefix)?.strip_suffix(&suffix)?;
    if !path_ok(project) {
        return None;
    }
    let state = row.get("state")?.as_str()?;
    if !["opened", "closed", "merged"].contains(&state) {
        return None;
    }
    let created_at = timestamp(row, "created_at")?;
    let merged_at = timestamp(row, "merged_at").filter(|at| *at >= created_at);
    let reviewers = row
        .get("reviewers")
        .and_then(Value::as_array)
        .and_then(|rs| {
            rs.iter()
                .map(|r| r.get("username")?.as_str().map(str::to_owned))
                .collect()
        });
    Some(Record {
        source: source.clone(),
        project: project.into(),
        iid,
        url: url.into(),
        title: row.get("title")?.as_str()?.into(),
        author: row.get("author")?.get("username")?.as_str()?.into(),
        state: state.into(),
        created_at,
        merged_at,
        reviewers,
    })
}
fn summarize(
    source: Source,
    viewer: String,
    scope: Scope,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    raw: Vec<Value>,
    mut coverage: Coverage,
) -> Report {
    let mut seen = HashSet::new();
    let mut history = Vec::new();
    for row in raw {
        if let Some(record) = record(&row, &source) {
            let in_scope = match &scope {
                Scope::Mine => true,
                Scope::Person(username) => &record.author == username,
                Scope::Project(p) => &record.project == p,
                Scope::Group(g) => record.project.starts_with(&format!("{g}/")),
            };
            if in_scope
                && record.created_at >= start
                && record.created_at <= end
                && seen.insert((record.project.clone(), record.iid))
            {
                history.push(record);
                continue;
            }
        }
        coverage.invalidate();
    }
    coverage.received = history.len();
    let mut counts = Counts {
        created: history.len(),
        merged: 0,
        closed: 0,
        opened: 0,
    };
    let mut series = BTreeMap::<String, usize>::new();
    // Empty days only represent measured zeros when coverage is complete.
    let mut day = start.date_naive();
    while day <= end.date_naive() {
        series.insert(day.to_string(), 0);
        day = day.succ_opt().expect("bounded dates");
    }
    let mut authors = BTreeMap::<String, (usize, usize, Vec<f64>)>::new();
    let mut reviewers = BTreeMap::<String, usize>::new();
    let mut reviewer_rows_measured = 0;
    for row in &history {
        *series
            .entry(row.created_at.date_naive().to_string())
            .or_default() += 1;
        let author = authors.entry(row.author.clone()).or_default();
        author.0 += 1;
        match row.state.as_str() {
            "merged" => {
                counts.merged += 1;
                author.1 += 1;
                if let Some(at) = row.merged_at {
                    author
                        .2
                        .push((at - row.created_at).num_seconds() as f64 / 3600.0);
                }
            }
            "closed" => counts.closed += 1,
            "opened" => counts.opened += 1,
            _ => {}
        }
        if let Some(assigned) = &row.reviewers {
            reviewer_rows_measured += 1;
            for username in assigned.iter().collect::<HashSet<_>>() {
                *reviewers.entry(username.clone()).or_default() += 1;
            }
        }
    }
    let complete = coverage.complete;
    Report {
        source,
        viewer,
        scope,
        start,
        end,
        fetched_at: Utc::now(),
        coverage,
        counts,
        series: series
            .into_iter()
            .map(|(day, created)| Day { day, created })
            .collect(),
        authors: authors
            .into_iter()
            .map(|(username, (created, merged, times))| Author {
                username,
                created,
                merged,
                timed_merges: times.len(),
                mean_merge_hours: (complete && times.len() == merged && !times.is_empty())
                    .then(|| times.iter().sum::<f64>() / times.len() as f64),
            })
            .collect(),
        reviewers: reviewers
            .into_iter()
            .map(|(username, assigned)| Reviewer { username, assigned })
            .collect(),
        reviewer_rows_measured,
        review_activity: None,
        history,
        merged_window: None,
        merged_error: None,
        activity: None,
        review_evidence: None,
    }
}

static CACHE_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CACHE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub async fn invalidate(host: String, db: std::path::PathBuf) {
    CACHE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = CACHE_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        crate::store::gitlab_stats::invalidate_host(&db, &host)
    })
    .await;
}

pub async fn load(
    host: &str,
    scope: Scope,
    days: u32,
    db: std::path::PathBuf,
    refresh: bool,
) -> Result<Report, String> {
    let generation = CACHE_GENERATION.load(std::sync::atomic::Ordering::Acquire);
    let source = source(host)?;
    endpoint(&scope)?;
    if !(1..=90).contains(&days) {
        return Err("GitLab statistics window must be 1 to 90 days".into());
    }
    let program =
        super::auth::find_glab().ok_or("GitLab CLI (glab) was not found on the desktop")?;
    let viewer = viewer(&program, host).await?;
    // Whole UTC dates give a stable cache window; today's cohort is still live.
    let end = Utc::now();
    let start = (end.date_naive() - chrono::Duration::days(i64::from(days) - 1))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let key = serde_json::to_string(&(
        "gitlab",
        host,
        &viewer,
        &scope,
        start.date_naive(),
        end.date_naive(),
    ))
    .map_err(|e| e.to_string())?;
    if !refresh {
        let path = db.clone();
        let cache_key = key.clone();
        let cached =
            tokio::task::spawn_blocking(move || crate::store::gitlab_stats::get(&path, &cache_key))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
        if let Some(report) = cached {
            return Ok(report);
        }
    }
    static READS: std::sync::LazyLock<super::coalesce::Reads<Result<Report, String>>> =
        std::sync::LazyLock::new(super::coalesce::Reads::default);
    let report = READS
        .run(
            format!("{key}:{generation}"),
            load_window(&program, source, viewer.clone(), scope, start, end),
        )
        .await?;
    if self::viewer(&program, host).await? != viewer {
        return Err(
            "GitLab account changed while loading statistics. Refresh to try again.".into(),
        );
    }
    if generation != CACHE_GENERATION.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(report);
    }
    let saved = report.clone();
    // Cache trouble never discards a measured result.
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = CACHE_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        if generation != CACHE_GENERATION.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        crate::store::gitlab_stats::put(&db, &key, &saved)
    })
    .await;
    Ok(report)
}

async fn load_window(
    program: &Path,
    source: Source,
    account: String,
    scope: Scope,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Report, String> {
    let started = tokio::time::Instant::now();
    let base = endpoint(&scope)?;
    let dates = |prefix: &str| {
        format!(
            "{prefix}_after={}&{prefix}_before={}",
            start.to_rfc3339().replace('+', "%2B"),
            end.to_rfc3339().replace('+', "%2B")
        )
    };
    let created_url = format!(
        "{base}&state=all&order_by=created_at&sort=desc&{}",
        dates("created")
    );
    let (raw, coverage) = pages(program, &source.host, &created_url, BUDGET)
        .await
        .map_err(|failure| error(&failure.stop))?;
    let mut report = summarize(source.clone(), account, scope, start, end, raw, coverage);
    let merged_url = format!(
        "{base}&state=merged&order_by=merged_at&sort=desc&{}",
        dates("merged")
    );
    // Each subsequent operation receives only the remaining load budget. No
    // outer timeout can drop already measured rows.
    let mut limited = report.coverage.rate_limited();
    if limited {
        report.merged_error =
            Some("GitLab request budget exhausted; merged MRs were not requested".into());
    } else if started.elapsed() < BUDGET {
        match pages(
            program,
            &source.host,
            &merged_url,
            BUDGET.saturating_sub(started.elapsed()),
        )
        .await
        {
            Ok((raw, coverage)) => {
                report.merged_window = Some(summarize_merged(&report, raw, coverage))
            }
            Err(failure) => {
                if failure.stop == Stop::RateLimited {
                    limited = true;
                    report.coverage.rate_remaining = failure.remaining;
                    report.coverage.rate_reset = failure.reset;
                }
                report.merged_error = Some(error(&failure.stop));
            }
        }
    } else {
        report.merged_error = Some("Load budget exhausted before requesting merged MRs".into());
    }
    limited |= report
        .merged_window
        .as_ref()
        .is_some_and(|m| m.coverage.rate_limited());
    report.activity = Some(
        activity::load(
            program,
            &report,
            if limited {
                Duration::ZERO
            } else {
                BUDGET.saturating_sub(started.elapsed())
            },
        )
        .await,
    );
    limited |= report
        .activity
        .as_ref()
        .is_some_and(|a| a.rate_limited || a.rate_remaining == Some(0));
    report.review_evidence = Some(
        review_evidence::load(
            program,
            &report,
            if limited {
                Duration::ZERO
            } else {
                BUDGET.saturating_sub(started.elapsed())
            },
        )
        .await,
    );
    Ok(report)
}

fn summarize_merged(report: &Report, raw: Vec<Value>, mut coverage: Coverage) -> MergedWindow {
    let mut seen = HashSet::new();
    let mut history = Vec::new();
    for raw in raw {
        if let Some(row) = record(&raw, &report.source) {
            let in_scope = match &report.scope {
                Scope::Mine => true,
                Scope::Person(p) => &row.author == p,
                Scope::Project(p) => &row.project == p,
                Scope::Group(g) => row.project.starts_with(&format!("{g}/")),
            };
            if in_scope
                && row.state == "merged"
                && row
                    .merged_at
                    .is_some_and(|at| at >= report.start && at <= report.end)
                && seen.insert((row.project.clone(), row.iid))
            {
                history.push(row);
                continue;
            }
        }
        coverage.invalidate();
    }
    coverage.received = history.len();
    let mut series = BTreeMap::<String, usize>::new();
    let mut date = report.start.date_naive();
    while date <= report.end.date_naive() {
        series.insert(date.to_string(), 0);
        date = date.succ_opt().expect("bounded dates");
    }
    let mut authors = BTreeMap::<String, Vec<f64>>::new();
    for row in &history {
        let at = row.merged_at.expect("validated above");
        *series.entry(at.date_naive().to_string()).or_default() += 1;
        authors
            .entry(row.author.clone())
            .or_default()
            .push((at - row.created_at).num_seconds() as f64 / 3600.0);
    }
    let complete = coverage.complete;
    MergedWindow {
        fetched_at: Utc::now(),
        coverage,
        count: history.len(),
        history,
        series: series
            .into_iter()
            .map(|(day, merged)| MergedDay { day, merged })
            .collect(),
        authors: authors
            .into_iter()
            .map(|(username, times)| MergedAuthor {
                username,
                merged: times.len(),
                mean_merge_hours: complete.then(|| times.iter().sum::<f64>() / times.len() as f64),
            })
            .collect(),
    }
}

pub use history::{backfill, Backfill, HistoryReceipt};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub(super) fn sample(iid: u64) -> Value {
        json!({"iid": iid, "web_url": format!("https://gitlab.com/g/sub/p/-/merge_requests/{iid}"), "title":"change", "author":{"username":"author"}, "state":"merged", "created_at":"2026-09-01T10:00:00Z", "merged_at":"2026-09-02T10:00:00Z", "reviewers":[{"username":"reviewer"}]})
    }
    pub(super) fn coverage() -> Coverage {
        Coverage {
            complete: true,
            stop: Stop::Complete,
            pages: 1,
            received: 1,
            total: Some(1),
            rate_remaining: None,
            rate_reset: None,
        }
    }
    pub(super) fn report(rows: Vec<Value>) -> Report {
        summarize(
            source("gitlab.com").unwrap(),
            "1".into(),
            Scope::Mine,
            "2026-09-01T00:00:00Z".parse().unwrap(),
            "2026-09-03T00:00:00Z".parse().unwrap(),
            rows,
            coverage(),
        )
    }
    #[test]
    fn merged_window_includes_old_creation_and_rejects_out_of_window_evidence() {
        let context = report(vec![]);
        let mut older = sample(1);
        older["created_at"] = json!("2026-08-01T10:00:00Z");
        let merged = summarize_merged(&context, vec![older.clone()], coverage());
        assert_eq!(merged.count, 1);
        assert_eq!(merged.series[1].merged, 1);
        assert_eq!(merged.authors[0].mean_merge_hours, Some(32.0 * 24.0));
        let mut outside = sample(2);
        outside["merged_at"] = json!("2026-09-05T10:00:00Z");
        let partial = summarize_merged(&context, vec![older, outside], coverage());
        assert_eq!(partial.count, 1);
        assert!(!partial.coverage.complete);
        assert_eq!(partial.authors[0].mean_merge_hours, None);
    }

    #[test]
    fn people_and_nested_scopes_validate_without_query_injection() {
        assert_eq!(
            endpoint(&Scope::Person("some.user".into())).unwrap(),
            "merge_requests?scope=all&author_username=some.user"
        );
        assert!(endpoint(&Scope::Person("a&scope=all".into())).is_err());
        assert!(endpoint(&Scope::Person("group/user".into())).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn later_429_without_headers_stops_all_following_cohorts() {
        use std::os::unix::fs::PermissionsExt;
        for throttle_created in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join("glab");
            let row = serde_json::to_string(&sample(1)).unwrap();
            let created_next = if throttle_created { "2" } else { "" };
            // The duplicate also checks that row validation cannot erase the
            // rate-limit stop reason before load_window decides what to fetch.
            let script = format!(
                r##"#!/bin/sh
printf '%s\n' "$5" >> "$0.calls"
case "$5" in
  *state=all*page=1) printf 'HTTP/2 200\nx-next-page: {created_next}\n\n[{row},{row}]';;
  *state=merged*page=1) printf 'HTTP/2 200\nx-next-page: 2\n\n[{row}]';;
  *page=2) printf 'HTTP/2 429\n\nlimited'; exit 1;;
  *) printf 'HTTP/2 200\nx-next-page: \n\n[]';;
esac
"##
            );
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            let report = load_window(
                &program,
                source("gitlab.com").unwrap(),
                "1".into(),
                Scope::Mine,
                "2026-09-01T00:00:00Z".parse().unwrap(),
                "2026-09-03T00:00:00Z".parse().unwrap(),
            )
            .await
            .unwrap();
            let calls = std::fs::read_to_string(program.with_extension("calls")).unwrap();
            assert_eq!(calls.lines().count(), if throttle_created { 2 } else { 3 });
            assert!(!calls.contains("/notes?"));
            assert_eq!(report.counts.created, 1);
            let coverage = if throttle_created {
                &report.coverage
            } else {
                &report.merged_window.as_ref().unwrap().coverage
            };
            assert_eq!(coverage.stop, Stop::RateLimited);
            assert_eq!(coverage.rate_remaining, None);
            assert_eq!(report.activity.unwrap().comments, None);
        }
    }

    #[test]
    fn missing_timing_and_reviewers_do_not_become_zero() {
        let mut row = sample(1);
        row.as_object_mut().unwrap().remove("merged_at");
        row.as_object_mut().unwrap().remove("reviewers");
        let out = report(vec![row]);
        assert_eq!(out.counts.merged, 1);
        assert_eq!(out.authors[0].mean_merge_hours, None);
        assert_eq!(out.authors[0].timed_merges, 0);
        assert_eq!(out.reviewer_rows_measured, 0);
        assert_eq!(out.review_activity, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn comment_429_without_headers_stops_formal_review_requests() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        let row = serde_json::to_string(&sample(1)).unwrap();
        std::fs::write(
            &program,
            format!(
                r##"#!/bin/sh
printf '%s\n' "$5" >> "$0.calls"
case "$5" in
  *state=all*|*state=merged*) printf 'HTTP/2 200\nx-next-page: \n\n[{row}]' ;;
  */notes\?*) printf 'HTTP/2 429\n\nlimited'; exit 1 ;;
  *) exit 1 ;;
esac
"##
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let report = load_window(
            &program,
            source("gitlab.com").unwrap(),
            "1".into(),
            Scope::Mine,
            "2026-09-01T00:00:00Z".parse().unwrap(),
            "2026-09-03T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        let calls = std::fs::read_to_string(program.with_extension("calls")).unwrap();
        assert_eq!(calls.lines().count(), 3);
        assert!(!calls.contains("/approvals"));
        assert!(!calls.contains("/reviewers"));
        assert_eq!(report.counts.created, 1);
        assert_eq!(report.merged_window.unwrap().count, 1);
        let activity = report.activity.unwrap();
        assert!(activity.rate_limited);
        assert_eq!(activity.rate_remaining, None);
        assert_eq!(activity.comments, None);
        let evidence = report.review_evidence.unwrap();
        assert_eq!(evidence.current_approvals, None);
        assert_eq!(evidence.current_change_requests, None);
    }
    #[test]
    fn duplicate_invalid_and_wrong_host_rows_qualify_all_measures() {
        let mut wrong = sample(2);
        wrong["web_url"] = json!("https://other.example/g/sub/p/-/merge_requests/2");
        let out = report(vec![sample(1), sample(1), wrong, json!({})]);
        assert_eq!(out.counts.created, 1);
        assert!(!out.coverage.complete);
        assert_eq!(out.coverage.stop, Stop::InvalidData);
        assert_eq!(out.history[0].project, "g/sub/p");
        assert_eq!(out.reviewers[0].assigned, 1);
        assert_eq!(out.authors[0].mean_merge_hours, None);
    }
    #[test]
    fn scope_paths_cannot_inject_query_or_escape_namespace() {
        assert!(endpoint(&Scope::Project("a/b?scope=all".into())).is_err());
        assert!(endpoint(&Scope::Group("a/../b".into())).is_err());
        assert_eq!(
            endpoint(&Scope::Project("g/sub/p".into())).unwrap(),
            "projects/g%2Fsub%2Fp/merge_requests?scope=all"
        );
        assert!(source("local.example").is_ok());
        assert!(source("local.example/path").is_err());
        let out = summarize(
            source("gitlab.com").unwrap(),
            "1".into(),
            Scope::Group("g/s".into()),
            "2026-09-01T00:00:00Z".parse().unwrap(),
            "2026-09-03T00:00:00Z".parse().unwrap(),
            vec![sample(1)],
            coverage(),
        );
        assert_eq!(out.history.len(), 0);
        assert!(!out.coverage.complete);
    }
    #[test]
    fn status_and_rate_headers_survive_transport() {
        assert!(matches!(
            parse(b"HTTP/2 429\n\n{}", false),
            Err(RequestFailure {
                stop: Stop::RateLimited,
                ..
            })
        ));
        let out = parse(b"HTTP/2 200\nX-Next-Page: 2\nRateLimit-Remaining: 0\nRateLimit-Reset: 42\nX-Total: 3\n\n[]", true).unwrap();
        assert_eq!(out.next, Some(2));
        assert_eq!(out.remaining, Some(0));
        assert_eq!(out.reset, Some(42));
        assert!(!parse(b"HTTP/2 200\n\n[]", true).unwrap().terminal);
    }
    #[cfg(unix)]
    async fn scripted(
        script: &str,
        budget: Duration,
    ) -> Result<(Vec<Value>, Coverage), RequestFailure> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(
            &program,
            pages(
                &program,
                "gitlab.com",
                "merge_requests?scope=created_by_me",
                budget,
            ),
        )
        .await
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn initial_project_timeout_keeps_personal_scope_available() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, "#!/bin/sh\nsleep 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let tree = discover_tree(
            &program,
            source("gitlab.com").unwrap(),
            "viewer-id".into(),
            Duration::from_millis(30),
        )
        .await
        .unwrap();
        assert_eq!(tree.viewer, "viewer-id");
        assert!(tree.projects.is_empty());
        assert_eq!(tree.coverage.pages, 0);
        assert_eq!(tree.coverage.stop, Stop::Timeout);
        assert!(!tree.coverage.complete);
        assert!(tree.group_error.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn late_timeout_retains_rows_and_first_failure_is_not_empty_success() {
        let (rows, coverage) = scripted("case \"$5\" in *page=1) printf 'HTTP/2 200\\nx-next-page: 2\\nx-total: 2\\n\\n[{}]';; *) touch \"$0.timeout\"; exec sleep 60;; esac", Duration::from_millis(300)).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!coverage.complete);
        assert_eq!(coverage.stop, Stop::Timeout);
        assert!(scripted("exit 1", Duration::from_secs(1)).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn rate_limit_and_missing_pagination_preserve_partial_evidence() {
        let (_, coverage) = scripted(
            "printf 'HTTP/2 200\\nx-next-page: 2\\nratelimit-remaining: 0\\n\\n[{}]'",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(coverage.stop, Stop::RateLimited);
        assert_eq!(coverage.received, 1);
        let (_, coverage) = scripted("printf 'HTTP/2 200\\n\\n[]'", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(coverage.stop, Stop::UnknownPagination);
        assert!(!coverage.complete);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn later_429_replaces_prior_rate_evidence_and_preserves_rows_in_cache() {
        for (headers, expected_remaining, expected_reset) in [
            (
                "ratelimit-remaining: 0\\nratelimit-reset: 200\\n",
                Some(0),
                Some(200),
            ),
            ("", None, None),
        ] {
            let script = format!(
                "case \"$5\" in *page=1) printf 'HTTP/2 200\\nx-next-page: 2\\nratelimit-remaining: 99\\nratelimit-reset: 100\\n\\n[{{}}]';; *) printf 'HTTP/2 429\\n{headers}\\nrate limit reached'; exit 1;; esac"
            );
            let (rows, coverage) = scripted(&script, Duration::from_secs(1)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(coverage.stop, Stop::RateLimited);
            assert_eq!(coverage.rate_remaining, expected_remaining);
            assert_eq!(coverage.rate_reset, expected_reset);
            assert!(!coverage.complete);
            let mut out = report(vec![sample(1)]);
            out.coverage = coverage;
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("cache.sqlite");
            crate::store::gitlab_stats::put(&db, "rate-test", &out).unwrap();
            let cached = crate::store::gitlab_stats::get(&db, "rate-test")
                .unwrap()
                .unwrap();
            assert_eq!(cached.coverage.rate_remaining, expected_remaining);
            assert_eq!(cached.coverage.rate_reset, expected_reset);
            assert_eq!(cached.history.len(), 1);
        }
    }
    #[test]
    fn cache_separates_viewers_hosts_scopes_and_retains_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cache.sqlite");
        let out = report(vec![sample(1), sample(1)]);
        let key = "[gitlab,gitlab.com,viewer1,mine]";
        crate::store::gitlab_stats::put(&db, key, &out).unwrap();
        assert!(
            crate::store::gitlab_stats::get(&db, "[github,github.com,viewer1,mine]")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::store::gitlab_stats::get(&db, "[gitlab,gitlab.com,viewer2,mine]")
                .unwrap()
                .is_none()
        );
        let cached = crate::store::gitlab_stats::get(&db, key).unwrap().unwrap();
        assert!(!cached.coverage.complete);
        assert_eq!(cached.history.len(), 1);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_later_page_keeps_earlier_statistics_as_partial() {
        let large = json!([{ "description": "x".repeat(4 * 1024 * 1024) }]);
        let script = format!("case \"$5\" in *page=1) printf 'HTTP/2 200\\nx-next-page: 2\\n\\n[{{}}]';; *) printf 'HTTP/2 200\\nx-next-page:\\n\\n%s' '{large}';; esac");
        let (rows, coverage) = scripted(&script, Duration::from_secs(30)).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(coverage.stop, Stop::InvalidData);
        assert!(!coverage.complete);
    }
}
