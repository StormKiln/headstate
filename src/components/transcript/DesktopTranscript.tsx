/// One session's transcript on the desktop (#1480): the viewer shell
/// (#1479) with the terminal renderer, the task checklist beside it
/// (#1504), a density toggle, and subagent transcripts opened in place.
///
/// Both desktop hosts -- the session detail's pane and the full-window
/// route -- render this. The phone keeps its own path until #1481.
///
/// The data is `useClaudeTranscriptMessages`: the transcript's newest
/// window, re-read on the poll while the session runs. #1476's follow
/// replaces it; everything below the read takes a message list and does
/// not care which.

import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useClaudeTranscriptMessages } from "../../api/hooks";
import { claudeTranscriptBlockText } from "../../api/tauri";
import { useFilters } from "../../store/filters";
import type { Liveness } from "../../types/pr";
import type { TranscriptMessage, TranscriptSubagent } from "../../types/transcript";
import { errorMessage } from "../QueryError";
import { palette } from "./palette";
import { transcriptStreaming } from "./streaming";
import { TaskChecklist } from "./TaskChecklist";
import { deriveTaskChecklist } from "./tasks";
import { TerminalMessage, type TerminalEnv, type TranscriptDensity } from "./TerminalMessage";
import { TranscriptViewer } from "./TranscriptViewer";
import { turnFooters } from "./turnFooter";
import type { LoadFullText, OpenSubagent } from "./types";

export function DesktopTranscript({
  path,
  liveness,
  label = "Transcript",
}: {
  path: string;
  liveness: Liveness;
  label?: string;
}) {
  // A subagent opened from a call replaces the main transcript here, with
  // a way back. A stack, because a subagent can open its own.
  const [stack, setStack] = useState<TranscriptSubagent[]>([]);
  const open = stack[stack.length - 1] ?? null;
  const onOpenSubagent = useCallback<OpenSubagent>((sub) => setStack((s) => [...s, sub]), []);

  if (open && open.transcript_path) {
    return (
      <div className="flex min-h-0 flex-1 flex-col gap-2" data-testid="subagent-transcript">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <button
            type="button"
            onClick={() => setStack((s) => s.slice(0, -1))}
            className="tap-target -ml-1 rounded px-2 hover:bg-[#161b22] focus-visible:outline focus-visible:outline-2"
            style={{ color: palette.link }}
          >
            ← {stack.length > 1 ? "Back to the previous subagent" : "Back to the main transcript"}
          </button>
          <span style={{ color: palette.text }}>
            Subagent{open.agent_type ? `: ${open.agent_type}` : ""}
          </span>
        </div>
        <Loaded
          // Keyed so a different subagent is a fresh read, not the last
          // one's messages under a new name.
          key={open.transcript_path}
          path={open.transcript_path}
          // A subagent that finished cannot still be running a call; one
          // that has not is as live as its session.
          liveness={
            open.status === "completed" ? { state: "dead", why: "subagent completed" } : liveness
          }
          label={`Subagent transcript${open.agent_type ? `: ${open.agent_type}` : ""}`}
          onOpenSubagent={onOpenSubagent}
        />
      </div>
    );
  }
  return (
    <Loaded path={path} liveness={liveness} label={label} onOpenSubagent={onOpenSubagent} />
  );
}

function Loaded({
  path,
  liveness,
  label,
  onOpenSubagent,
}: {
  path: string;
  liveness: Liveness;
  label: string;
  onOpenSubagent: OpenSubagent;
}) {
  const live = liveness.state === "running";
  const page = useClaudeTranscriptMessages(path, true, live);
  const density = useFilters((f) => f.transcriptDensity);
  const setDensity = useFilters((f) => f.setTranscriptDensity);

  const messages = page.data?.messages;
  const truncated = page.data?.truncated ?? false;
  // Read by "Copy turn" on click. Updated after commit, never in render.
  const holder = useRef<readonly TranscriptMessage[]>([]);
  useLayoutEffect(() => {
    holder.current = messages ?? [];
  }, [messages]);

  const onLoadFullText = useCallback<LoadFullText>(
    (a) => claudeTranscriptBlockText(path, a.messageId, a.index),
    [path],
  );
  const streaming = transcriptStreaming(liveness);
  const env = useMemo<TerminalEnv>(
    () => ({
      liveness,
      density: density === "compact" ? "compact" : "comfortable",
      onLoadFullText,
      onOpenSubagent,
      messages: () => holder.current,
    }),
    [liveness, density, onLoadFullText, onOpenSubagent],
  );
  const footers = useMemo(
    () => turnFooters(messages ?? [], streaming === true),
    [messages, streaming],
  );
  const tasks = useMemo(
    () => deriveTaskChecklist(messages ?? [], { truncated }),
    [messages, truncated],
  );
  const renderMessage = useCallback(
    (m: TranscriptMessage) => (
      <TerminalMessage
        message={m}
        footer={footers.get(m.id)}
        env={env}
        tasks={holdsTaskCall(m) ? tasks : undefined}
      />
    ),
    [footers, env, tasks],
  );

  if (page.isError) {
    // BEFORE the empty arm (#846): no messages on a rejection is not a
    // transcript with nothing in it.
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        Could not read its transcript
        {errorMessage(page.error) ? ` (${errorMessage(page.error)})` : ""}. This is not the same
        as the session having said nothing.
      </p>
    );
  }
  if (page.data === undefined) {
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        Reading its transcript…
      </p>
    );
  }
  const p = page.data;
  if (p.messages.length === 0) {
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        {p.truncated
          ? `No conversation in the last ${formatBytes(p.bytes_read)} of a ${formatBytes(p.file_bytes)} transcript.`
          : "Its transcript holds no conversation to show."}
      </p>
    );
  }
  return (
    <div className="@container flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs" style={{ color: palette.muted }}>
        <p data-testid="transcript-read-status">
          {live
            ? `Following. Last read at ${clockTime(page.dataUpdatedAt)}.`
            : `Read at ${clockTime(page.dataUpdatedAt)}.`}
        </p>
        {p.truncated ? (
          <p data-testid="transcript-truncated">
            The last {p.messages.length.toLocaleString()} message
            {p.messages.length === 1 ? "" : "s"} of a {formatBytes(p.file_bytes)} transcript.
            Earlier exchanges are not shown.
          </p>
        ) : null}
        <DensityToggle value={env.density} onChange={setDensity} />
      </div>
      {tasks.tasks.length > 0 ? (
        // Narrow panes (the session detail) fold the checklist above the
        // transcript; wide ones pin it beside it. A container query, not
        // the viewport: the same window holds both hosts.
        <details
          className="rounded border p-2 @2xl:hidden"
          style={{ background: palette.surface, borderColor: palette.border }}
          data-testid="task-checklist-folded"
        >
          <summary className="cursor-pointer text-xs" style={{ color: palette.text }}>
            Tasks
          </summary>
          <TaskChecklist checklist={tasks} variant="terminal" />
        </details>
      ) : null}
      <div className="flex min-h-0 flex-1 gap-2">
        <div
          className="min-h-0 min-w-0 flex-1 rounded border"
          style={{ background: palette.ground, borderColor: palette.border }}
        >
          <TranscriptViewer
            messages={p.messages}
            renderMessage={renderMessage}
            streaming={streaming}
            label={label}
          />
        </div>
        {tasks.tasks.length > 0 ? (
          <aside
            className="hidden w-64 shrink-0 overflow-y-auto rounded border p-2 @2xl:block"
            style={{ background: palette.surface, borderColor: palette.border }}
          >
            <TaskChecklist checklist={tasks} variant="terminal" />
          </aside>
        ) : null}
      </div>
    </div>
  );
}

function holdsTaskCall(m: TranscriptMessage): boolean {
  return m.blocks.some(
    (b) =>
      b.kind === "tool_call" &&
      (b.args.tool === "task_create" ||
        b.args.tool === "task_update" ||
        b.args.tool === "task_get" ||
        b.args.tool === "task_list"),
  );
}

function DensityToggle({
  value,
  onChange,
}: {
  value: TranscriptDensity;
  onChange: (d: TranscriptDensity) => void;
}) {
  return (
    <div role="group" aria-label="Transcript density" className="ml-auto flex gap-1">
      {(["comfortable", "compact"] as const).map((d) => (
        <button
          key={d}
          type="button"
          aria-pressed={value === d}
          onClick={() => onChange(d)}
          className="rounded border px-2 py-0.5 capitalize focus-visible:outline focus-visible:outline-2"
          style={{
            borderColor: palette.border,
            background: value === d ? palette.userBand : "transparent",
            color: value === d ? palette.text : palette.muted,
          }}
        >
          {d}
        </button>
      ))}
    </div>
  );
}

function clockTime(ms: number): string {
  const at = new Date(ms);
  if (ms === 0 || Number.isNaN(at.getTime())) return "an unknown time";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(at.getHours())}:${p(at.getMinutes())}:${p(at.getSeconds())}`;
}

/// Bytes as KB or MB, whichever reads better. The same rule as the
/// session detail's `formatKb`.
function formatBytes(bytes: number): string {
  return bytes >= 1024 * 1024
    ? `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    : `${Math.round(bytes / 1024).toLocaleString()} KB`;
}
