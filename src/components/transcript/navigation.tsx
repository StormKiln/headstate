/// Transcript navigation's controls (#1484): the "while you were away"
/// card and the unread divider, the turn outline, find in the whole
/// session, what is shown, and export. The state behind them is
/// `useNavigation.ts`. Shared by both hosts, which lay them out their
/// own way: a side panel and `j`/`k` on the desktop, buttons and sheets
/// on the phone.
///
/// # Whole-session find
///
/// `claude_transcript_find` streams the one file, so a match anywhere in
/// it is found -- not only in the pages held, which is all the browser's
/// own find-in-page can see. The corpus search (`claude_search_transcripts`)
/// was not reused: it indexes each session's first 8 MB as one row and
/// answers with sessions, not messages.

import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { useClaudeTranscriptFind } from "../../api/hooks";
import { copyText } from "../../lib/clipboard";
import { useFilters } from "../../store/filters";
import type { FindHit, TranscriptMessage } from "../../types/transcript";
import { unsearchedNote } from "../../lib/masked";
import { MaskedText } from "../MaskedText";
import { errorMessage } from "../QueryError";
import { SHOW_LABELS, type TranscriptShow } from "./filters";
import { palette } from "./palette";
import { messagesMarkdown } from "./transcriptCopy";
import { loadedTurns, openerText } from "./turnNav";
import { findShortfall, type SinceYouLeft } from "./useNavigation";

// ---------------------------------------------------------------------
// What is shown
// ---------------------------------------------------------------------

export function ShowControls({ show, hidden }: { show: TranscriptShow; hidden: number }) {
  const set = useFilters((f) => f.setTranscriptShow);
  const hiddenText =
    hidden > 0
      ? `${hidden.toLocaleString()} ${hidden === 1 ? "item is" : "items are"} hidden in the loaded part of this transcript.`
      : "";
  return (
    <fieldset className="text-xs" style={{ color: palette.text }}>
      <legend className="mb-1" style={{ color: palette.muted }}>
        Show
      </legend>
      {(Object.keys(SHOW_LABELS) as (keyof TranscriptShow)[]).map((k) => (
        <label key={k} className="tap-target flex items-center gap-2">
          <input
            type="checkbox"
            checked={show[k]}
            onChange={(e) => set({ ...show, [k]: e.currentTarget.checked })}
          />
          {SHOW_LABELS[k]}
        </label>
      ))}
      {hidden > 0 ? (
        <p className="mt-1" style={{ color: palette.muted }} data-testid="filter-hidden">
          {hiddenText}
        </p>
      ) : null}
      {/* Mounted with the controls, so ticking a box announces what it
          hid (#1489): a region that arrives with its text is often not
          read. */}
      <span role="status" className="sr-only">
        {hiddenText}
      </span>
    </fieldset>
  );
}

// ---------------------------------------------------------------------
// Since you left
// ---------------------------------------------------------------------

/// The line drawn above the first message since the reader left.
export function UnreadDivider() {
  return (
    <div
      role="separator"
      aria-label="New since you left"
      className="mb-2 flex items-center gap-2 text-[11px]"
      style={{ color: palette.accent }}
      data-testid="unread-divider"
    >
      <span className="h-px flex-1" style={{ background: palette.accent }} />
      New since you left
      <span className="h-px flex-1" style={{ background: palette.accent }} />
    </div>
  );
}

export function AwayCard({
  since,
  onGo,
  onDismiss,
}: {
  since: SinceYouLeft;
  /// Go to where the reader left off; omitted when there is nowhere to go.
  onGo?: () => void;
  onDismiss: () => void;
}) {
  if (since.summary === null) return null;
  return (
    <section
      aria-label="While you were away"
      className="rounded border p-2 text-xs"
      style={{ background: palette.surface, borderColor: palette.border, color: palette.text }}
      data-testid="away-card"
    >
      <p>
        <span className="font-semibold">While you were away:</span> {since.summary.text}
      </p>
      {since.beforeHeld ? (
        <p className="mt-1" style={{ color: palette.muted }}>
          Where you left off is earlier than the part of this transcript that is loaded.
        </p>
      ) : null}
      <div className="mt-1 flex flex-wrap gap-3">
        {onGo ? (
          <button type="button" className="tap-target underline" style={{ color: palette.link }} onClick={onGo}>
            {since.beforeHeld ? "Load back to where you left off" : "Go to where you left off"}
          </button>
        ) : null}
        <button type="button" className="tap-target underline" style={{ color: palette.link }} onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------
// The outline, and find
// ---------------------------------------------------------------------

function clock(ts: string | null): string | null {
  if (ts === null) return null;
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? null : d.toLocaleString();
}

function HitList({
  hits,
  onJump,
  label,
}: {
  hits: readonly FindHit[];
  onJump: (hit: FindHit) => void;
  label: string;
}) {
  return (
    <ol aria-label={label} className="flex flex-col gap-1">
      {hits.map((h) => (
        <li key={`${h.cursor.offset}:${h.message_id}`}>
          <button
            type="button"
            onClick={() => onJump(h)}
            className="tap-target w-full rounded px-2 py-1 text-left text-xs hover:bg-[#21262d] focus-visible:outline focus-visible:outline-2"
            style={{ color: palette.text }}
          >
            {clock(h.timestamp) ? (
              <span className="block text-[11px]" style={{ color: palette.muted }}>
                {clock(h.timestamp)}
              </span>
            ) : null}
            <span className="line-clamp-2 break-words">
              {h.snippet === "" ? (
                <span style={{ color: palette.muted }}>(no text)</span>
              ) : (
                <MaskedText text={h.snippet} />
              )}
            </span>
          </button>
        </li>
      ))}
    </ol>
  );
}

/// Every turn in the session, from the whole file.
export function TurnOutline({
  path,
  reveal = false,
  onJump,
}: {
  path: string;
  reveal?: boolean;
  onJump: (hit: FindHit) => void;
}) {
  const q = useClaudeTranscriptFind(path, null, { enabled: true, reveal });
  if (q.isError) {
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        Could not list the turns{errorMessage(q.error) ? ` (${errorMessage(q.error)})` : ""}.{" "}
        <button type="button" className="underline" style={{ color: palette.link }} onClick={() => void q.refetch()}>
          Try again
        </button>
      </p>
    );
  }
  if (q.data === undefined) {
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        Listing the turns…
      </p>
    );
  }
  const short = findShortfall(q.data, "turns");
  return (
    <div data-testid="turn-outline">
      {short ? (
        <p className="mb-1 text-[11px]" style={{ color: palette.warn }}>
          {short}
        </p>
      ) : null}
      {q.data.hits.length === 0 ? (
        <p className="text-xs" style={{ color: palette.muted }}>
          {q.data.complete ? "No prompts in this session." : "No prompts in the part that was read."}
        </p>
      ) : (
        <HitList hits={q.data.hits} onJump={onJump} label="Turns" />
      )}
    </div>
  );
}

/// Find in the whole session.
export function FindInSession({
  path,
  reveal = false,
  onJump,
  autoFocus = false,
}: {
  path: string;
  reveal?: boolean;
  onJump: (hit: FindHit) => void;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setQuery(text.trim()), 300);
    return () => clearTimeout(t);
  }, [text]);
  const asked = query.length >= 2;
  const q = useClaudeTranscriptFind(path, asked ? query : null, { enabled: asked, reveal });
  let body = null;
  // What a screen reader hears as the search moves on (#1489), from a
  // region mounted with the box -- one that appeared already holding
  // its text would often not be read.
  let said = "";
  if (!asked) {
    // Nothing typed: nothing was asked, which is not "no matches".
  } else if (q.isError) {
    said = "Could not search this session.";
    body = (
      <p className="text-xs" style={{ color: palette.muted }}>
        Could not search this session{errorMessage(q.error) ? ` (${errorMessage(q.error)})` : ""}.{" "}
        <button type="button" className="underline" style={{ color: palette.link }} onClick={() => void q.refetch()}>
          Try again
        </button>
      </p>
    );
  } else if (q.data === undefined) {
    said = "Searching the session…";
    body = (
      <p className="text-xs" style={{ color: palette.muted }}>
        Searching the session…
      </p>
    );
  } else {
    const f = q.data;
    const short = findShortfall(f, "matches");
    const unsearched = unsearchedNote(f.masking);
    const bounded = f.more || !f.complete;
    const count =
      f.hits.length === 0
        ? f.complete
          ? "No matches in this session."
          : "No matches in the part that was read."
        : `${bounded ? "At least " : ""}${f.hits.length.toLocaleString()} ${f.hits.length === 1 ? "match" : "matches"}`;
    said = count;
    body = (
      <>
        <p className="mb-1 text-[11px]" style={{ color: palette.muted }} data-testid="find-count">
          {count}
        </p>
        {short ? (
          <p className="mb-1 text-[11px]" style={{ color: palette.warn }}>
            {short}
          </p>
        ) : null}
        {unsearched ? (
          <p className="mb-1 text-[11px]" style={{ color: palette.muted }}>
            {unsearched}
          </p>
        ) : null}
        <HitList hits={f.hits} onJump={onJump} label="Matches" />
      </>
    );
  }
  return (
    <div data-testid="find-in-session">
      <input
        type="search"
        aria-label="Find in this session"
        placeholder="Find in this session"
        value={text}
        // A search box the reader opened on purpose takes the focus.
        autoFocus={autoFocus}
        onChange={(e) => setText(e.currentTarget.value)}
        className="mb-2 w-full rounded border px-2 py-1 text-xs"
        style={{ background: palette.ground, borderColor: palette.border, color: palette.text }}
      />
      <span role="status" className="sr-only">
        {said}
      </span>
      {body}
    </div>
  );
}

// ---------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------

function copy(what: string, markdown: string) {
  void copyText(markdown).then((failure) => {
    if (failure !== null) {
      toast.error(`Could not copy the ${what}`, { description: failure });
      return;
    }
    toast.success(`Copied the ${what} as markdown`);
  });
}

/// Copy the loaded session, or a range of its turns, as markdown.
export function ExportControls({
  messages,
  hasOlder,
  atLiveEdge,
}: {
  messages: readonly TranscriptMessage[];
  hasOlder: boolean;
  atLiveEdge: boolean;
}) {
  const turns = useMemo(() => loadedTurns(messages), [messages]);
  const [from, setFrom] = useState(0);
  const [to, setTo] = useState<number | null>(null);
  const last = turns.length - 1;
  const end = Math.min(to ?? last, last);
  const start = Math.min(from, end);
  const label = (i: number) => {
    const t = openerText(turns[i].opener);
    return `${i + 1}. ${t.length > 40 ? `${t.slice(0, 40)}…` : t || "(no text)"}`;
  };
  return (
    <div className="flex flex-col gap-2 text-xs" style={{ color: palette.text }} data-testid="export-controls">
      <button
        type="button"
        className="tap-target self-start rounded border px-2 py-0.5"
        style={{ borderColor: palette.border }}
        onClick={() =>
          copy(
            "session",
            messagesMarkdown(messages, { earlierUnloaded: hasOlder, laterUnloaded: !atLiveEdge }),
          )
        }
      >
        {hasOlder || !atLiveEdge ? "Copy the loaded part as markdown" : "Copy the session as markdown"}
      </button>
      {turns.length > 1 ? (
        <div className="flex flex-wrap items-center gap-1">
          <label className="flex items-center gap-1">
            Turns from
            <select
              value={start}
              onChange={(e) => setFrom(Number(e.currentTarget.value))}
              className="max-w-40 rounded border px-1"
              style={{ background: palette.ground, borderColor: palette.border }}
            >
              {turns.map((_, i) => (
                <option key={turns[i].opener.id} value={i}>
                  {label(i)}
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-1">
            to
            <select
              value={end}
              onChange={(e) => setTo(Number(e.currentTarget.value))}
              className="max-w-40 rounded border px-1"
              style={{ background: palette.ground, borderColor: palette.border }}
            >
              {turns.map((_, i) => (
                <option key={turns[i].opener.id} value={i}>
                  {label(i)}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="tap-target rounded border px-2 py-0.5"
            style={{ borderColor: palette.border }}
            onClick={() =>
              copy(
                "turns",
                messagesMarkdown(
                  turns.slice(start, end + 1).flatMap((t) => t.messages),
                  { earlierUnloaded: false, laterUnloaded: end === last && !atLiveEdge },
                ),
              )
            }
          >
            Copy these turns
          </button>
        </div>
      ) : null}
    </div>
  );
}
