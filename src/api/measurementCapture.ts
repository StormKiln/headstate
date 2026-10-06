import type { QueryClient } from "@tanstack/react-query";
import type { MeasurementCapture, MeasurementReference } from "../types/measurement";
import type { StatsBoard } from "../types/pr";
const key = ["measurement-capture"] as const;
const writes = new WeakMap<QueryClient, number>();
/** A successful same-desktop reply proves lifetime, never handle ownership. */
export function captureReference(qc: QueryClient, reference?: MeasurementReference | null) {
  const current = qc.getQueryData<MeasurementCapture | null>(key);
  return reference && current && reference.epoch === current.epoch && reference.capture === current.capture ? reference : undefined;
}
export function beginCaptureWrite(qc: QueryClient) {
  const serial = (writes.get(qc) ?? 0) + 1;
  writes.set(qc, serial);
  return serial;
}
export function acceptCaptureWrite(qc: QueryClient, serial: number, capture: MeasurementCapture | null | void) {
  const latest = writes.get(qc) === serial;
  // A late acknowledgment cannot establish which transition ran last natively.
  qc.setQueryData(key, latest ? capture ?? null : null);
  for (const query of qc.getQueryCache().findAll({queryKey:["stats-board"]})) {
    const board = query.state.data as StatsBoard | undefined;
    if (board?.measurementScope && !captureReference(qc, board.measurementScope)) {
      qc.setQueryData(query.queryKey, {...board, measurementScope:undefined}, {updatedAt:query.state.dataUpdatedAt});
    }
  }
  return latest;
}
