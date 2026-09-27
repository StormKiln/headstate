import type { ReactNode } from "react";
import type { ClaudeToolArgs, Liveness } from "../../types/pr";
import type { TranscriptSubagent, TranscriptToolOutput } from "../../types/transcript";
import { ClippedText } from "./ClippedText";
import { DiffView } from "./DiffView";
import { changeFromArgs, creationFromWrite, linesOf } from "./diff";
import { Fold } from "./Fold";
import { MonoOutput, ProseOutput } from "./output";
import { palette } from "./palette";
import {
  bashOutcome,
  callState,
  countLabel,
  formatDuration,
  isClipped,
  mcpName,
  readSpan,
  searchSummary,
  type CallState,
} from "./summary";
import { taskCallRefused, taskIdOfCall, taskStatusWords, type TaskListState } from "./tasks";
import type { LoadFullText, OpenSubagent, ToolCallBlock, ToolVariant } from "./types";

/// One tool call, rendered the way Claude Code shows it (#1483).
///
/// Presentational. Both renderers -- desktop terminal (#1480) and phone
/// bubbles (#1481) -- draw calls with this and pick a `variant`. What it
/// needs from outside comes in as props: whether the session is live
/// (`liveness`), how long the call took (`durationMs`, from the call's
/// and result's timestamps -- see `durationBetween`), and the two
/// callbacks that reach past the page (full text, subagent transcript).
///
/// Every call ends in exactly one of the states `callState` names, and
/// each renders its own sentence: "still running" is only said of a
/// session that IS running.
export function ToolCall({
  call,
  variant,
  liveness,
  durationMs = null,
  onLoadFullText,
  onOpenSubagent,
  tasks,
}: {
  call: ToolCallBlock;
  variant: ToolVariant;
  liveness: Liveness;
  /// `null` when it could not be measured -- not shown, never "0 ms".
  durationMs?: number | null;
  onLoadFullText?: LoadFullText;
  onOpenSubagent?: OpenSubagent;
  /// The session's task list (`deriveTaskChecklist`), so a `TaskUpdate`
  /// row -- which names its task only by id -- can say which task.
  tasks?: TaskListState;
}) {
  const state = callState(call, liveness);
  const result = state.state === "paired" ? state.result : null;
  const ctx: Ctx = { call, result, variant, onLoadFullText, onOpenSubagent, tasks };
  const view = render(call.args, ctx);
  const failed = result?.is_error === true;

  return (
    <div
      role="group"
      aria-label={`${view.title}${view.summary ? `: ${view.summary}` : ""}`}
      className={variant === "terminal" ? "font-mono text-[12px]" : "text-[13px]"}
      style={{ color: palette.text }}
      data-state={state.state}
    >
      <div className="flex flex-wrap items-baseline gap-x-1.5">
        <span
          aria-hidden
          style={{
            color:
              state.state === "paired" ? (failed ? palette.error : palette.ok) : palette.muted,
          }}
        >
          ⏺
        </span>
        <span className="font-semibold">{view.title}</span>
        {view.summary ? (
          <span className="min-w-0 break-all" style={{ color: palette.muted }}>
            {view.summary}
          </span>
        ) : null}
        {view.status}
        {durationMs !== null ? (
          <span className="text-[11px]" style={{ color: palette.muted }}>
            {formatDuration(durationMs)}
          </span>
        ) : null}
      </div>
      <div className={variant === "terminal" ? "ml-4" : "ml-3"}>
        {view.body}
        {result && result.images.length > 0 ? <Images result={result} /> : null}
        <StateNote state={state} />
      </div>
    </div>
  );
}

interface Ctx {
  call: ToolCallBlock;
  result: TranscriptToolOutput | null;
  variant: ToolVariant;
  onLoadFullText?: LoadFullText;
  onOpenSubagent?: OpenSubagent;
  tasks?: TaskListState;
}

interface View {
  title: string;
  /// Plain text: it is also the group's accessible name.
  summary: string | null;
  status?: ReactNode;
  body: ReactNode;
}

function render(args: ClaudeToolArgs, ctx: Ctx): View {
  switch (args.tool) {
    case "bash":
      return bash(args, ctx);
    case "read":
      return read(args, ctx);
    case "edit":
    case "multi_edit":
      return edit(args, ctx);
    case "write":
      return write(args, ctx);
    case "grep":
    case "glob":
      return search(args, ctx);
    case "task":
      return task(args, ctx);
    case "todo_write":
      return todos(args, ctx);
    case "web_fetch":
    case "web_search":
      return web(args, ctx);
    case "task_create":
    case "task_update":
    case "task_get":
    case "task_list":
      return taskCall(args, ctx);
    case "other":
    case "none":
      return other(args, ctx);
  }
}

/// A chip of text beside the title. Colour AND words, never colour
/// alone.
function Chip({ tone, children }: { tone: "error" | "ok" | "muted"; children: ReactNode }) {
  const color = tone === "error" ? palette.error : tone === "ok" ? palette.ok : palette.muted;
  return (
    <span className="text-[11px]" style={{ color }}>
      {children}
    </span>
  );
}

/// The result's text in a fold: "N lines ▸", opened in place or in a
/// sheet, with the clip stated and the full text one button away.
function ResultFold({
  ctx,
  label,
  title,
  prose = false,
  defaultOpen = false,
}: {
  ctx: Ctx;
  label: string;
  title: string;
  prose?: boolean;
  defaultOpen?: boolean;
}) {
  const r = ctx.result;
  if (!r) return null;
  const error = r.is_error === true;
  if (r.text === "" && !isClipped(r.clip)) {
    // Measured, and empty: said, not folded behind "0 lines".
    return (
      <p className="text-[11px]" style={{ color: palette.muted }}>
        (no output)
      </p>
    );
  }
  const count = countLabel(linesOf(r.text).length, "line", isClipped(r.clip));
  return (
    <Fold label={label} count={count} variant={ctx.variant} title={title} defaultOpen={defaultOpen}>
      <ClippedText
        text={r.text}
        clip={r.clip}
        address={{ messageId: r.message_id, index: r.index, offset: r.offset }}
        onLoadFullText={ctx.onLoadFullText}
      >
        {(t) => (prose && !error ? <ProseOutput text={t} /> : <MonoOutput text={t} error={error} />)}
      </ClippedText>
    </Fold>
  );
}

/// An error result shown whole, in red, with the word beside it.
function ErrorText({ ctx }: { ctx: Ctx }) {
  const r = ctx.result;
  if (!r) return null;
  return (
    <ClippedText
      text={r.text}
      clip={r.clip}
      address={{ messageId: r.message_id, index: r.index, offset: r.offset }}
      onLoadFullText={ctx.onLoadFullText}
    >
      {(t) => <MonoOutput text={t} error />}
    </ClippedText>
  );
}

function firstLine(s: string): string {
  return linesOf(s)[0] ?? "";
}

function bash(args: Extract<ClaudeToolArgs, { tool: "bash" }>, ctx: Ctx): View {
  const r = ctx.result;
  const outcome = r ? bashOutcome(r) : null;
  let status: ReactNode = null;
  if (outcome?.kind === "error") {
    status = <Chip tone="error">{outcome.code !== null ? `exit ${outcome.code}` : "error"}</Chip>;
  } else if (outcome?.kind === "completed") {
    status = <Chip tone="ok">completed</Chip>;
  }
  return {
    title: "Bash",
    summary: args.description ?? firstLine(args.command),
    status,
    body: (
      <>
        <pre
          className="mt-0.5 whitespace-pre-wrap break-words font-mono text-[11px]"
          style={{ color: palette.text }}
        >
          <span aria-hidden style={{ color: palette.muted }}>
            ${" "}
          </span>
          {args.command}
        </pre>
        {args.truncated ? <Clipped what="command" /> : null}
        <ResultFold ctx={ctx} label="Output" title={args.description ?? "Bash output"} />
      </>
    ),
  };
}

function Clipped({ what }: { what: string }) {
  return (
    <p className="text-[11px]" style={{ color: palette.muted }}>
      The {what} was clipped.
    </p>
  );
}

function read(args: Extract<ClaudeToolArgs, { tool: "read" }>, ctx: Ctx): View {
  const r = ctx.result;
  const span = r && r.is_error !== true ? readSpan(r.text) : null;
  let detail = "";
  if (span) {
    const clipped = isClipped(r?.clip ?? null);
    detail = ` (${countLabel(span.count, "line", clipped)})`;
  } else if (args.offset !== null || args.limit !== null) {
    // What was ASKED for, worded as a request: the result did not say
    // what came back.
    detail = ` (from line ${args.offset ?? 1}${args.limit !== null ? `, up to ${args.limit} lines` : ""})`;
  }
  return {
    title: "Read",
    summary: `${args.file_path}${detail}`,
    status: r?.is_error === true ? <Chip tone="error">error</Chip> : null,
    body:
      r?.is_error === true ? (
        <ErrorText ctx={ctx} />
      ) : (
        <ResultFold ctx={ctx} label="Contents" title={args.file_path} />
      ),
  };
}

function edit(args: Extract<ClaudeToolArgs, { tool: "edit" | "multi_edit" }>, ctx: Ctx): View {
  const r = ctx.result;
  const fromArgs = changeFromArgs(args);
  const clipped =
    args.tool === "edit" ? args.truncated : args.edits.some((e) => e.truncated);
  const title = args.tool === "edit" ? "Edit" : "MultiEdit";
  let body: ReactNode;
  if (r?.is_error === true) {
    // Refused: the error is the news. The attempted change stays one
    // click away and says it was not applied.
    body = (
      <>
        <ErrorText ctx={ctx} />
        {fromArgs ? (
          <DiffView
            change={fromArgs}
            variant={ctx.variant}
            defaultOpen={false}
            note="Not applied: the tool reported an error."
          />
        ) : null}
      </>
    );
  } else if (r?.change) {
    body = <DiffView change={r.change} variant={ctx.variant} />;
  } else if (fromArgs) {
    body = (
      <DiffView
        change={fromArgs}
        variant={ctx.variant}
        note={clipped ? "The edit's text was clipped." : undefined}
      />
    );
  }
  return {
    title,
    summary:
      args.tool === "multi_edit"
        ? `${args.file_path} (${countLabel(args.edits.length + args.edits_omitted, "edit", false)})`
        : args.file_path + (args.replace_all ? " (every occurrence)" : ""),
    status: r?.is_error === true ? <Chip tone="error">error</Chip> : null,
    body,
  };
}

function write(args: Extract<ClaudeToolArgs, { tool: "write" }>, ctx: Ctx): View {
  const r = ctx.result;
  const n = linesOf(args.content).length;
  const lines = countLabel(n, "line", args.truncated);
  const created = r?.change?.created === true;
  let body: ReactNode;
  if (r?.is_error === true) {
    body = <ErrorText ctx={ctx} />;
  } else if (r?.change && r.change.hunks.length > 0) {
    body = <DiffView change={r.change} variant={ctx.variant} />;
  } else if (created) {
    body = (
      <DiffView
        change={creationFromWrite(args.file_path, args.content)}
        variant={ctx.variant}
        countsKnown={!args.truncated}
        note={args.truncated ? "The file's content was clipped." : undefined}
      />
    );
  } else {
    // No recorded change: whether this created or replaced the file is
    // not known, so it is not drawn as all-added.
    body = (
      <Fold label="Content" count={lines} variant={ctx.variant} title={args.file_path}>
        <MonoOutput text={args.content} />
        {args.truncated ? <Clipped what="content" /> : null}
      </Fold>
    );
  }
  return {
    title: "Write",
    summary: `${args.file_path} (${created ? "new file, " : ""}${lines})`,
    status: r?.is_error === true ? <Chip tone="error">error</Chip> : null,
    body,
  };
}

function search(args: Extract<ClaudeToolArgs, { tool: "grep" | "glob" }>, ctx: Ctx): View {
  const r = ctx.result;
  const where = args.path ? ` in ${args.path}` : "";
  const title = args.tool === "grep" ? "Grep" : "Glob";
  if (!r || r.is_error === true) {
    return {
      title,
      summary: `${args.pattern}${where}`,
      status: r ? <Chip tone="error">error</Chip> : null,
      body: r ? <ErrorText ctx={ctx} /> : null,
    };
  }
  const s = searchSummary(
    args.tool,
    args.tool === "grep" ? args.output_mode : null,
    r.text,
    isClipped(r.clip),
  );
  const listLabel = args.tool === "grep" && args.output_mode === "content" ? "Lines" : "Files";
  return {
    title,
    summary: `${args.pattern}${where}`,
    status: <Chip tone="muted">{s.label}</Chip>,
    body:
      s.items.length > 0 ? (
        <Fold
          label={listLabel}
          count={countLabel(s.items.length, listLabel === "Lines" ? "line" : "file", isClipped(r.clip))}
          variant={ctx.variant}
          title={`${title} ${args.pattern}`}
        >
          <MonoOutput text={s.items.join("\n")} />
        </Fold>
      ) : null,
  };
}

function task(args: Extract<ClaudeToolArgs, { tool: "task" }>, ctx: Ctx): View {
  const r = ctx.result;
  const sub = r?.subagent ?? null;
  return {
    title: ctx.call.name,
    summary: [args.description ?? "subagent", args.subagent_type ? `(${args.subagent_type})` : ""]
      .filter(Boolean)
      .join(" "),
    status: r?.is_error === true ? (
      <Chip tone="error">error</Chip>
    ) : sub?.status ? (
      <Chip tone="muted">{sub.status.replace(/_/g, " ")}</Chip>
    ) : null,
    body: (
      <>
        {sub ? <SubagentLink sub={sub} onOpen={ctx.onOpenSubagent} /> : null}
        <Fold
          label="Prompt"
          count={countLabel(linesOf(args.prompt).length, "line", args.truncated)}
          variant={ctx.variant}
          title="Subagent prompt"
        >
          <ProseOutput text={args.prompt} />
          {args.truncated ? <Clipped what="prompt" /> : null}
        </Fold>
        <ResultFold ctx={ctx} label="Report" title={args.description ?? "Subagent report"} prose />
      </>
    ),
  };
}

/// The link to a subagent's own transcript, said as honestly as
/// `transcript_found` allows: found, looked for and missing, or not
/// checked -- which is not "missing".
export function SubagentLink({ sub, onOpen }: { sub: TranscriptSubagent; onOpen?: OpenSubagent }) {
  if (sub.transcript_path === null) {
    return (
      <p className="text-[11px]" style={{ color: palette.muted }}>
        No transcript is linked to this subagent.
      </p>
    );
  }
  if (sub.transcript_found === false) {
    return (
      <p className="text-[11px]" style={{ color: palette.warn }}>
        This subagent's transcript file was not found.
      </p>
    );
  }
  if (!onOpen) return null;
  return (
    <p className="text-[11px]">
      <button
        type="button"
        className="underline focus-visible:outline focus-visible:outline-2"
        style={{ color: palette.link }}
        aria-label={`Open the transcript of subagent ${sub.agent_type ?? sub.agent_id}`}
        onClick={() => onOpen(sub)}
      >
        Open subagent transcript
      </button>
      {sub.transcript_found === null ? (
        <span style={{ color: palette.muted }}> (not checked whether its file exists)</span>
      ) : null}
    </p>
  );
}

const TODO_MARK: Record<string, string> = {
  completed: "☑",
  in_progress: "◐",
  pending: "☐",
};

function todoStatusWords(status: string | null): string {
  if (status === null) return "status not recorded";
  if (status === "completed") return "done";
  return status.replace(/_/g, " ");
}

function todos(args: Extract<ClaudeToolArgs, { tool: "todo_write" }>, ctx: Ctx): View {
  const total = args.todos.length + args.todos_omitted;
  const done = args.todos.filter((t) => t.status === "completed").length;
  return {
    title: "Todos",
    summary: `${args.todos_omitted > 0 ? "at least " : ""}${done} of ${total} done`,
    status: ctx.result?.is_error === true ? <Chip tone="error">error</Chip> : null,
    body: (
      <>
        <ul className="mt-0.5 space-y-0.5" aria-label="Todo list">
          {args.todos.map((t, i) => (
            <li
              key={i}
              className="flex gap-1.5"
              style={{
                color: t.status === "completed" ? palette.muted : palette.text,
              }}
            >
              <span aria-hidden>{t.status ? (TODO_MARK[t.status] ?? "?") : "?"}</span>
              <span className="sr-only">{todoStatusWords(t.status)}: </span>
              <span
                className={t.status === "completed" ? "line-through" : undefined}
                style={t.status === "in_progress" ? { fontWeight: 600 } : undefined}
              >
                {t.status === "in_progress" && t.active_form ? t.active_form : t.content}
                {t.truncated ? " …" : ""}
              </span>
              {t.status !== null && !(t.status in TODO_MARK) ? (
                <span style={{ color: palette.muted }}>({todoStatusWords(t.status)})</span>
              ) : null}
            </li>
          ))}
        </ul>
        {args.todos_omitted > 0 ? (
          <p className="text-[11px]" style={{ color: palette.muted }}>
            {args.todos_omitted.toLocaleString()} more item
            {args.todos_omitted === 1 ? " is" : "s are"} not shown.
          </p>
        ) : null}
        {ctx.result?.is_error === true ? <ErrorText ctx={ctx} /> : null}
      </>
    ),
  };
}

/// A task-list call as one compact row (#1504): "#3 → in progress ·
/// Write the parser". An update names its task only by id, so the name
/// comes from the session's task list when the host passed one.
function taskCall(
  args: Extract<ClaudeToolArgs, { tool: "task_create" | "task_update" | "task_get" | "task_list" }>,
  ctx: Ctx,
): View {
  const r = ctx.result;
  const id = taskIdOfCall(ctx.call);
  const refused = taskCallRefused(ctx.call);
  const known = id !== null ? ctx.tasks?.tasks.find((t) => t.id === id) : undefined;
  const ref = id !== null ? `#${id}` : "task";
  // A refusal recorded only as `success: false` says so in words: it
  // carried no error flag, so "error" would overstate what was recorded.
  const status: ReactNode = refused ? (
    <Chip tone="error">{r?.is_error === true ? "error" : "not applied"}</Chip>
  ) : null;
  const errorBody = refused ? <ErrorText ctx={ctx} /> : null;

  switch (args.tool) {
    case "task_create":
      return {
        title: "TaskCreate",
        summary: `${id !== null ? `#${id} ` : ""}${args.subject}${args.truncated ? " …" : ""}`,
        status,
        body: (
          <>
            {args.description ? (
              <Fold
                label="Description"
                count={countLabel(linesOf(args.description).length, "line", args.truncated)}
                variant={ctx.variant}
                title={args.subject}
              >
                <ProseOutput text={args.description} />
              </Fold>
            ) : null}
            {errorBody}
          </>
        ),
      };
    case "task_update": {
      const name = args.subject ?? known?.subject ?? null;
      const extra = args.fields.filter(
        (f) => f !== "status" && f !== "subject" && f !== "activeForm",
      );
      const change =
        args.status !== null
          ? `→ ${taskStatusWords(args.status)}`
          : args.subject !== null
            ? "renamed"
            : "updated";
      return {
        title: "TaskUpdate",
        summary: `${ref} ${change}${name !== null ? ` · ${name}` : ""}`,
        status,
        body: (
          <>
            {args.task_id === null ? (
              <p className="text-[11px]" style={{ color: palette.muted }}>
                No task was named.
              </p>
            ) : null}
            {extra.length > 0 ? (
              <p className="text-[11px]" style={{ color: palette.muted }}>
                Also set: {extra.join(", ")}
              </p>
            ) : null}
            {errorBody}
          </>
        ),
      };
    }
    case "task_get":
      return {
        title: "TaskGet",
        summary: `${ref}${known?.subject ? ` · ${known.subject}` : ""}`,
        status,
        body: refused ? errorBody : <ResultFold ctx={ctx} label="Task" title={ref} />,
      };
    case "task_list":
      return {
        title: "TaskList",
        summary: null,
        status,
        body: refused ? errorBody : <ResultFold ctx={ctx} label="Tasks" title="Tasks" />,
      };
  }
}

function web(args: Extract<ClaudeToolArgs, { tool: "web_fetch" | "web_search" }>, ctx: Ctx): View {
  const r = ctx.result;
  const fetch = args.tool === "web_fetch";
  return {
    title: fetch ? "WebFetch" : "WebSearch",
    summary: fetch ? args.url : args.query,
    status: r?.is_error === true ? <Chip tone="error">error</Chip> : null,
    body: (
      <>
        {fetch && args.prompt ? (
          <p className="text-[11px]" style={{ color: palette.muted }}>
            {args.prompt}
            {args.truncated ? " …" : ""}
          </p>
        ) : null}
        {!fetch && args.truncated ? <Clipped what="query" /> : null}
        {r?.is_error === true ? (
          <ErrorText ctx={ctx} />
        ) : (
          <ResultFold
            ctx={ctx}
            label="Result"
            title={fetch ? args.url : args.query}
            prose
          />
        )}
      </>
    ),
  };
}

function other(args: Extract<ClaudeToolArgs, { tool: "other" | "none" }>, ctx: Ctx): View {
  const mcp = mcpName(ctx.call.name);
  const r = ctx.result;
  return {
    title: mcp ? `${mcp.server} › ${mcp.tool}` : ctx.call.name,
    summary: null,
    status: (
      <>
        {mcp ? <Chip tone="muted">MCP</Chip> : null}
        {r?.is_error === true ? <Chip tone="error">error</Chip> : null}
      </>
    ),
    body: (
      <>
        {/* NAMED, never dropped: the keys say whether Headstate is
            behind; the values are the blob that tells a reader nothing.
            No input at all is a different sentence -- absent is not
            zero. */}
        <p className="text-[11px]" style={{ color: palette.muted }}>
          {args.tool === "none"
            ? "No arguments were recorded."
            : args.keys.length === 0
              ? "Called with no arguments."
              : `Arguments: ${args.keys.join(", ")}`}
        </p>
        <ResultFold ctx={ctx} label="Result" title={ctx.call.name} />
      </>
    ),
  };
}

function Images({ result }: { result: TranscriptToolOutput }) {
  return (
    <ul className="text-[11px]" style={{ color: palette.muted }}>
      {result.images.map((im, i) => (
        <li key={i}>
          [image{im.media_type ? `, ${im.media_type}` : ""}
          {im.width !== null && im.height !== null ? `, ${im.width}×${im.height}` : ""}
          {im.approx_bytes !== null ? `, about ${Math.max(1, Math.round(im.approx_bytes / 1024))} KB` : ""}]
        </li>
      ))}
    </ul>
  );
}

/// The sentence for a call with no result. One per state, and none of
/// them borrows another's words.
function StateNote({ state }: { state: CallState }) {
  switch (state.state) {
    case "paired":
      return null;
    case "running":
      return (
        <p className="text-[11px]" style={{ color: palette.muted }}>
          Running…
        </p>
      );
    case "not_recorded":
      return (
        <p className="text-[11px]" style={{ color: palette.warn }}>
          No result was recorded, and the session is no longer running.
        </p>
      );
    case "unknown":
      return (
        <p className="text-[11px]" style={{ color: palette.muted }}>
          No result was recorded. Whether it is still running could not be determined.
        </p>
      );
    case "unkeyed":
      return (
        <p className="text-[11px]" style={{ color: palette.muted }}>
          This call has no id, so no result can be matched to it.
        </p>
      );
  }
}
