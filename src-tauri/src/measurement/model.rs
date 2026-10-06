use serde::{Deserialize, Serialize};
macro_rules! closed { ($name:ident { $($value:ident),+ $(,)? }) => {
    #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
    #[serde(rename_all = "snake_case")]
    pub enum $name { $($value),+ }
}; }
closed!(Role { Desktop, Phone });
closed!(Platform {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
    Unknown
});
closed!(Domain {
    Queue,
    Stats,
    StopFailure,
    Transcript,
    Client
});
closed!(List {
    Authored,
    Reviewing,
    Unknown
});
closed!(Phase {
    NotRequested,
    Fetching,
    Ready,
    Partial,
    Unknown,
    Retrying,
    Failed,
    NotAsked
});
closed!(Coverage {
    Complete,
    Partial,
    Unknown
});
closed!(Acceptance {
    Accepted,
    RejectedOlder,
    RejectedOwnership,
    RejectedCas,
    NoWork,
    Unknown
});
closed!(Stage {
    Started,
    Submitted,
    Acknowledged,
    Published,
    Completed
});
closed!(Outcome {
    Success,
    Failed,
    Refused,
    Canceled,
    Coalesced,
    CacheReuse,
    NotIssued,
    Unknown,
    Unsupported,
    Unmeasured
});
closed!(StatsOutcome {
    Accepted,
    Retained,
    Rejected,
    CacheReuse,
    NoWork,
    Unknown,
    Unsupported
});
closed!(MatchOutcome {
    HookObserved,
    BoundaryObserved,
    Matched,
    Unpaired,
    OutOfWindow,
    Duplicate,
    ClockAnomaly,
    Censored,
    Ambiguous
});
closed!(FailureObservation {
    IngestedHook,
    SampledIdle,
    CandidatePair,
    RetrospectivePair
});
closed!(TranscriptPhase {
    Read,
    Follow,
    Page,
    Render,
    RafProxy,
    Evict,
    Hidden,
    Idle
});
closed!(Capability {
    Measured,
    Unsupported,
    Unmeasured
});
closed!(Source {
    Github,
    Gitlab,
    Unknown
});
closed!(Selection {
    AllRepositories,
    SelectedScope,
    Unknown
});
closed!(Surface {
    ReadyPanel,
    ReviewList
});
closed!(FooterLocation {
    DesktopFooter,
    PhoneBanner,
    Hidden,
    Unmeasured
});
closed!(Footer {
    Checked,
    NeedsChecking,
    PartlyChecked,
    CoverageUnknown,
    NotChecked,
    Checking,
    Retrying,
    LoadFailed,
    RefreshFailed,
    BackgroundStopped,
    AuthUnavailable,
    AuthUnknown,
    LegacyUpToDate,
    Hidden,
    Unavailable
});
closed!(AggregateKind {
    Admitted,
    Refused,
    Canceled,
    Completed,
    Active,
    MaxConcurrency,
    Tick,
    Idle,
    Declined,
    Continuation,
    NoWork,
    Candidates,
    Hidden,
    Read,
    Growth,
    Eviction,
    CacheReuse,
    Coalesced,
    NotIssued,
    Upsert
});
closed!(WorkClass {
    Foreground,
    Background,
    Unknown
});

/// Fields are private: only a recorder can issue native authority. Deserialization
/// is necessary for returning tokens; every input token is validated at admission.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct OpaqueId {
    pub(super) epoch: String,
    pub(super) capture: u64,
    pub(super) id: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMeasurement {
    MountedReview {
        source: Source,
        list: List,
        surface: Surface,
        selection: Selection,
        receipt: Option<OpaqueId>,
        scope: Option<OpaqueId>,
        inventory_count: Option<u32>,
        eligible_count: Option<u32>,
        visible_count: Option<u32>,
        visible_retained_count: Option<u32>,
        visible_retained_readiness_count: Option<u32>,
        visible_readiness_unknown_count: Option<u32>,
        visible_last_known_count: Option<u32>,
        visible_advisory_unavailable_count: Option<u32>,
        footer_location: FooterLocation,
        footer: Footer,
    },
    StatsView {
        scope: Option<OpaqueId>,
        outcome: StatsOutcome,
        elapsed_ms: Option<u64>,
        rows: Option<u32>,
    },
    TranscriptView {
        operation: Option<OpaqueId>,
        phase: TranscriptPhase,
        elapsed_ms: Option<u64>,
        rows: Option<u32>,
        resident_rows: Option<u32>,
        capability: Capability,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    QueueReceipt {
        owner: OpaqueId,
        list: List,
        operation: Option<OpaqueId>,
        revision: u64,
        receipt_revision: Option<u64>,
        phase: Phase,
        coverage: Coverage,
        rows: u32,
        receipt_age_ms: Option<u64>,
        outcome: Acceptance,
    },
    Operation {
        operation: OpaqueId,
        parent: Option<OpaqueId>,
        domain: Domain,
        stage: Stage,
        outcome: Outcome,
        elapsed_ms: Option<u64>,
        affected_fields: u16,
    },
    StatsProgress {
        scope: OpaqueId,
        days: Option<u16>,
        covered_days: Option<u16>,
        partial_days: Option<u16>,
        unknown_days: Option<u16>,
        retrieved: Option<u32>,
        accumulated: Option<u32>,
        total: Option<u32>,
        outcome: StatsOutcome,
        elapsed_ms: Option<u64>,
    },
    StopFailureMatch {
        session: OpaqueId,
        observed_boundary: Option<u32>,
        observation: FailureObservation,
        delta_ms: Option<i64>,
        observed_lag_ms: Option<i64>,
        hook_age_ms: Option<u64>,
        outcome: MatchOutcome,
    },
    Transcript {
        operation: OpaqueId,
        phase: TranscriptPhase,
        elapsed_ms: Option<u64>,
        bytes: Option<u32>,
        rows: Option<u32>,
        resident_rows: Option<u32>,
        capability: Capability,
    },
    Client {
        observation: ClientMeasurement,
    },
    Aggregate {
        domain: Domain,
        metric: AggregateKind,
        work: WorkClass,
        count: u64,
        interval_ms: u64,
        coalesced: bool,
    },
}
impl Event {
    pub fn domain(&self) -> Domain {
        match self {
            Self::QueueReceipt { .. } => Domain::Queue,
            Self::Operation { domain, .. } | Self::Aggregate { domain, .. } => *domain,
            Self::StatsProgress { .. } => Domain::Stats,
            Self::StopFailureMatch { .. } => Domain::StopFailure,
            Self::Transcript { .. } => Domain::Transcript,
            Self::Client { .. } => Domain::Client,
        }
    }
    pub(super) fn handles(&self) -> Vec<&OpaqueId> {
        match self {
            Self::QueueReceipt {
                owner, operation, ..
            } => std::iter::once(owner).chain(operation.iter()).collect(),
            Self::Operation {
                operation, parent, ..
            } => std::iter::once(operation).chain(parent.iter()).collect(),
            Self::StatsProgress { scope, .. } => vec![scope],
            Self::StopFailureMatch { session, .. } => vec![session],
            Self::Transcript { operation, .. } => vec![operation],
            Self::Aggregate { .. } => vec![],
            Self::Client { observation } => match observation {
                ClientMeasurement::MountedReview { receipt, scope, .. } => {
                    receipt.iter().chain(scope.iter()).collect()
                }
                ClientMeasurement::StatsView { scope, .. } => scope.iter().collect(),
                ClientMeasurement::TranscriptView { operation, .. } => operation.iter().collect(),
            },
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub schema: u8,
    pub epoch: String,
    pub capture: u64,
    pub seq: u64,
    pub monotonic_ms: u64,
    pub wall_time_ms: u64,
    pub role: Role,
    pub event: Event,
}
#[derive(Clone, Default, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Loss {
    pub dropped: u64,
    pub invalid: u64,
    pub cardinality: u64,
    pub stale_handle: u64,
    pub budget: u64,
    pub coalesced: u64,
    pub clock_anomaly: u64,
    pub overflow: u64,
    pub writer: u64,
    pub malformed: u64,
    pub unclean_capture: u64,
    pub rotated_out: u64,
    pub rotated_bytes: u64,
    pub by_domain: [u64; 5],
}
impl Loss {
    pub fn incomplete(&self) -> bool {
        self.dropped > 0
            || self.invalid > 0
            || self.cardinality > 0
            || self.stale_handle > 0
            || self.budget > 0
            || self.overflow > 0
            || self.writer > 0
            || self.malformed > 0
            || self.unclean_capture > 0
            || self.rotated_out > 0
    }
}
closed!(WriterState {
    Ready,
    Disabled,
    Unavailable
});
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalStatus {
    pub enabled: bool,
    pub schema: u8,
    pub epochs: u32,
    pub oldest_wall_ms: Option<u64>,
    pub newest_wall_ms: Option<u64>,
    pub durable_records: u64,
    pub bytes: u64,
    pub dropped: u64,
    pub invalid: u64,
    pub coalesced: u64,
    pub rotated_out: u64,
    pub writer_state: WriterState,
    pub loss: Loss,
    pub durable_seq: u64,
    pub incomplete: bool,
}
impl Default for JournalStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            schema: 1,
            epochs: 0,
            oldest_wall_ms: None,
            newest_wall_ms: None,
            durable_records: 0,
            bytes: 0,
            dropped: 0,
            invalid: 0,
            coalesced: 0,
            rotated_out: 0,
            writer_state: WriterState::Disabled,
            loss: Loss::default(),
            durable_seq: 0,
            incomplete: false,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportReceipt {
    pub canceled: bool,
    pub records: u64,
    pub bytes: u64,
    pub oldest_wall_ms: Option<u64>,
    pub newest_wall_ms: Option<u64>,
    pub incomplete: bool,
}
impl ExportReceipt {
    pub fn canceled() -> Self {
        Self {
            canceled: true,
            records: 0,
            bytes: 0,
            oldest_wall_ms: None,
            newest_wall_ms: None,
            incomplete: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportError {
    Busy,
    Unavailable,
    Timeout,
    Destination,
    Canceled,
}
impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "A measurement export is already running.",
            Self::Unavailable => "Measurements are unavailable. Try again after restarting.",
            Self::Timeout => "Measurements could not reach a durable cutoff. Try again.",
            Self::Destination => "Could not save measurements. Choose a writable local file.",
            Self::Canceled => "Measurement export canceled.",
        })
    }
}

/// Reserved metadata; does not compete with exact-event admission.
#[derive(Clone, Default, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    pub captures_opened: u64,
    pub captures_closed: u64,
    pub active_capture: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeferredAggregate {
    pub domain: Domain,
    pub metric: AggregateKind,
    pub work: WorkClass,
    pub count: u64,
    pub elapsed_ms: u64,
}
