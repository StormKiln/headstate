//! Scope only awaited command work. Spawned background tasks do not inherit it.
use crate::measurement::{Domain, Event, OpaqueId, OperationClass, Outcome, Recorder, Stage};
use std::{future::Future, time::Instant};
tokio::task_local! { static CURRENT: (OpaqueId, OperationClass); }
pub fn current() -> Option<OpaqueId> {
    CURRENT.try_with(|(id, _)| id.clone()).ok()
}
pub fn mark(recorder: Option<&Recorder>, stage: Stage, outcome: Outcome) {
    if let Some(recorder) = recorder {
        let _ = CURRENT.try_with(|(id, class)| {
            recorder.record(Event::Operation {
                operation: id.clone(),
                operation_class: Some(*class),
                parent: None,
                domain: Domain::Queue,
                stage,
                outcome,
                elapsed_ms: None,
                affected_fields: None,
            })
        });
    }
}
struct Scope<'a> {
    recorder: &'a Recorder,
    id: OpaqueId,
    class: OperationClass,
    started: Instant,
    outcome: Outcome,
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.recorder.record(Event::Operation {
            operation: self.id.clone(),
            operation_class: Some(self.class),
            parent: None,
            domain: Domain::Queue,
            stage: Stage::Completed,
            outcome: self.outcome,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).ok(),
            affected_fields: None,
        });
        self.recorder.retire(&self.id);
    }
}
pub async fn run<T, E>(
    recorder: Option<&Recorder>,
    class: OperationClass,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    if current().is_some() {
        return future.await;
    }
    let Some(recorder) = recorder.filter(|r| r.enabled()) else {
        return future.await;
    };
    let Some(id) = recorder.next_operation(None) else {
        return future.await;
    };
    let mut scope = Scope {
        recorder,
        id: id.clone(),
        class,
        started: Instant::now(),
        outcome: Outcome::Canceled,
    };
    CURRENT
        .scope((id, class), async {
            mark(Some(recorder), Stage::Started, Outcome::Unknown);
            let result = future.await;
            scope.outcome = if result.is_ok() {
                Outcome::Success
            } else {
                Outcome::Failed
            };
            result
        })
        .await
}
pub fn aggregate(
    recorder: Option<&Recorder>,
    metric: crate::measurement::AggregateKind,
    work: crate::measurement::WorkClass,
) {
    if let Some(recorder) = recorder {
        recorder.aggregate(crate::measurement::AggregateDelta {
            domain: Domain::Queue,
            metric,
            work,
            count: 1,
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::{Config, Platform, Role};
    #[tokio::test]
    async fn actual_command_scope_records_completion_and_retirement_without_changing_result() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(Config {
            directory: dir.path().canonicalize().unwrap().join("journal"),
            epoch: [44; 16],
            role: Role::Desktop,
            platform: Platform::Macos,
            build: "synthetic".into(),
        })
        .unwrap();
        recorder.set_enabled(true);
        let value = run(Some(&recorder), OperationClass::Detail, async {
            Ok::<_, ()>(42)
        })
        .await;
        assert_eq!(value, Ok(42));
        let path = dir.path().canonicalize().unwrap().join("report.jsonl");
        recorder.export_to(path.clone()).await.unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"operation_class\":\"detail\""));
        assert!(text.contains("\"stage\":\"completed\""));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::measurement::{Config, Platform, Role};
    #[tokio::test]
    async fn measurement_cancellation_early_error_and_spawn_do_not_leak_operation_authority() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(Config {
            directory: dir.path().canonicalize().unwrap().join("journal"),
            epoch: [48; 16],
            role: Role::Desktop,
            platform: Platform::Macos,
            build: "synthetic".into(),
        })
        .unwrap();
        recorder.set_enabled(true);
        let mut captured = None;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            run(Some(&recorder), OperationClass::Action, async {
                captured = current();
                assert!(tokio::spawn(async { current() }).await.unwrap().is_none());
                std::future::pending::<Result<(), ()>>().await
            }),
        )
        .await;
        assert!(result.is_err());
        assert!(recorder.next_operation(captured.as_ref()).is_none());
        let result = run(Some(&recorder), OperationClass::Detail, async {
            Err::<(), _>("PRIVATE_FAILURE")
        })
        .await;
        assert!(result.is_err());
        assert!(current().is_none());
        let path = dir.path().canonicalize().unwrap().join("report.jsonl");
        recorder.export_to(path.clone()).await.unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("PRIVATE_FAILURE"));
        assert!(text.contains("\"outcome\":\"canceled\""));
        assert!(text.contains("\"outcome\":\"failed\""));
    }
}
