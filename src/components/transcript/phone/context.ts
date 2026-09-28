import { createContext, useContext } from "react";
import type { Liveness } from "../../../types/pr";
import type { TaskListState } from "../tasks";
import type { LoadFullText, OpenSubagent } from "../types";

/// What every phone message needs beyond the message itself (#1481).
///
/// A context rather than props on `renderMessage`, because the viewer
/// shell calls `renderMessage(message)` with the message alone -- the
/// shell owns scrolling and knows nothing about sessions.
export interface PhoneTranscriptContext {
  /// The session's liveness: whether a call with no result is still
  /// running, or never came back (`callState`).
  liveness: Liveness;
  /// The session's task list, for task calls that name a task by id.
  tasks: TaskListState;
  /// Dynamic Type's scale; see `textScale.ts`.
  scale: number;
  /// For each message holding recorded thinking, when the thing before
  /// it was recorded; see `thinkingStarts`.
  thinkingStarts: ReadonlyMap<string, string>;
  onLoadFullText?: LoadFullText;
  onOpenSubagent?: OpenSubagent;
  /// Page back until the call a result answers is held (#1476, #1484):
  /// offered on a result whose call is earlier than what is held. Absent
  /// when nothing earlier exists, so no button is offered that cannot
  /// work.
  onLoadEarlier?: (toolUseId: string | null) => void;
}

export const PhoneContext = createContext<PhoneTranscriptContext | null>(null);

export function usePhone(): PhoneTranscriptContext {
  const ctx = useContext(PhoneContext);
  if (ctx === null) throw new Error("a phone transcript message rendered outside PhoneContext");
  return ctx;
}
