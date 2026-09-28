/// The state behind transcript navigation (#1484); the controls that
/// draw it are `navigation.tsx`. Both hosts -- `DesktopTranscript` and
/// `PhoneTranscript` -- use these.
///
/// # Jumping to a message that is not held
///
/// The viewer can only scroll to a row it holds. A jump to anything else
/// -- a turn from the outline, a find's hit, the previous prompt above
/// what is loaded -- first asks the follow for it (`seek`, or
/// `loadOlderUntil`), and lands once the messages holding it have
/// rendered. A jump the follow could not satisfy says so; it never lands
/// somewhere else silently.

import {
  type KeyboardEvent,
  type RefObject,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { TranscriptLive } from "../../api/hooks";
import { useFilters } from "../../store/filters";
import type { ClaudeWaiting } from "../../types/pr";
import type { PageCursor, TranscriptFind, TranscriptMessage } from "../../types/transcript";
import { applyShow, showFrom, shownAnchor } from "./filters";
import { advanceMarker, awaySummary, type AwaySummary, readMarker, type ReadMarker, unreadFrom } from "./sinceYouLeft";
import type { TaskListState } from "./tasks";
import type { TranscriptViewerHandle } from "./TranscriptViewer";
import { adjacentOpener, isOpener, rowFor } from "./turnNav";

// ---------------------------------------------------------------------
// What is shown
// ---------------------------------------------------------------------

/// The per-device show settings, and the messages as they show them.
export function useShown(messages: readonly TranscriptMessage[]) {
  const stored = useFilters((f) => f.transcriptShow);
  const show = useMemo(() => showFrom(stored), [stored]);
  const filtered = useMemo(() => applyShow(messages, show), [messages, show]);
  return { show, ...filtered };
}

// ---------------------------------------------------------------------
// Since you left
// ---------------------------------------------------------------------

/// This device's marker for `path`, as it was when the transcript was
/// opened: the divider and the summary are about THAT visit, however the
/// stored marker moves while reading.
export function useOpenedMarker(path: string): ReadMarker | null {
  const [held, setHeld] = useState(() => ({ path, marker: readMarker(path) }));
  if (held.path !== path) {
    // Another transcript in the same host: its own marker, read once.
    const next = { path, marker: readMarker(path) };
    setHeld(next);
    return next.marker;
  }
  return held.marker;
}

export interface SinceYouLeft {
  /// The summary card's content, or `null` when there is nothing new.
  summary: AwaySummary | null;
  /// The shown row the unread divider is drawn above, or `null` when it
  /// cannot be placed in what is held.
  dividerAt: string | null;
  /// Where the reader left off is older than what is held.
  beforeHeld: boolean;
  /// The viewer's `onRead`: moves the stored marker forward.
  onRead: (id: string) => void;
}

export function useSinceYouLeft({
  path,
  marker,
  messages,
  shown,
  hasOlder,
  tasks,
  waiting,
}: {
  path: string;
  marker: ReadMarker | null;
  messages: readonly TranscriptMessage[];
  shown: readonly TranscriptMessage[];
  hasOlder: boolean;
  tasks?: TaskListState;
  waiting?: ClaudeWaiting;
}): SinceYouLeft {
  const held = useRef(messages);
  useEffect(() => {
    held.current = messages;
  }, [messages]);
  // How far this visit has already moved the marker: a scroll reports
  // on every frame, and only a row past it is worth touching storage for.
  const reached = useRef<{ path: string; offset: number } | null>(null);
  const onRead = useCallback(
    (id: string) => {
      const row = held.current.find((m) => m.id === id);
      if (!row || row.offset === null) return;
      const r = reached.current;
      if (r !== null && r.path === path && r.offset >= row.offset) return;
      advanceMarker(path, row);
      reached.current = { path, offset: row.offset };
    },
    [path],
  );
  const summary = useMemo(
    () => (marker === null ? null : awaySummary(messages, marker, { hasOlder, tasks, waiting })),
    [marker, messages, hasOlder, tasks, waiting],
  );
  const { dividerAt, beforeHeld } = useMemo(() => {
    if (marker === null) return { dividerAt: null, beforeHeld: false };
    const { index, placed } = unreadFrom(messages, marker, hasOlder);
    if (!placed) return { dividerAt: null, beforeHeld: true };
    const visible = new Set(shown.map((m) => m.id));
    const first = messages.slice(index).find((m) => visible.has(m.id));
    return { dividerAt: first?.id ?? null, beforeHeld: false };
  }, [marker, messages, shown, hasOlder]);
  return { summary, dividerAt, beforeHeld, onRead };
}

// ---------------------------------------------------------------------
// Jumping
// ---------------------------------------------------------------------

type Pending =
  | { kind: "id"; id: string }
  /// `seen`: the messages when the step asked for more. A step that
  /// still finds nothing once they change is over, so a later arrival
  /// never turns into a jump nobody asked for.
  | { kind: "step"; from: string | null; dir: -1 | 1; seen: readonly TranscriptMessage[] };

/// Where a jump lands, and what to say about it.
type Landing = { anchor: string | null; note: string | null };

const HIDDEN_NEAREST = "That message is hidden by the Show settings; this is the nearest one shown.";
const HIDDEN_ALL = "That message is hidden by the Show settings, and nothing near it is shown.";

export interface Jumps {
  /// Scroll to message `id`, loading its page first when it is not held.
  jumpTo: (id: string, at: PageCursor | null) => void;
  /// The previous (`-1`) or next (`1`) prompt from the top of the view.
  step: (dir: -1 | 1) => void;
  /// What the last jump could not do, in words; `null` when it did it.
  note: string | null;
}

export function useJumps({
  live,
  messages,
  shown,
  handle,
}: {
  live: Pick<TranscriptLive, "seek" | "loadOlderUntil" | "loadNewer" | "hasOlder" | "atLiveEdge">;
  messages: readonly TranscriptMessage[];
  shown: readonly TranscriptMessage[];
  handle: RefObject<TranscriptViewerHandle | null>;
}): Jumps {
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<string | null>(null);
  // Scrolling is the viewer's, through its handle: done in an effect
  // after the render that decided it. `seq` makes a second jump to the
  // same row a new request.
  const [landing, setLanding] = useState<{ anchor: string; seq: number } | null>(null);
  useEffect(() => {
    if (landing !== null) handle.current?.scrollTo(landing.anchor);
  }, [landing, handle]);

  /// Where `id` lands, if a row showing it is held.
  const resolveId = useCallback(
    (id: string): Landing | null => {
      const row = rowFor(messages, id);
      if (row === null) return null;
      const anchor = shownAnchor(messages, shown, row);
      if (anchor === null) return { anchor: null, note: HIDDEN_ALL };
      return { anchor, note: anchor === row ? null : HIDDEN_NEAREST };
    },
    [messages, shown],
  );

  const land = useCallback((l: Landing) => {
    setNote(l.note);
    if (l.anchor !== null) {
      const anchor = l.anchor;
      setLanding((p) => ({ anchor, seq: (p?.seq ?? 0) + 1 }));
    }
  }, []);

  // A jump waiting on a read lands once the messages holding it arrive:
  // settled during render, from the messages this render has.
  if (pending !== null) {
    if (pending.kind === "id") {
      const l = resolveId(pending.id);
      if (l !== null) {
        setPending(null);
        land(l);
      }
    } else {
      const target = adjacentOpener(shown, pending.from, pending.dir);
      if (target !== null) {
        setPending(null);
        land({ anchor: target.id, note: null });
      } else if (pending.seen !== messages) {
        setPending(null);
      }
    }
  }

  const jumpTo = useCallback(
    (id: string, at: PageCursor | null) => {
      const l = resolveId(id);
      if (l !== null) {
        land(l);
        return;
      }
      setPending({ kind: "id", id });
      void live.seek(id, at).then((held) => {
        if (held) return;
        setPending(null);
        setNote(
          at === null
            ? "That message is further back than can be loaded at once. Use Turns to go there."
            : "That message could not be loaded. The transcript may have changed since it was found: search again.",
        );
      });
    },
    [resolveId, land, live],
  );

  const step = useCallback(
    (dir: -1 | 1) => {
      const from = handle.current?.firstVisible() ?? null;
      const target = adjacentOpener(shown, from, dir);
      if (target !== null) {
        land({ anchor: target.id, note: null });
        return;
      }
      if (dir === -1 && live.hasOlder) {
        setPending({ kind: "step", from, dir, seen: messages });
        void live.loadOlderUntil(isOpener).then((found) => {
          if (found) return;
          setPending(null);
          setNote("No earlier prompt in the part loaded so far. Ask again to keep looking.");
        });
        return;
      }
      if (dir === 1 && !live.atLiveEdge) {
        setPending({ kind: "step", from, dir, seen: messages });
        live.loadNewer();
        return;
      }
      setNote(dir === -1 ? "This is the first prompt in the session." : "This is the latest prompt.");
    },
    [handle, shown, land, live, messages],
  );

  return { jumpTo, step, note };
}

/// `j`/`k` on the desktop: the next and previous prompt, from anywhere
/// in the transcript but a text field.
///
/// `End` goes to the live edge and follows it (#1489) -- more than the
/// engine's own `End`, which stops at the last message MOUNTED and does
/// not bring back newer pages the follow let go.
export function turnKeys(step: (dir: -1 | 1) => void, toLatest?: () => void) {
  return (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    const t = e.target as HTMLElement;
    if (t.closest("input, textarea, select, [contenteditable='true']")) return;
    if (e.key === "j") {
      e.preventDefault();
      step(1);
    } else if (e.key === "k") {
      e.preventDefault();
      step(-1);
    } else if (e.key === "End" && toLatest) {
      e.preventDefault();
      toLatest();
    }
  };
}

function mb(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// What a find's bounds left out, as a fact; `null` when nothing was.
export function findShortfall(f: TranscriptFind, what: "turns" | "matches"): string | null {
  const parts: string[] = [];
  if (!f.complete) {
    parts.push(`Only the first ${mb(f.scanned_to)} of ${mb(f.file_bytes)} was read.`);
  }
  if (f.more) parts.push(`The first ${f.hits.length.toLocaleString()} ${what} are listed.`);
  if (f.skimmed_records > 0 && what === "matches") {
    parts.push(
      `${f.skimmed_records.toLocaleString()} very large ${f.skimmed_records === 1 ? "record was" : "records were"} searched only in part.`,
    );
  }
  return parts.length > 0 ? parts.join(" ") : null;
}
