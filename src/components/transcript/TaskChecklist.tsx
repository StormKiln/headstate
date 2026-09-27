import { palette } from "./palette";
import { taskStatusWords, taskSummary, type ChecklistTask, type TaskListState } from "./tasks";
import type { ToolVariant } from "./types";

/// The session's task list as a pinned panel (#1504).
///
/// Its hosts -- the desktop side panel (#1480) and the phone sheet
/// (#1481) -- pass `deriveTaskChecklist` over the messages they have
/// loaded. Presentational: nothing here fetches.
///
/// A partial list says so twice: its heading counts are "at least", and
/// a note says which tasks cannot be listed. A task known only from an
/// update is listed by id, its name said to be unknown rather than
/// left blank.
export function TaskChecklist({
  checklist,
  variant,
}: {
  checklist: TaskListState;
  variant: ToolVariant;
}) {
  const summary = taskSummary(checklist);
  const terminal = variant === "terminal";
  return (
    <section
      aria-label="Tasks"
      className={terminal ? "font-mono text-[12px]" : "text-[14px]"}
      style={{ color: palette.text }}
    >
      <h2 className="flex flex-wrap items-baseline gap-x-2 font-semibold">
        Tasks
        {summary ? (
          <span className="font-normal" style={{ color: palette.muted }}>
            {summary.text}
          </span>
        ) : null}
      </h2>
      {checklist.tasks.length === 0 ? (
        <p className="mt-1" style={{ color: palette.muted }}>
          No tasks in the messages shown.
        </p>
      ) : (
        <ul className={terminal ? "mt-1 space-y-0.5" : "mt-2 space-y-1.5"} aria-label="Task list">
          {checklist.tasks.map((t) => (
            <TaskItem key={t.key} task={t} />
          ))}
        </ul>
      )}
      {checklist.truncated ? (
        <p className="mt-1 text-[11px]" style={{ color: palette.muted }}>
          Tasks created before the earliest message shown are not listed.
        </p>
      ) : null}
    </section>
  );
}

const MARK: Record<string, string> = {
  completed: "☑",
  in_progress: "◐",
  pending: "☐",
  deleted: "✕",
};

function TaskItem({ task: t }: { task: ChecklistTask }) {
  const faded = t.status === "completed" || t.status === "deleted";
  const running = t.status === "in_progress";
  const label = running && t.active_form ? t.active_form : t.subject;
  return (
    <li className="flex gap-1.5" style={{ color: faded ? palette.muted : palette.text }}>
      <span aria-hidden>{t.status !== null ? (MARK[t.status] ?? "?") : "?"}</span>
      <span className="sr-only">{taskStatusWords(t.status)}: </span>
      <span className="min-w-0 break-words">
        {t.id !== null ? (
          <span style={{ color: palette.muted }}>#{t.id} </span>
        ) : null}
        {label !== null ? (
          <span
            className={faded ? "line-through" : undefined}
            style={running ? { fontWeight: 600 } : undefined}
          >
            {label}
          </span>
        ) : (
          <span style={{ color: palette.muted }}>(name not in the messages shown)</span>
        )}
        {t.status !== null && !(t.status in MARK) ? (
          <span style={{ color: palette.muted }}> ({taskStatusWords(t.status)})</span>
        ) : null}
        {t.status === "deleted" ? <span style={{ color: palette.muted }}> (deleted)</span> : null}
        {!t.confirmed ? (
          <span style={{ color: palette.muted }}> (requested, not confirmed)</span>
        ) : null}
      </span>
    </li>
  );
}
