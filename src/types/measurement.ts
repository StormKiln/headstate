/** Closed diagnostic input. References are native-issued capture-qualified handles. */
export interface MeasurementReference { epoch: string; capture: number; id: number }
export type MeasurementOutcome = "accepted" | "retained" | "rejected" | "cache_reuse" | "no_work" | "unknown" | "unsupported";
export type MeasurementTranscriptPhase = "read" | "follow" | "page" | "render" | "raf_proxy" | "evict" | "hidden" | "idle";
export type ClientMeasurement = {
  kind: "mounted_review"; source: "github" | "gitlab" | "unknown"; list: "authored" | "reviewing" | "unknown";
  surface: "ready_panel" | "review_list"; selection: "all_repositories" | "selected_scope" | "unknown";
  receipt?: MeasurementReference | null; scope?: MeasurementReference | null;
  inventory_count?: number | null; eligible_count?: number | null; visible_count?: number | null;
  visible_retained_count?: number | null; visible_retained_readiness_count?: number | null;
  visible_readiness_unknown_count?: number | null; visible_last_known_count?: number | null;
  visible_advisory_unavailable_count?: number | null;
  footer_location: "desktop_footer" | "phone_banner" | "hidden" | "unmeasured";
  footer: "checked" | "needs_checking" | "partly_checked" | "coverage_unknown" | "not_checked" | "checking" | "retrying" | "load_failed" | "refresh_failed" | "background_stopped" | "auth_unavailable" | "auth_unknown" | "legacy_up_to_date" | "hidden" | "unavailable";
} | {
  kind: "stats_view"; scope?: MeasurementReference | null; outcome: MeasurementOutcome; elapsed_ms?: number | null; rows?: number | null;
} | {
  kind: "transcript_view"; operation?: MeasurementReference | null; phase: MeasurementTranscriptPhase;
  elapsed_ms?: number | null; rows?: number | null; resident_rows?: number | null; capability: "measured" | "unsupported" | "unmeasured";
};
export interface MeasurementLoss {
  dropped: number; invalid: number; cardinality: number; stale_handle: number; budget: number;
  coalesced: number; clock_anomaly: number; overflow: number; writer: number; malformed: number; unclean_capture: number;
  durable_gap_records: number; durable_gap_bytes: number; durable_gap_segments: number; deferred_aggregate_gaps: number;
  rotated_out: number; rotated_bytes: number; by_domain: number[];
}
export interface MeasurementStatus {
  enabled: boolean; schema: number; epochs: number; oldestWallMs: number | null; newestWallMs: number | null;
  durableRecords: number; bytes: number; dropped: number; invalid: number; coalesced: number; rotatedOut: number;
  writerState: "ready" | "disabled" | "unavailable"; loss: MeasurementLoss; durableSeq: number; incomplete: boolean;
}
export interface MeasurementExportReceipt {
  canceled: boolean; records: number; bytes: number; oldestWallMs: number | null; newestWallMs: number | null; incomplete: boolean;
}
