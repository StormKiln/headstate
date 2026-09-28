/// Which messages the transcript viewer mounts, and which it anchors (#1479).
///
/// # Window the data, not the DOM
///
/// #1479 chose this over a measuring virtualizer, and `virtualWindow.ts`
/// gives the test-environment half of the reason: a virtualizer measures
/// through `ResizeObserver` and `getBoundingClientRect`, jsdom has
/// neither, and the ~3,700 tests would see zero rows. The other half is
/// that transcript rows are variable-height, which `virtualWindow.ts`
/// cannot handle at all.
///
/// So the viewer mounts a bounded run of consecutive messages -- at most
/// `WINDOW_SIZE + WINDOW_SLACK` -- and leaves painting cost inside it to
/// `content-visibility: auto`, which the vendored `MessageScrollerItem`
/// already sets. The data hook (the paging reads, #1220 and #1476) may
/// hold more than that; everything outside the window stays data.
///
/// Budget B3 in `docs/transcript-performance.md` is the test of this
/// choice. If the browser harness shows it missing on the phone, a
/// measuring virtualizer goes in `MessageScrollerViewport` and a jsdom
/// metrics shim with it -- the decision #1479 records.
///
/// # Pinned to an edge, or to an id
///
/// Each end of the window is either pinned to the matching edge of what
/// the hook holds (`head`, `tail`) or to a message id. Ids, never
/// indexes: the hook prepends older pages and may let its oldest go, and
/// both shift every index while leaving ids where they were.
///
/// - Pinned to `tail`, new messages mount as they arrive.
/// - Pinned to `head`, an older page the hook prepends mounts, and the
///   scroller's `preserveScrollOnPrepend` keeps the visible row still.
///
/// # Hysteresis, and why it is load-bearing
///
/// The window is trimmed back to `WINDOW_SIZE` only once it passes
/// `WINDOW_SIZE + WINDOW_SLACK`, never one row per append. A window that
/// dropped its oldest row for every new one would be a same-length
/// change to the list, and the scroller treats a same-length change that
/// holds an anchor it has not yet scrolled to as a request to scroll to
/// it (`getUnanchoredScrollAnchor` in `@shadcn/react`'s controller) --
/// yanking a reader to an old turn once per message.

import type { TranscriptMessage } from "../../types/transcript";

/// The window's target length. #1479 asked for 300-500.
export const WINDOW_SIZE = 400;
/// How far past `WINDOW_SIZE` the window may grow before it is trimmed.
export const WINDOW_SLACK = 100;
/// How many messages one reach toward an edge brings in.
export const WINDOW_STEP = 100;

type StartPin = { at: "head" } | { at: "id"; id: string };
type EndPin = { at: "tail" } | { at: "id"; id: string };

export interface WindowPins {
  start: StartPin;
  end: EndPin;
}

/// Where a freshly opened viewer starts: the newest `WINDOW_SIZE`.
///
/// `start` is `head` here only as a placeholder; `resolveWindow` pins it
/// to the id `WINDOW_SIZE` before the tail on the first render that has
/// messages, or leaves it at `head` if there are fewer than that.
export const OPEN_PINS: WindowPins = { start: { at: "head" }, end: { at: "tail" } };

export interface ResolvedWindow {
  /// Half-open indexes into `messages`.
  from: number;
  to: number;
  /// The pins, normalised: equal to the input when nothing moved, so a
  /// caller can compare and skip a state update.
  pins: WindowPins;
}

function indexOfId(messages: readonly TranscriptMessage[], id: string): number {
  // From the end: the window is near the tail far more often than not.
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].id === id) return i;
  }
  return -1;
}

function pinsEqual(a: WindowPins, b: WindowPins): boolean {
  const s =
    a.start.at === b.start.at &&
    (a.start.at !== "id" || a.start.id === (b.start as { id: string }).id);
  const e =
    a.end.at === b.end.at && (a.end.at !== "id" || a.end.id === (b.end as { id: string }).id);
  return s && e;
}

function pinsFor(
  messages: readonly TranscriptMessage[],
  from: number,
  to: number,
): WindowPins {
  const n = messages.length;
  return {
    start: from === 0 ? { at: "head" } : { at: "id", id: messages[from].id },
    end: to >= n ? { at: "tail" } : { at: "id", id: messages[to - 1].id },
  };
}

/// Resolve the pins against the messages the hook holds now.
///
/// `following` is whether the reader is at the live edge. It decides
/// which end an over-long window is trimmed from: a reader at the bottom
/// loses the oldest rows, anyone else keeps what they are reading and
/// the newest stop mounting (they are counted, not lost -- see
/// `trackArrivals`).
///
/// `opening` is true until the first non-empty resolve, when the window
/// is placed at the tail regardless of the placeholder `head` pin. The
/// viewer passes `pins === OPEN_PINS`.
export function resolveWindow(
  messages: readonly TranscriptMessage[],
  pins: WindowPins,
  following: boolean,
  opening: boolean,
): ResolvedWindow {
  const n = messages.length;
  if (n === 0) return { from: 0, to: 0, pins };

  let to = n;
  if (pins.end.at === "id") {
    const i = indexOfId(messages, pins.end.id);
    // A pinned end the hook no longer holds (a re-read after compaction
    // replaced the list) falls back to the tail rather than to nothing.
    to = i >= 0 ? i + 1 : n;
  }

  let from: number;
  if (opening) {
    from = Math.max(0, to - WINDOW_SIZE);
  } else if (pins.start.at === "head") {
    from = 0;
  } else {
    const i = indexOfId(messages, pins.start.id);
    from = i >= 0 && i < to ? i : Math.max(0, to - WINDOW_SIZE);
  }

  if (to - from > WINDOW_SIZE + WINDOW_SLACK) {
    if (to === n && following) from = to - WINDOW_SIZE;
    else to = from + WINDOW_SIZE;
  }

  const next = pinsFor(messages, from, to);
  // Always a new object when opening, so a caller that tells "opening"
  // by identity with `OPEN_PINS` stops being in that state.
  return { from, to, pins: !opening && pinsEqual(next, pins) ? pins : next };
}

export interface WindowMove {
  pins: WindowPins;
  /// Rows were dropped from the TOP to make room. The caller holds the
  /// visible row still across that, since the scroller only does so for
  /// rows added above, not removed.
  trimmedStart: boolean;
}

/// Bring `WINDOW_STEP` older messages into the window, or `null` when it
/// already starts at the oldest message the hook holds -- the caller then
/// asks the hook for an older page.
export function extendStart(
  messages: readonly TranscriptMessage[],
  window: ResolvedWindow,
): WindowMove | null {
  if (window.from === 0) return null;
  const from = Math.max(0, window.from - WINDOW_STEP);
  let to = window.to;
  if (to - from > WINDOW_SIZE + WINDOW_SLACK) to = from + WINDOW_SIZE;
  return { pins: pinsFor(messages, from, to), trimmedStart: false };
}

/// Bring `WINDOW_STEP` newer messages into the window, or `null` when it
/// already ends at the newest message the hook holds.
export function extendEnd(
  messages: readonly TranscriptMessage[],
  window: ResolvedWindow,
): WindowMove | null {
  const n = messages.length;
  if (window.to >= n) return null;
  const to = Math.min(n, window.to + WINDOW_STEP);
  let from = window.from;
  let trimmedStart = false;
  if (to - from > WINDOW_SIZE + WINDOW_SLACK) {
    from = to - WINDOW_SIZE;
    trimmedStart = true;
  }
  return { pins: pinsFor(messages, from, to), trimmedStart };
}

/// The window "jump to latest" lands on: the newest `WINDOW_SIZE`.
export function tailPins(messages: readonly TranscriptMessage[]): WindowPins {
  const n = messages.length;
  return pinsFor(messages, Math.max(0, n - WINDOW_SIZE), n);
}

/// A window of `WINDOW_SIZE` centred on the message at `index`: where a
/// jump to a message out of the window lands (#1484).
export function aroundPins(messages: readonly TranscriptMessage[], index: number): WindowPins {
  const n = messages.length;
  const to = Math.min(n, Math.max(0, index - WINDOW_SIZE / 2) + WINDOW_SIZE);
  return pinsFor(messages, Math.max(0, to - WINDOW_SIZE), to);
}

/// Whether a message opens a turn: a prompt, slash command or shell
/// input. The read model says so by giving it its own id as `turn_id`.
function opensTurn(m: TranscriptMessage): boolean {
  return m.turn_id === m.id;
}

export interface Arrivals {
  /// The newest message id seen, or `null` before any.
  tailId: string | null;
  /// That message's recorded timestamp: what a replacement is counted
  /// against (#1476). `null` when it recorded none.
  tailAt: string | null;
  /// Messages that arrived below the reader while they were scrolled up.
  ///
  /// `null` when it cannot be counted: the hook replaced the list (a
  /// re-read after compaction) and neither the last message seen nor its
  /// timestamp can place what is new. "Qualify, or suppress": the button
  /// then says "Latest" with no figure rather than a count that might be
  /// wrong.
  newCount: number | null;
  /// `newCount` is a floor, not a count: the list was replaced, and only
  /// messages recorded after the last one seen are counted -- one with
  /// no timestamp may be new too. Said as "at least N".
  atLeast: boolean;
  /// Turn openers to mark `scrollAnchor` in the CURRENT batch.
  ///
  /// Only the current batch, deliberately. The scroller scrolls to any
  /// newly mounted anchor, so a flag that stayed on would re-anchor a
  /// turn every time the window brought its row back in. Two sources:
  ///
  /// - the first batch: the last opener, so `last-anchor` opens on it;
  /// - a later batch that arrived while the reader was at the live
  ///   edge: the openers in it, so a new turn is placed at the top with
  ///   its reply below (and 7.10's sent message is anchored the same
  ///   way, #1490 constraint 4).
  ///
  /// A turn that arrives while the reader is scrolled up is NOT anchored:
  /// anchoring it would scroll them away from what they are reading.
  anchors: ReadonlySet<string>;
}

const NO_ANCHORS: ReadonlySet<string> = new Set();

export const NO_ARRIVALS: Arrivals = {
  tailId: null,
  tailAt: null,
  newCount: 0,
  atLeast: false,
  anchors: NO_ANCHORS,
};

/// How many of `messages` were recorded after `after`: a floor on what
/// a replaced list brought in, or `null` when there is nothing to count
/// against. Claude Code stamps records as it writes them, so a message
/// recorded later than the last one seen was not seen; one with no
/// timestamp is not counted, which is why this is a floor.
function recordedAfter(messages: readonly TranscriptMessage[], after: string | null): number | null {
  const t = after === null ? Number.NaN : Date.parse(after);
  if (Number.isNaN(t)) return null;
  let n = 0;
  for (const m of messages) {
    if (m.timestamp !== null && Date.parse(m.timestamp) > t) n++;
  }
  return n;
}

/// Fold one new list from the hook into the arrival bookkeeping.
export function trackArrivals(
  prev: Arrivals,
  messages: readonly TranscriptMessage[],
  following: boolean,
): Arrivals {
  const n = messages.length;
  if (n === 0) return { ...prev, anchors: NO_ANCHORS };
  const tailId = messages[n - 1].id;
  const tailAt = messages[n - 1].timestamp;

  if (prev.tailId === null) {
    let last: string | null = null;
    for (let i = n - 1; i >= 0; i--) {
      if (opensTurn(messages[i])) {
        last = messages[i].id;
        break;
      }
    }
    return {
      tailId,
      tailAt,
      newCount: 0,
      atLeast: false,
      anchors: last === null ? NO_ANCHORS : new Set([last]),
    };
  }

  const k = indexOfId(messages, prev.tailId);
  if (k < 0) {
    if (following) return { tailId, tailAt, newCount: 0, atLeast: false, anchors: NO_ANCHORS };
    // Replaced (#1476): the last message seen is gone, so what arrived is
    // placed by time instead -- a floor, said as one -- or not at all.
    const floor = recordedAfter(messages, prev.tailAt);
    return {
      tailId,
      tailAt,
      newCount: floor === null ? null : (prev.newCount ?? 0) + floor,
      atLeast: floor !== null,
      anchors: NO_ANCHORS,
    };
  }
  const appended = messages.slice(k + 1);
  if (following) {
    const openers = appended.filter(opensTurn).map((m) => m.id);
    return {
      tailId,
      tailAt,
      newCount: 0,
      atLeast: false,
      anchors: openers.length ? new Set(openers) : NO_ANCHORS,
    };
  }
  return {
    tailId,
    tailAt,
    newCount: prev.newCount === null ? null : prev.newCount + appended.length,
    atLeast: prev.atLeast,
    anchors: NO_ANCHORS,
  };
}

/// The same bookkeeping once the reader is back at the live edge: nothing
/// is new any more.
export function caughtUp(prev: Arrivals): Arrivals {
  return prev.newCount === 0 && !prev.atLeast ? prev : { ...prev, newCount: 0, atLeast: false };
}

/// Drop the batch's anchors, for a window move the READER made (a reach
/// or a jump): see `Arrivals.anchors`.
export function withoutAnchors(prev: Arrivals): Arrivals {
  return prev.anchors.size === 0 ? prev : { ...prev, anchors: NO_ANCHORS };
}
