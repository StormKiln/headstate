//! Searching merge-request titles for a Jira key (#2040).
//!
//! The GitLab half of `jira::lookup`. One request per project, through the
//! same admission, cooldown and response bound as the queue reads, and only
//! across projects the caller names -- `jira::lookup` passes the projects
//! already in the user's own lists, never the whole host. Each project's
//! answer is kept apart, so one that failed does not discard the others
//! (root CLAUDE.md, "Partial is not nothing").
use super::*;

/// One project's answer: the raw rows read, and what GitLab counted
/// (`X-Total`), which it omits for large result sets.
#[derive(Debug)]
pub struct ProjectSearch {
    pub project: String,
    pub outcome: Result<(Vec<Value>, Option<u64>), QueueError>,
}

/// A Jira key, and nothing else, reaches the query string.
fn plain_phrase(phrase: &str) -> bool {
    !phrase.is_empty()
        && phrase
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A project path as GitLab's API takes it: one URL-encoded id.
pub fn encoded_project(path: &str) -> Option<String> {
    let ok = !path.is_empty()
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        });
    ok.then(|| path.replace('/', "%2F"))
}

pub fn endpoint(project: &str, phrase: &str, per_project: usize) -> Option<String> {
    if !plain_phrase(phrase) {
        return None;
    }
    let id = encoded_project(project)?;
    Some(format!(
        "projects/{id}/merge_requests?search={phrase}&in=title&state=all&order_by=updated_at&per_page={per_project}"
    ))
}

/// Search each of `projects` on `host` for merge requests whose title
/// contains `phrase`. `Err` only when nothing could be asked at all: an
/// unsupported host, or an account glab could not verify.
pub async fn search_titles(
    program: &Path,
    host: &str,
    projects: &[String],
    phrase: &str,
    per_project: usize,
    deadline: tokio::time::Instant,
) -> Result<Vec<ProjectSearch>, QueueError> {
    if super::super::host::validate(host).is_err() {
        return Err(QueueError::UnsupportedHost);
    }
    if !plain_phrase(phrase) {
        return Err(QueueError::Request);
    }
    let initial = super::super::transport::Context::new(
        program,
        host,
        super::super::transport::Class::Foreground,
        deadline,
    );
    let (_, context) = super::super::auth::viewer_with_context(program, host, &initial)
        .await
        .map_err(|issue| {
            if issue == super::super::detail::DetailIssue::Timeout {
                QueueError::Timeout
            } else {
                QueueError::Unauthorized
            }
        })?;
    let mut out = Vec::with_capacity(projects.len());
    for project in projects {
        let outcome = match endpoint(project, phrase, per_project) {
            Some(endpoint) => request_context(program, host, &endpoint, &context)
                .await
                .map(|page| (page.rows, page.total)),
            None => Err(QueueError::Request),
        };
        out.push(ProjectSearch {
            project: project.clone(),
            outcome,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_path_is_one_encoded_id() {
        assert_eq!(
            endpoint("group/sub/project", "ABC-1", 20).as_deref(),
            Some("projects/group%2Fsub%2Fproject/merge_requests?search=ABC-1&in=title&state=all&order_by=updated_at&per_page=20")
        );
    }

    #[test]
    fn nothing_but_a_key_and_a_plain_path_reaches_the_query() {
        assert!(endpoint("group/project", "ABC-1&scope=all", 20).is_none());
        assert!(endpoint("group/../project", "ABC-1", 20).is_none());
        assert!(endpoint("group//project", "ABC-1", 20).is_none());
        assert!(endpoint("group/pro?ject", "ABC-1", 20).is_none());
        assert!(endpoint("", "ABC-1", 20).is_none());
    }
}
