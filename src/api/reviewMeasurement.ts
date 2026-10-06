import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { observationStatus } from "../lib/rowObservation";
import { readyAge } from "../lib/readyAge";
import type { SourceRefreshSnapshot } from "./sourceRefresh";
import type { PullRequest } from "../types/pr";
import type { ClientMeasurement, MeasurementReference } from "../types/measurement";
import { publishClientMeasurements } from "./clientMeasurements";

type Review = Extract<ClientMeasurement, { kind: "mounted_review" }>;
type Presentation = Pick<Review, "footer" | "footer_location">;
const INITIAL: Presentation = { footer: "unavailable", footer_location: "unmeasured" };
const Context = createContext<{
  enabled: boolean; receipt?: MeasurementReference; selection: Review["selection"];
  presentation: Presentation | undefined; present: (value: Presentation) => void;
}>({ enabled: false, selection: "unknown", presentation: INITIAL, present: () => {} });

/** Lifetime is the mounted application, never a process-global account cache. */
export function ReviewMeasurementProvider({ enabled, receipt, selected, snapshot, children }: {
  enabled: boolean; receipt?: MeasurementReference; selected: boolean; snapshot: SourceRefreshSnapshot; children: ReactNode;
}) {
  const frame = useMemo(() => ({ rows: snapshot.prs, phase: snapshot.phase, error: snapshot.error, coverage: snapshot.coverage, session: snapshot.session, receipt, selected }), [snapshot.prs, snapshot.phase, snapshot.error, snapshot.coverage, snapshot.session, receipt, selected]);
  const [shown, setShown] = useState<{ frame: object; value: Presentation }>();
  const presentation = shown?.frame === frame ? shown.value : undefined;
  const present = useCallback((next: Presentation) => setShown(old =>
    old?.frame === frame && old.value.footer === next.footer && old.value.footer_location === next.footer_location ? old : { frame, value: next }), [frame]);
  const value = useMemo(() => ({ enabled, receipt, selection: selected ? "selected_scope" as const : "all_repositories" as const,
    presentation, present }), [enabled, receipt, selected, presentation, present]);
  return createElement(Context.Provider, { value }, children);
}

export function useReviewPresentation(footer: Review["footer"], footer_location: Review["footer_location"]) {
  const { enabled, present } = useContext(Context);
  useEffect(() => { if (enabled) present({ footer, footer_location }); }, [enabled, present, footer, footer_location]);
}

/** The mounted Ready qualifier, shared with its cost measurement. Display evidence is read-only. */
export function readyMeasurementLastKnown(pr: PullRequest,
  stack: { freshness: "fresh" | "retained"; value: { kind: string } } | undefined,
  pusher: { freshness: "fresh" | "retained"; value: { pusher: { state: string } } } | undefined, now: Date) {
  return observationStatus(pr)?.label === "Readiness last known"
    || (pr.observation?.ready_at_state === "retained" && readyAge(pr.ready_at, now).since !== null)
    || (stack?.freshness === "retained" && stack.value.kind === "stacked")
    || (pusher?.freshness === "retained" && pusher.value.pusher.state === "viewer");
}

/** Bounded pure observations of arrays the view already owns. No demand-producing calls. */
export function readyMeasurementCounts(inventory: PullRequest[], eligible: PullRequest[], visible: PullRequest[], available: boolean,
  lastKnown: (row: PullRequest) => boolean) {
  if (!available) return {};
  const counts: Partial<Review> = { inventory_count: inventory.length, eligible_count: eligible.length, visible_count: visible.length };
  if (visible.length > 4096) return counts;
  let membership = 0, retained = 0, unknown = 0, last = 0;
  for (const row of visible) {
    if (row.observation?.state === "retained") membership++;
    if (row.observation?.retained_fields.length) retained++;
    if (!row.observation || row.observation.unknown_fields.length) unknown++;
    if (lastKnown(row)) last++;
  }
  return { ...counts, visible_retained_count: membership, visible_retained_readiness_count: retained,
    visible_readiness_unknown_count: unknown, visible_last_known_count: last };
}

export function useReadyMeasurement(inventory: PullRequest[], eligible: PullRequest[], visible: PullRequest[], available: boolean,
  lastKnown: (row: PullRequest) => boolean) {
  const context = useContext(Context);
  const last = useRef<string | undefined>(undefined);
  const observation: Review | undefined = context.enabled && context.presentation ? {
    kind: "mounted_review", source: "github", list: "reviewing", surface: "ready_panel",
    selection: context.selection, receipt: context.receipt,
    ...readyMeasurementCounts(inventory, eligible, visible, available, lastKnown), ...context.presentation,
  } : undefined;
  const encoded = observation ? JSON.stringify(observation) : undefined;
  useEffect(() => {
    if (!encoded) { last.current = undefined; return; }
    if (encoded === last.current) return;
    last.current = encoded;
    // Failure is diagnostic loss, never a UI error or a retry/refresh request.
    void publishClientMeasurements([JSON.parse(encoded) as Review]).catch(() => {});
  }, [encoded]);
}
