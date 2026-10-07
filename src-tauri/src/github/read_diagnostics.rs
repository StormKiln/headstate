//! Bounded, allowlisted admission evidence; no identity, request or error text.
use super::admission::{AdmissionSnapshot, Bucket, ReadClass};
use super::client::ClientError;
use serde::Serialize;
use std::{sync::Mutex, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Dispatch,
    LocalRefusal,
    ProviderEvidence,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    Admitted,
    RecoveryProbe,
    SecondaryCooldown,
    PrimaryCooldown,
    ReserveCooldown,
    ProbePending,
    AdvisoryAllowance,
    ConsumerShare,
    OperationAllowance,
    PrincipalRetired,
    Deadline,
    LocalOther,
    PrimaryHeader,
    SecondaryHeader,
    PrimaryGraphql,
    SecondaryGraphql,
    SecondaryBody,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Class {
    Foreground,
    Background,
    Advisory,
    Unclassified,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Protocol {
    Graphql,
    Rest,
}

#[derive(Serialize)]
pub(super) struct Event {
    id: u64,
    class: Class,
    protocol: Protocol,
    stage: Stage,
    reason: Reason,
    attempt: u8,
    retry_ms: u64,
    primary_cooldown_ms: u64,
    secondary_cooldown_ms: u64,
    reserve_cooldown_ms: u64,
    evidence_age_ms: Option<u64>,
    recovery_probe: bool,
}
impl Event {
    pub(super) fn new(
        id: u64,
        class: Option<ReadClass>,
        bucket: Bucket,
        stage: Stage,
        reason: Reason,
        snapshot: AdmissionSnapshot,
    ) -> Self {
        let quota = match bucket {
            Bucket::Graphql => snapshot.graphql,
            Bucket::Rest => snapshot.rest,
        };
        Self {
            id,
            class: match class {
                Some(ReadClass::Foreground) => Class::Foreground,
                Some(ReadClass::Background) => Class::Background,
                Some(ReadClass::Advisory) => Class::Advisory,
                None => Class::Unclassified,
            },
            protocol: match bucket {
                Bucket::Graphql => Protocol::Graphql,
                Bucket::Rest => Protocol::Rest,
            },
            stage,
            reason,
            attempt: 0,
            retry_ms: snapshot
                .secondary_cooldown_ms
                .max(quota.primary_cooldown_ms)
                .max(quota.reserve_cooldown_ms),
            primary_cooldown_ms: quota.primary_cooldown_ms,
            secondary_cooldown_ms: snapshot.secondary_cooldown_ms,
            reserve_cooldown_ms: quota.reserve_cooldown_ms,
            evidence_age_ms: quota.evidence_age_ms,
            recovery_probe: quota.recovery_probe,
        }
    }
    pub(super) fn attempt(mut self, attempt: u8) -> Self {
        self.attempt = attempt;
        self
    }
    pub(super) fn refused(mut self, error: &ClientError) -> Self {
        // Categorize only exact internal admission messages. Unknown provider
        // or transport text becomes LocalOther and is never serialized.
        self.reason = match error {
            ClientError::NotDispatched(message) | ClientError::RateLimited(message) => {
                match message.as_str() {
                    "provider cooldown is active" => Reason::SecondaryCooldown,
                    "provider quota retry deadline is active" if self.primary_cooldown_ms > 0 => {
                        Reason::PrimaryCooldown
                    }
                    "provider quota retry deadline is active" => Reason::ReserveCooldown,
                    "provider recovery probe is in progress" => Reason::ProbePending,
                    "advisory attempt allowance is spent for this cycle" => {
                        Reason::AdvisoryAllowance
                    }
                    "advisory consumer share is spent for this cycle"
                    | "advisory consumer capacity is full" => Reason::ConsumerShare,
                    "operation attempt allowance is spent" => Reason::OperationAllowance,
                    "advisory consumer pairing has been retired" => Reason::PrincipalRetired,
                    "read deadline elapsed before dispatch" => Reason::Deadline,
                    _ => Reason::LocalOther,
                }
            }
            ClientError::Timeout(_) => Reason::Deadline,
            _ => Reason::LocalOther,
        };
        self
    }
}

struct Window {
    started: Instant,
    counts: [u8; 12],
}
impl Default for Window {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            counts: [0; 12],
        }
    }
}
#[derive(Default)]
pub(super) struct Diagnostics(Mutex<Window>);
impl Diagnostics {
    fn take(&self, event: &Event) -> bool {
        let mut window = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if window.started.elapsed() >= Duration::from_secs(30) {
            *window = Window::default();
        }
        // Refused strip work cannot exhaust the selected dispatch/evidence log.
        let index = event.class as usize * 3 + event.stage as usize;
        let count = &mut window.counts[index];
        if *count >= 16 {
            return false;
        }
        *count += 1;
        true
    }
    pub(super) fn emit(&self, event: Event) {
        if crate::diag::enabled() && self.take(&event) {
            crate::diag!(
                "[diag] provider admission {}",
                serde_json::to_string(&event).expect("allowlisted admission event")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::admission::Admission;
    #[test]
    fn unknown_messages_cannot_escape_the_schema_and_counters_are_partitioned() {
        let admission = Admission::default();
        let event = |class, stage| {
            Event::new(
                1,
                Some(class),
                Bucket::Graphql,
                stage,
                Reason::LocalOther,
                admission.snapshot(),
            )
        };
        let value = serde_json::to_value(event(ReadClass::Advisory, Stage::LocalRefusal).refused(
            &ClientError::NotDispatched("PRIVATE_TOKEN /private/path PRIVATE_REPO".into()),
        ))
        .unwrap();
        assert_eq!(value["reason"], "local_other");
        assert!(!value.to_string().contains("PRIVATE_"));
        assert_eq!(value.as_object().unwrap().len(), 12);
        let diagnostics = Diagnostics::default();
        for _ in 0..16 {
            assert!(diagnostics.take(&event(ReadClass::Advisory, Stage::LocalRefusal)));
        }
        for _ in 0..150 {
            assert!(!diagnostics.take(&event(ReadClass::Advisory, Stage::LocalRefusal)));
        }
        assert!(diagnostics.take(&event(ReadClass::Foreground, Stage::Dispatch)));
        assert!(diagnostics.take(&event(ReadClass::Advisory, Stage::ProviderEvidence)));
    }
    #[tokio::test(start_paused = true)]
    async fn diagnostic_allowance_recovers_without_unbounded_state() {
        let admission = Admission::default();
        let diagnostics = Diagnostics::default();
        let event = Event::new(
            1,
            None,
            Bucket::Rest,
            Stage::ProviderEvidence,
            Reason::SecondaryBody,
            admission.snapshot(),
        );
        for _ in 0..16 {
            assert!(diagnostics.take(&event));
        }
        assert!(!diagnostics.take(&event));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(diagnostics.take(&event));
    }
}
