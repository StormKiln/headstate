import type { TranscriptMessage } from "../../types/transcript";
import { palette } from "./palette";
import type { ToolVariant } from "./types";

type TaskStatusKind = Extract<TranscriptMessage["kind"], { kind: "task_status" }>;

/// A background task's recorded state (#1483, the #1475 follow-up).
///
/// A `task_status` attachment used to render as a notice carrying only
/// its description. It is its own kind now, so the row shows the STATUS
/// verbatim beside the task. A status the record did not carry is said
/// to be unrecorded, never guessed.
///
/// The `model` attachment is NOT shown: it is the identity line the
/// harness hands the model each turn, and a change of model is already
/// its own derived message (`model_change`).
export function TaskStatusRow({
  kind,
  description,
  variant,
}: {
  kind: TaskStatusKind;
  /// The message's first text block, when it had one.
  description: string | null;
  variant: ToolVariant;
}) {
  const type = kind.task_type ? kind.task_type.replace(/^local_/, "").replace(/_/g, " ") : null;
  return (
    <p
      className={variant === "terminal" ? "font-mono text-[12px]" : "text-[13px]"}
      style={{ color: palette.muted }}
    >
      <span aria-hidden>⎿ </span>
      Background {type ? `${type} ` : ""}task
      {description ? <span style={{ color: palette.text }}>: {description}</span> : null}
      {" — "}
      {kind.status ? kind.status.replace(/_/g, " ") : "status not recorded"}
    </p>
  );
}
