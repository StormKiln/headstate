/// The transcript viewer's shell (#1479, epic #1473).
///
/// Scrolling, following and windowing only. What a message LOOKS like is
/// the caller's `renderMessage` -- the desktop terminal renderer (#1480)
/// and the phone's bubbles (#1481) -- and where the messages come from is
/// the caller's data hook (the paging and follow reads, #1220 and #1476).
///
/// # Behaviour, and which part owns it
///
/// Built on the vendored shadcn `MessageScroller` (`ui/message-scroller`),
/// which owns the scroll mechanics. This file decides only what the
/// scroller cannot know about a transcript:
///
/// | behaviour | owner |
/// |---|---|
/// | open at the newest turn: its prompt at the top, the reply below | `defaultScrollPosition="last-anchor"` + the first batch's last opener anchored (`trackArrivals`) |
/// | follow new output while at the bottom; stop when scrolled away | `autoScroll`, on only while the window reaches the newest message |
/// | a new turn arriving at the live edge is placed at the top | its opener anchored (`trackArrivals`) |
/// | a new turn arriving while scrolled up does NOT move the reader | its opener NOT anchored (`trackArrivals`) |
/// | "↓ N new", counting what arrived while scrolled up | `trackArrivals`, shown on `MessageScrollerButton` |
/// | the button returns to the live edge and re-engages follow | `scrollToEnd`, after re-windowing to the tail if needed |
/// | older rows loaded above keep the visible row still | `preserveScrollOnPrepend` (the default) |
/// | at most ~`WINDOW_SIZE` messages mounted | `transcriptWindow.ts` |
/// | a jump to a held message: re-window around it, then scroll it to the top | `handle.scrollTo` + `aroundPins` (#1484) |
/// | the newest message read, for the "since you left" marker | `onRead` (#1484) |
///
/// # Accessibility
///
/// `role="log"` and `aria-relevant="additions"` come from
/// `MessageScrollerContent`. `aria-busy` is set while the caller says the
/// last turn is streaming. The jump button is a real `<button>`, in the
/// tab order whenever it is active. Programmatic scrolls are instant
/// under `prefers-reduced-motion`, and the button's own transition is
/// dropped.
///
/// # The composer slot (#1491, #1490 constraint 3)
///
/// The viewer is a column: the scroller takes the free height and a
/// composer slot sits below it. 7.10's input goes in that slot, so
/// showing it shrinks the scroller rather than pushing the conversation
/// -- and the scroller's follow re-pins the live edge when its viewport
/// shrinks. In 7.9 the slot is empty and `hidden`, behind
/// `COMPOSER_ENABLED` (`composerFlag.ts`).
///
/// # Pending messages and sends (#1491, #1490 constraints 2 and 4)
///
/// `pending` are messages this app is sending that the transcript does
/// not hold yet (`pending.ts`). They are drawn by `renderPending` after
/// the newest message, and only while the window reaches it AND the held
/// messages reach the live edge (`atLiveEdge`): a pending message goes
/// after the newest turn, not after whatever the reader scrolled back
/// to. A send away from the live edge asks `onJumpToLatest` first, as
/// the jump button does. A composer inside the viewer calls
/// `useSendScroll()` once a send starts; that re-engages follow and
/// anchors the pending row (`sendScroll.ts`).

import {
  type MouseEvent,
  type ReactNode,
  type Ref,
  type UIEvent,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
  useMessageScroller,
  useMessageScrollerScrollable,
} from "@/components/ui/message-scroller";
import { cn } from "@/lib/utils";
import type { TranscriptMessage } from "../../types/transcript";
import { COMPOSER_ENABLED } from "./composerFlag";
import { type PendingMessage, pendingItemId } from "./pending";
import { sendScroll, TranscriptSendContext } from "./sendScroll";
import {
  type Arrivals,
  aroundPins,
  caughtUp,
  extendEnd,
  extendStart,
  NO_ARRIVALS,
  OPEN_PINS,
  resolveWindow,
  tailPins,
  trackArrivals,
  type WindowPins,
  withoutAnchors,
} from "./transcriptWindow";

/// What a host can ask of the viewer directly (#1484): turn navigation,
/// a find's hit, the "since you left" marker.
export interface TranscriptViewerHandle {
  /// Bring `id` into the window and scroll it to the top. `false` when
  /// the viewer does not hold it: the host loads its page first.
  scrollTo(id: string): boolean;
  /// The first row inside the viewport, or `null` when none is laid out.
  firstVisible(): string | null;
  /// Go to the live edge and follow it, as the jump button does: the
  /// desktop's `End` key (#1489).
  scrollToLatest(): void;
}

export interface TranscriptViewerProps {
  /// Oldest first, as the read model returns them. May hold more than
  /// the viewer mounts; see `transcriptWindow.ts`.
  messages: readonly TranscriptMessage[];
  /// One message's content. Rendered inside the scroller's item, which
  /// carries the id and the anchor; the renderer carries neither.
  renderMessage: (message: TranscriptMessage) => ReactNode;
  /// Whether the last turn is still being written. `undefined` is "not
  /// known", which renders as not busy: `aria-busy` only ever asserts
  /// what the caller established.
  streaming?: boolean;
  /// The reader reached the oldest message the viewer holds. The data
  /// hook pages older messages in by PREPENDING to `messages`.
  onReachStart?: () => void;
  /// The reader reached the newest message the viewer holds. The data
  /// hook pages newer messages in by APPENDING to `messages`.
  onReachEnd?: () => void;
  /// Which messages are mounted, by id, whenever that changes: the data
  /// hook keeps the pages holding them and lets far ones go (#1476).
  onWindowChange?: (firstId: string, lastId: string) => void;
  /// Whether `messages` reaches the newest message the hook knows of.
  /// `false` once it let the newest pages go while the reader was far
  /// back; "jump to latest" then asks `onJumpToLatest` for them rather
  /// than scrolling to the end of what is held. Default `true`.
  atLiveEdge?: boolean;
  onJumpToLatest?: () => void;
  /// The viewport's accessible name.
  label?: string;
  handle?: Ref<TranscriptViewerHandle>;
  /// The newest message the reader has had on screen: the last row in
  /// the viewport after a scroll, and the newest message while following
  /// the live edge. Feeds the "since you left" marker (#1484).
  onRead?: (id: string) => void;
  /// Reserved for 7.10's composer (#1491); ignored while
  /// `COMPOSER_ENABLED` is off.
  composer?: ReactNode;
  /// Messages being sent that the transcript does not hold yet, oldest
  /// first, already reconciled (`usePendingMessages`).
  pending?: readonly PendingMessage[];
  /// One pending message's content, like `renderMessage`.
  renderPending?: (pending: PendingMessage) => ReactNode;
  className?: string;
}

export function TranscriptViewer(props: TranscriptViewerProps) {
  // Follow-output only while the window reaches the newest message. The
  // scroller arms follow whenever the reader reaches the bottom of what
  // is MOUNTED; at the bottom of a window that stops short of the newest
  // message, that would turn the next page brought in below into a jump
  // to its end -- skipping the page the reader was scrolling into.
  const [windowAtTail, setWindowAtTail] = useState(true);
  return (
    <MessageScrollerProvider autoScroll={windowAtTail} defaultScrollPosition="last-anchor">
      <ViewerBody {...props} onWindowAtTail={setWindowAtTail} />
    </MessageScrollerProvider>
  );
}

/// Whether the reader asked for less motion. Absent `matchMedia` (jsdom,
/// and nothing else this app runs in) reads as no preference.
function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-reduced-motion: reduce)").matches
    : false;
}

function ViewerBody({
  messages,
  renderMessage,
  streaming,
  onReachStart,
  onReachEnd,
  onWindowChange,
  atLiveEdge = true,
  onJumpToLatest,
  label = "Transcript",
  composer,
  pending,
  renderPending,
  className,
  onWindowAtTail,
  handle,
  onRead,
}: TranscriptViewerProps & { onWindowAtTail: (atTail: boolean) => void }) {
  const scrollable = useMessageScrollerScrollable();
  const { scrollToEnd, scrollToMessage } = useMessageScroller();
  const viewportRef = useRef<HTMLDivElement | null>(null);

  const [pins, setPins] = useState<WindowPins>(OPEN_PINS);
  // At the live edge: the window ends at the newest message AND the
  // scroller has nothing further to scroll toward the end. While
  // following, the scroller publishes `end: false` even mid-catch-up.
  const following = pins.end.at === "tail" && !scrollable.end;
  const win = resolveWindow(messages, pins, following, pins === OPEN_PINS);
  if (win.pins !== pins) setPins(win.pins);

  // The arrival bookkeeping, folded in during render ("adjusting state
  // when a prop changes", as React documents it) so the anchors a batch
  // carries are on the rows in the SAME commit that mounts them -- the
  // scroller reads them from the DOM when it sees the rows appear.
  const [seen, setSeen] = useState<{
    messages: readonly TranscriptMessage[];
    arrivals: Arrivals;
  }>({ messages: [], arrivals: NO_ARRIVALS });
  let arrivals = seen.arrivals;
  if (seen.messages !== messages) {
    arrivals = trackArrivals(seen.arrivals, messages, following);
    setSeen({ messages, arrivals });
  } else if (following && arrivals.newCount !== 0) {
    arrivals = caughtUp(arrivals);
    setSeen({ messages, arrivals });
  }

  // Following only where the held messages reach the live edge: at the
  // end of a run the hook detached from it, new output is not arriving.
  const windowAtTail = win.pins.end.at === "tail" && atLiveEdge;
  useLayoutEffect(() => onWindowAtTail(windowAtTail), [windowAtTail, onWindowAtTail]);

  const firstShown = messages[win.from]?.id;
  const lastShown = messages[win.to - 1]?.id;
  useLayoutEffect(() => {
    if (firstShown !== undefined && lastShown !== undefined) onWindowChange?.(firstShown, lastShown);
  }, [firstShown, lastShown, onWindowChange]);

  // Rows dropped from the top by a reach toward the end: hold the row the
  // reader was looking at in place. Compared relative to the viewport, so
  // this is a no-op wherever the engine's own scroll anchoring already
  // did it -- the same approach the scroller takes for prepends.
  const holdRef = useRef<{ element: HTMLElement; top: number } | null>(null);
  // "Jump to latest" from a window that does not reach the newest
  // message: re-window first, then scroll once the rows are mounted.
  const jumpRef = useRef(false);
  const reducedMotion = prefersReducedMotion();

  useLayoutEffect(() => {
    const hold = holdRef.current;
    const viewport = viewportRef.current;
    holdRef.current = null;
    if (hold && viewport && hold.element.isConnected) {
      const delta =
        hold.element.getBoundingClientRect().top - viewport.getBoundingClientRect().top - hold.top;
      if (Math.abs(delta) > 0.5) viewport.scrollTop += delta;
    }
    if (jumpRef.current) {
      jumpRef.current = false;
      scrollToEnd({ behavior: "auto" });
    }
  }, [win.from, win.to, scrollToEnd]);

  // One request per edge message: a scroll handler fires many times at
  // an edge, and the hook should be asked once per page it could add.
  const askedOlderAt = useRef<string | null>(null);
  const askedNewerAt = useRef<string | null>(null);

  const moveWindow = (next: WindowPins) => {
    setSeen((s) => ({ ...s, arrivals: withoutAnchors(s.arrivals) }));
    setPins(next);
  };

  // The live edge from a window that does not reach it: re-window, and
  // scroll once the rows are mounted. `false` when the window already
  // reaches it and a plain scroll to the end is the whole job.
  // `moveWindow` written out with the (stable) setters, so this is stable
  // for the handle below.
  const winTo = win.to;
  const rewindToLatest = useCallback((): boolean => {
    const toTail = () => {
      jumpRef.current = true;
      setSeen((s) => ({ ...s, arrivals: withoutAnchors(s.arrivals) }));
      setPins(tailPins(messages));
    };
    if (!atLiveEdge && onJumpToLatest) {
      // The newest messages are not held: the hook opens on them, and
      // the window lands on the tail of what it brings.
      onJumpToLatest();
      toTail();
      return true;
    }
    if (winTo >= messages.length) return false;
    toTail();
    return true;
  }, [atLiveEdge, onJumpToLatest, messages, winTo]);

  // A jump to a message (#1484): re-window around it when it is outside
  // the window, then scroll once its row is mounted.
  // A ref, not state: the scroll is the effect's whole job, and a request
  // bumps `jumps` so the effect runs even when the window did not move.
  const targetRef = useRef<string | null>(null);
  const [jumps, setJumps] = useState(0);
  useLayoutEffect(() => {
    const id = targetRef.current;
    if (id !== null && scrollToMessage(id, { align: "start", behavior: "auto" })) {
      targetRef.current = null;
    }
  }, [jumps, win.from, win.to, scrollToMessage]);
  useImperativeHandle(
    handle,
    () => ({
      scrollTo(id: string) {
        const i = messages.findIndex((m) => m.id === id);
        if (i < 0) return false;
        if (i < win.from || i >= win.to) moveWindow(aroundPins(messages, i));
        targetRef.current = id;
        setJumps((n) => n + 1);
        return true;
      },
      firstVisible() {
        const viewport = viewportRef.current;
        const row = viewport ? firstVisibleRow(viewport) : null;
        return row?.element.dataset.messageId ?? null;
      },
      scrollToLatest() {
        if (!rewindToLatest()) scrollToEnd({ behavior: reducedMotion ? "auto" : "smooth" });
      },
    }),
    // `moveWindow` is recreated each render and closes over setters only.
    [messages, win.from, win.to, rewindToLatest, reducedMotion, scrollToEnd],
  );

  // Following the live edge, the newest message is on screen.
  const newestId = messages[messages.length - 1]?.id;
  const followingLive = following && atLiveEdge;
  useEffect(() => {
    if (followingLive && newestId !== undefined) onRead?.(newestId);
  }, [followingLive, newestId, onRead]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (onRead) {
      const last = lastVisibleRow(el);
      if (last !== null) onRead(last);
    }
    const margin = el.clientHeight;
    if (messages.length === 0 || margin <= 0) return;

    if (el.scrollTop <= margin) {
      const move = extendStart(messages, win);
      if (move) {
        moveWindow(move.pins);
      } else if (onReachStart && askedOlderAt.current !== messages[0].id) {
        askedOlderAt.current = messages[0].id;
        onReachStart();
      }
    }

    if (el.scrollHeight - el.scrollTop - el.clientHeight <= margin) {
      const move = extendEnd(messages, win);
      if (move) {
        if (move.trimmedStart) holdRef.current = firstVisibleRow(el);
        moveWindow(move.pins);
      } else if (onReachEnd && askedNewerAt.current !== messages[messages.length - 1].id) {
        askedNewerAt.current = messages[messages.length - 1].id;
        onReachEnd();
      }
    }
  };

  // Otherwise the scroller's own scrollToEnd does it.
  const onJump = (e: MouseEvent<HTMLButtonElement>) => {
    if (rewindToLatest()) e.preventDefault();
  };

  const count = arrivals.newCount;
  const counted =
    count !== null && count > 0
      ? `${arrivals.atLeast ? "at least " : ""}${count.toLocaleString()} new`
      : null;
  const shown = messages.slice(win.from, win.to);
  const atTail = win.to >= messages.length;
  const shownPending = atTail && atLiveEdge && renderPending ? (pending ?? []) : [];

  // The pending row a send anchored (`sendScroll.ts`). One at a time:
  // the newest send is the turn the reader is waiting on.
  const [sendAnchor, setSendAnchor] = useState<string | null>(null);
  const onSent = useCallback(
    (clientId: string) =>
      sendScroll(clientId, {
        atTail: atTail && atLiveEdge,
        reducedMotion,
        scrollToEnd,
        rewindToTail: () => {
          // As "jump to latest": the newest messages may not be held.
          if (!atLiveEdge) onJumpToLatest?.();
          jumpRef.current = true;
          setSeen((s) => ({ ...s, arrivals: withoutAnchors(s.arrivals) }));
          setPins(tailPins(messages));
        },
        anchor: setSendAnchor,
      }),
    [atTail, atLiveEdge, onJumpToLatest, reducedMotion, scrollToEnd, messages],
  );

  return (
    <TranscriptSendContext.Provider value={onSent}>
      <div data-slot="transcript-viewer" className={cn("flex h-full min-h-0 flex-col", className)}>
        <MessageScroller className="min-h-0 flex-1">
          <MessageScrollerViewport ref={viewportRef} aria-label={label} onScroll={onScroll}>
            <MessageScrollerContent aria-busy={streaming === true} className="gap-3 p-3">
              {shown.map((m, i) => (
                <MessageScrollerItem
                  key={m.id}
                  messageId={m.id}
                  scrollAnchor={arrivals.anchors.has(m.id)}
                  // Budget B1's probe (docs/transcript-performance.md): the
                  // browser harness times first paint of the newest message
                  // through Element Timing, which reads this attribute.
                  {...(atTail && i === shown.length - 1 ? { elementtiming: "newest" } : {})}
                >
                  {renderMessage(m)}
                </MessageScrollerItem>
              ))}
              {shownPending.map((p) => (
                <MessageScrollerItem
                  key={pendingItemId(p.clientId)}
                  messageId={pendingItemId(p.clientId)}
                  scrollAnchor={sendAnchor === p.clientId}
                  data-pending-state={p.state}
                >
                  {renderPending?.(p)}
                </MessageScrollerItem>
              ))}
            </MessageScrollerContent>
          </MessageScrollerViewport>
          <MessageScrollerButton
            size="sm"
            behavior={reducedMotion ? "auto" : "smooth"}
            onClick={onJump}
            className="h-auto gap-1 rounded-full px-3 py-1 motion-reduce:transition-none"
            aria-label={
              counted !== null ? `Jump to the latest message, ${counted}` : "Jump to the latest message"
            }
          >
            <span aria-hidden="true">↓</span>
            {counted ?? "Latest"}
          </MessageScrollerButton>
        </MessageScroller>
        <div data-slot="transcript-composer" className="shrink-0" hidden={!COMPOSER_ENABLED}>
          {COMPOSER_ENABLED ? composer : null}
        </div>
      </div>
    </TranscriptSendContext.Provider>
  );
}

/// The id of the last mounted row whose box is inside the viewport.
function lastVisibleRow(viewport: HTMLElement): string | null {
  const top = viewport.getBoundingClientRect().top;
  const bottom = viewport.getBoundingClientRect().bottom;
  let last: string | null = null;
  for (const el of viewport.querySelectorAll<HTMLElement>("[data-message-id]")) {
    const r = el.getBoundingClientRect();
    if (r.bottom > top && r.top < bottom) last = el.dataset.messageId ?? last;
    else if (last !== null) break;
  }
  return last;
}

/// The first mounted row whose box is inside the viewport, and where.
function firstVisibleRow(viewport: HTMLElement): { element: HTMLElement; top: number } | null {
  const top = viewport.getBoundingClientRect().top;
  const bottom = viewport.getBoundingClientRect().bottom;
  for (const el of viewport.querySelectorAll<HTMLElement>("[data-message-id]")) {
    const r = el.getBoundingClientRect();
    if (r.bottom > top && r.top < bottom) return { element: el, top: r.top - top };
  }
  return null;
}
