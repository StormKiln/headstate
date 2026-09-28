/// The transcript's session header (#1485): what the session is doing
/// now, what it has cost so far, where it ran, and what can be done
/// about it. One component for both hosts; the layout (`variant`) and
/// the build (`IS_MOBILE_BUILD`) each decide what they own.
///
/// - `variant` is LAYOUT: the phone abbreviates the folder.
/// - `IS_MOBILE_BUILD` is CAPABILITY: the phone build cannot open a
///   terminal (`claude_launch_*` are `Class::Local`), so it offers the
///   resume command to copy instead of Resume and Claudify.
///
/// The wording of each state lives in `header.ts`, where each is tested
/// apart from the layout.
///
/// # Reserved for 7.10
///
/// `reply` is the slot a "Reply" affordance will fill once sending into
/// a session is real (#1490, #1491). Nothing fills it in 7.9, and the
/// header makes no claim about whether a session can be sent to: the
/// owner's decision is that nothing is displayed until the capability
/// exists.

import { useState, type ReactNode } from "react";
import { Circle, GitBranch, Folder } from "lucide-react";
import { toast } from "sonner";
import {
  claudeLaunchSession,
  claudeLaunchSessionPreview,
  claudeLaunchWorktree,
  claudeLaunchWorktreePreview,
  claudifyCommand,
  type LaunchPreview,
  type LaunchTerms,
} from "@/api/tauri";
import { useClaudeSessionUsage, useUiPrefs, useWorktrees } from "@/api/hooks";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { canClaudify, sessionWorktree } from "@/lib/worktrees";
import type { ClaudeSession, ClaudeSessionDetail, ClaudeUsage } from "@/types/pr";
import { LaunchTermsPicker } from "../LaunchTermsPicker";
import { MaskedText } from "../MaskedText";
import { errorMessage } from "../QueryError";
import {
  abbreviatePath,
  compactCount,
  elapsedView,
  livenessView,
  waitingView,
  type LivenessTone,
  type WaitingTone,
} from "./header";

export function TranscriptHeader({
  session: s,
  detail: d,
  now,
  variant,
  withheld = false,
  subagentRollup,
  reply,
}: {
  /// The list row: name, folder, branch, context pressure, activity.
  session: ClaudeSession;
  /// The detail read: liveness and waiting derived on THIS read, the
  /// resume command, the transcript's path and state.
  detail: ClaudeSessionDetail;
  /// The list poll's time. Never `Date.now()`, the page's rule.
  now: number;
  variant: "desktop" | "phone";
  /// This phone may not read transcripts, so the opening prompt was
  /// withheld rather than absent.
  withheld?: boolean;
  /// The subagent rollup, opened in place. The host renders it because
  /// the host owns it; absent when there is none to open.
  subagentRollup?: ReactNode;
  /// Reserved for 7.10's Reply. See the module docs.
  reply?: ReactNode;
}) {
  const noTranscript =
    d.transcript_path === null ||
    d.transcript_state.state === "gone" ||
    d.transcript_state.state === "not-recorded";
  const live = livenessView(d.liveness, noTranscript);
  const waiting = waitingView(d.waiting, clockTime);
  const running = d.liveness.state === "running";
  const elapsed = elapsedView(d.first_seen_at, s.last_activity_at, running, now);
  const [rollupOpen, setRollupOpen] = useState(false);
  const subagents = d.subagents.length;

  return (
    <section
      aria-label="Session status"
      data-testid="transcript-header"
      className="flex flex-col gap-1.5 rounded-md border border-[#30363d] bg-[#161b22] p-2 text-xs"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <LivenessLine tone={live.tone} label={live.label} />
        <WaitingLine tone={waiting.tone} label={waiting.label} title={waiting.title} />
      </div>
      {/* Mounted with the header, so the session STARTING to wait is
          announced (#1489): a region that arrives with its text is often
          not read. Polite, and only the present tense -- "waited earlier"
          is history, not news. */}
      <span role="status" className="sr-only">
        {waiting.tone === "now" ? waiting.label : ""}
      </span>
      {live.detail ? (
        <p className="text-[11px] text-[#8b949e]" data-testid="transcript-header-liveness-why">
          {live.detail}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[#8b949e]">
        {noTranscript ? null : (
          <UsageFacts
            path={d.transcript_path}
            running={running}
          />
        )}
        <ContextFact pressure={s.context_pressure} />
        {elapsed ? <span data-testid="transcript-header-elapsed">{elapsed}</span> : null}
        <span className="flex min-w-0 items-center gap-1">
          <GitBranch className="h-3 w-3 shrink-0" aria-hidden="true" />
          {s.git_branch ? (
            <span className="min-w-0 font-mono [overflow-wrap:anywhere]">{s.git_branch}</span>
          ) : (
            <span>no branch recorded</span>
          )}
        </span>
        <span className="flex min-w-0 items-center gap-1" title={s.cwd ?? undefined}>
          <Folder className="h-3 w-3 shrink-0" aria-hidden="true" />
          {s.cwd ? (
            <span className="min-w-0 font-mono [overflow-wrap:anywhere]" data-testid="transcript-header-cwd">
              {variant === "phone" ? abbreviatePath(s.cwd) : s.cwd}
            </span>
          ) : (
            <span>no folder recorded</span>
          )}
        </span>
      </div>
      {withheld ? (
        <p className="text-[11px] text-[#8b949e]" data-testid="transcript-header-withheld">
          Transcripts are turned off for this phone on the desktop, so its opening prompt is not
          shown.
        </p>
      ) : s.opening_prompt ? (
        <p className="truncate text-[11px] text-[#8b949e]" title={s.opening_prompt}>
          Asked: <MaskedText text={s.opening_prompt} />
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        {IS_MOBILE_BUILD ? (
          <PhoneActions detail={d} />
        ) : (
          <DesktopActions session={s} detail={d} />
        )}
        {subagents > 0 && subagentRollup ? (
          <button
            type="button"
            aria-expanded={rollupOpen}
            onClick={() => setRollupOpen((o) => !o)}
            className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-[#e6edf3] hover:bg-[#30363d]"
          >
            {rollupOpen ? "Hide subagents" : `Subagents (${subagents.toLocaleString()})`}
          </button>
        ) : null}
        {reply ? <div className="ml-auto">{reply}</div> : null}
      </div>
      {rollupOpen && subagentRollup ? (
        <div data-testid="transcript-header-rollup">{subagentRollup}</div>
      ) : null}
    </section>
  );
}

/// Three renderings, not one with a tint: a filled dot, a hollow one,
/// and a dashed one for "could not tell" -- never the stopped glyph.
function LivenessLine({ tone, label }: { tone: LivenessTone; label: string }) {
  if (tone === "running") {
    return (
      <span className="flex items-center gap-1 font-medium text-[#3fb950]" data-tone={tone}>
        <Circle className="h-2.5 w-2.5 fill-current" aria-hidden="true" />
        {label}
      </span>
    );
  }
  if (tone === "stopped") {
    return (
      <span className="flex items-center gap-1 text-[#8b949e]" data-tone={tone}>
        <Circle className="h-2.5 w-2.5" aria-hidden="true" />
        {label}
      </span>
    );
  }
  return (
    <span className="flex items-center gap-1 text-[#8b949e]" data-tone={tone}>
      <span
        className="h-2.5 w-2.5 rounded-full border border-dashed border-[#8b949e]"
        aria-hidden="true"
      />
      {label}
    </span>
  );
}

/// Present tense in amber (and announced, by the header), past tense muted; "not
/// recorded" and "not waiting" in words of their own.
function WaitingLine({
  tone,
  label,
  title,
}: {
  tone: WaitingTone;
  label: string;
  title: string | null;
}) {
  if (tone === "now") {
    return (
      <span
        className="flex items-center gap-1 rounded bg-[#322e22] px-1.5 py-0.5 font-semibold text-[#d29922]"
        title={title ?? undefined}
        data-tone={tone}
      >
        <Circle className="h-2.5 w-2.5 fill-current" aria-hidden="true" />
        {label}
      </span>
    );
  }
  return (
    <span className="text-[#8b949e]" title={title ?? undefined} data-tone={tone}>
      {label}
    </span>
  );
}

/// Model, tokens and cost so far, from the same usage read the detail's
/// "How much work it did" uses (`useClaudeSessionUsage`), re-read while
/// the session runs.
///
/// While it runs, or when the read was short, every figure is a floor
/// and says "at least". Four token counters, never one total.
function UsageFacts({ path, running }: { path: string | null; running: boolean }) {
  const usage = useClaudeSessionUsage(path, running);
  if (usage.isError) {
    return (
      <span>
        Usage could not be read
        {errorMessage(usage.error) ? ` (${errorMessage(usage.error)})` : ""}
      </span>
    );
  }
  if (usage.data === undefined) return <span>Reading usage…</span>;
  return <UsageFigures usage={usage.data} running={running} />;
}

function UsageFigures({ usage: u, running }: { usage: ClaudeUsage; running: boolean }) {
  if (u.messages === 0) return <span>No token usage recorded</span>;
  const floor = running || u.truncated;
  const cost = u.recorded_cost;
  return (
    <>
      {u.models.length > 0 ? (
        <span data-testid="transcript-header-model">
          {u.models.length === 1 ? u.models[0].model : u.models.map((m) => m.model).join(", ")}
        </span>
      ) : null}
      <span
        data-testid="transcript-header-tokens"
        title={`input ${u.input_tokens.toLocaleString()}, output ${u.output_tokens.toLocaleString()}, cache read ${u.cache_read_tokens.toLocaleString()}, cache written ${u.cache_creation_tokens.toLocaleString()}`}
      >
        {floor ? "At least " : ""}
        {compactCount(u.input_tokens)} in · {compactCount(u.output_tokens)} out ·{" "}
        {compactCount(u.cache_read_tokens)} cache read · {compactCount(u.cache_creation_tokens)}{" "}
        cache written
      </span>
      <span data-testid="transcript-header-cost">
        {cost === null
          ? running
            ? "Cost not recorded yet"
            : "Cost not recorded"
          : `${floor || cost.has_unknown_model_cost ? "At least " : ""}${formatUsd(cost.total_cost_usd)} recorded by Claude Code`}
      </span>
    </>
  );
}

/// Context pressure, in its three states (#1065): the flag, a measured
/// no, and not recorded.
function ContextFact({ pressure }: { pressure: boolean | null }) {
  return (
    <span data-testid="transcript-header-context">
      {pressure === true
        ? "Context compacted repeatedly"
        : pressure === false
          ? "Context not compacted repeatedly"
          : "Context pressure not recorded"}
    </span>
  );
}

/// Resume in terminal and Claudify, on the desktop build.
///
/// Both reuse the launch commands the detail and the Worktrees page use,
/// and both show the exact argv before a terminal opens (#1214's rule),
/// through the same `LaunchTermsPicker`. With no terminal configured each
/// copies its command instead, as the detail's Resume does.
function DesktopActions({
  session: s,
  detail: d,
}: {
  session: ClaudeSession;
  detail: ClaudeSessionDetail;
}) {
  const { prefs } = useUiPrefs();
  const terminal = (prefs?.terminal_command ?? "").trim() !== "";
  const { data: repos } = useWorktrees();
  const match = sessionWorktree(s.cwd, s.git_branch, repos);
  const claudifiable = match !== null && canClaudify(match.worktree.safety);
  const [open, setOpen] = useState<"resume" | "claudify" | null>(null);
  const [terms, setTerms] = useState<LaunchTerms>({});
  const toggle = (which: "resume" | "claudify") => {
    setTerms({});
    setOpen((o) => (o === which ? null : which));
  };

  // A running session is not offered Resume: resuming it starts a second
  // copy. The liveness line above already says it is running.
  const resumable = d.liveness.state !== "running";

  return (
    <>
      {resumable ? (
        terminal ? (
          <button
            type="button"
            aria-expanded={open === "resume"}
            onClick={() => toggle("resume")}
            className="tap-target rounded-md bg-[#1f6feb] px-2 py-1 text-[#ffffff] hover:bg-[#316dca]"
          >
            Resume in terminal
          </button>
        ) : (
          <button
            type="button"
            onClick={() => copy(d.resume.command, "Resume command")}
            className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-[#e6edf3] hover:bg-[#30363d]"
          >
            Copy resume command
          </button>
        )
      ) : null}
      {claudifiable && match ? (
        terminal ? (
          <button
            type="button"
            aria-expanded={open === "claudify"}
            onClick={() => toggle("claudify")}
            className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-[#e6edf3] hover:bg-[#30363d]"
          >
            Claudify
          </button>
        ) : (
          <button
            type="button"
            onClick={() => {
              void claudifyCommand(match.repoPath, match.worktree.path, match.worktree.branch).then(
                ({ command }) => copy(command, "Claudify command"),
                (e: unknown) =>
                  toast.error("Could not build the command", { description: errorMessage(e) }),
              );
            }}
            className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-[#e6edf3] hover:bg-[#30363d]"
          >
            Copy Claudify command
          </button>
        )
      ) : null}
      {open === "resume" && resumable && terminal ? (
        <LaunchPanel
          caveats={[
            d.resume.caveat,
            d.liveness.state === "unknown"
              ? "It could not be established whether this session is running, so it may already be open somewhere. Resuming it then starts a second copy."
              : null,
          ]}
          terms={terms}
          onTerms={setTerms}
          preview={(t) => claudeLaunchSessionPreview(s.session_id, s.cwd ?? null, t)}
          start="Resume"
          onStart={() => {
            setOpen(null);
            void claudeLaunchSession(s.session_id, s.cwd ?? null, terms).then(
              () => toast.success("Opening the session in your terminal"),
              (e: unknown) =>
                toast.error("Could not open your terminal", {
                  description: errorMessage(e),
                  action: {
                    label: "Copy instead",
                    onClick: () => copy(d.resume.command, "Resume command"),
                  },
                }),
            );
          }}
        />
      ) : null}
      {open === "claudify" && claudifiable && match && terminal ? (
        <LaunchPanel
          caveats={[]}
          terms={terms}
          onTerms={setTerms}
          preview={(t) =>
            claudeLaunchWorktreePreview(match.repoPath, match.worktree.path, match.worktree.branch, t)
          }
          start="Claudify"
          onStart={() => {
            setOpen(null);
            void claudeLaunchWorktree(
              match.repoPath,
              match.worktree.path,
              match.worktree.branch,
              terms,
            ).then(
              () => toast.success("Opening Claude Code on its worktree in your terminal"),
              (e: unknown) =>
                toast.error("Could not open your terminal", { description: errorMessage(e) }),
            );
          }}
        />
      ) : null}
    </>
  );
}

/// The terms and the exact argv, then the button that runs it.
function LaunchPanel({
  caveats,
  terms,
  onTerms,
  preview,
  start,
  onStart,
}: {
  caveats: (string | null)[];
  terms: LaunchTerms;
  onTerms: (t: LaunchTerms) => void;
  preview: (t: LaunchTerms) => Promise<LaunchPreview>;
  start: string;
  onStart: () => void;
}) {
  return (
    <div className="basis-full rounded-md border border-[#30363d] bg-[#0d1117] p-2">
      {caveats
        .filter((c): c is string => c !== null)
        .map((c) => (
          <p key={c} className="mb-1 text-[11px] text-[#d29922]">
            {c}
          </p>
        ))}
      <LaunchTermsPicker terms={terms} onChange={onTerms} preview={preview} />
      <button
        type="button"
        onClick={onStart}
        className="tap-target mt-2 rounded-md bg-[#1f6feb] px-2 py-1 text-[#ffffff] hover:bg-[#316dca]"
      >
        {start}
      </button>
    </div>
  );
}

/// The phone build's one action: the resume command, to paste into a
/// terminal on the desktop. Not offered while the session runs, for the
/// reason the desktop does not offer Resume.
function PhoneActions({ detail: d }: { detail: ClaudeSessionDetail }) {
  if (d.liveness.state === "running") return null;
  return (
    <>
      <button
        type="button"
        onClick={() => copy(d.resume.command, "Resume command")}
        className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-[#e6edf3] hover:bg-[#30363d]"
      >
        Copy resume command
      </button>
      {d.liveness.state === "unknown" ? (
        <span className="basis-full text-[11px] text-[#8b949e]">
          It may already be open somewhere; resuming it then starts a second copy.
        </span>
      ) : null}
    </>
  );
}

function copy(value: string, what: string) {
  void copyText(value).then((failure) =>
    failure === null
      ? toast.success(`${what} copied to the clipboard`)
      : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
  );
}

/// A transcribed dollar figure: to the cent, but never `$0.00` for a
/// spend that was not zero. The detail's `formatUsd` rule.
function formatUsd(usd: number): string {
  if (usd > 0 && usd < 0.005) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

/// `HH:MM`, local, from a timestamp; the raw string when it does not
/// parse, rather than `NaN:NaN`.
function clockTime(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;
  return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
}
