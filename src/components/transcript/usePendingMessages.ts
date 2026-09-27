/// The pending messages one transcript host holds (#1491), reconciled
/// against the messages it has read. See `pending.ts` for the model and
/// the rule.
///
/// In 7.9 nothing calls `add`, so `visible` is always empty; the hosts
/// hold this so 7.10's send path has somewhere to put what it sends.

import { useCallback, useMemo, useState } from "react";
import type { TranscriptMessage } from "../../types/transcript";
import {
  newPendingMessage,
  type PendingMessage,
  type PendingState,
  reconcilePending,
} from "./pending";

export interface PendingMessages {
  /// Pending messages the transcript does not hold yet, oldest first.
  visible: readonly PendingMessage[];
  /// Start a pending message for `text`; returns it (its `clientId` is
  /// the send's idempotency key).
  add: (text: string) => PendingMessage;
  /// Record what the send path learned about one.
  settle: (clientId: string, state: PendingState, reason?: string | null) => void;
}

export function usePendingMessages(messages: readonly TranscriptMessage[]): PendingMessages {
  const [list, setList] = useState<PendingMessage[]>([]);
  const { unmatched } = useMemo(() => reconcilePending(list, messages), [list, messages]);

  // A matched message is gone for good: drop it, so a later window that
  // no longer holds its record cannot bring the pending copy back.
  // Adjusted during render, as React documents for state derived from a
  // prop, so no commit ever shows it beside its record.
  if (unmatched.length !== list.length) setList(unmatched);

  const newest = messages.length > 0 ? messages[messages.length - 1].id : null;
  const add = useCallback(
    (text: string) => {
      const p = newPendingMessage(text, newest);
      setList((l) => [...l, p]);
      return p;
    },
    [newest],
  );
  const settle = useCallback((clientId: string, state: PendingState, reason: string | null = null) => {
    setList((l) => l.map((p) => (p.clientId === clientId ? { ...p, state, reason } : p)));
  }, []);

  return { visible: unmatched, add, settle };
}
