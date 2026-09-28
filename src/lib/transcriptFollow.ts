/// Live, paged, bounded reading of one transcript (#1476, epic #1473).
///
/// The viewer's data layer. It replaces `useClaudeTranscriptMessages`,
/// which re-read the newest 256 KB every ten seconds, with pages from
/// `claude_transcript_page` (#1220) joined by `mergeWindows`:
///
/// - **Open** on the newest page (`end`, `before`). Never a read from
///   byte 0: a 70 MB transcript is paged, not followed from the start.
/// - **Older** pages on the viewer's `onReachStart`, prepended.
/// - **Live growth** by reading the page AFTER the newest cursor held, on
///   an adaptive cadence. A tick that finds more than a page pages the
///   catch-up, `CATCH_UP_PAGES` at a time, rather than reading it whole.
/// - **Bounded memory**: at most `MAX_RESIDENT` messages are held. Pages
///   far from what the reader is looking at are let go and read again
///   when the reader scrolls back to them.
///
/// # Why polling, still
///
/// #1201 rejected a filesystem watcher: on macOS a dead FSEvents stream
/// says nothing, indistinguishably from "no new content". A poll that
/// stops is visible -- `lastReadAt` stops advancing -- and the follow
/// after a cursor costs one `stat` and three 4 KB digests when nothing
/// changed (`CURSOR_FINGERPRINT_BYTES`: the one checked, the two
/// returned), so a sub-second cadence is affordable (#1487 measured this
/// idle tick at 12 KiB and 0.02 ms, on a 70 MB file as on a 2 MB one).
///
/// # The cadence
///
/// | condition | next read in |
/// |---|---|
/// | the view is hidden, or the reader is far from the live edge | never (paused) |
/// | the session is not running | never, after the first page (stopped) |
/// | running, and the file grew in the last `RECENT_GROWTH_MS` | `FAST_MS` |
/// | otherwise | `IDLE_MIN_MS`, doubling to `IDLE_MAX_MS` |
/// | the last read failed | the idle backoff, and the state says so |
/// | a catch-up has more pages to read | at once |
/// | the desktop nudged: this transcript changed (#1477) | at once, if any row above but the first two would read |
///
/// Liveness `unknown` follows at the idle cadence only: nothing says the
/// session is writing, and nothing says it is not.
///
/// # Nudges (#1477)
///
/// The desktop stats running sessions' transcripts about once a second
/// and emits a content-free `claude-session-activity` when one changed.
/// `nudge` is how that reaches here: when the follow would read anyway
/// (running, visible, attached, not stopped), it reads NOW instead of
/// waiting out the rest of its delay. A nudge whose size is the size
/// already read changes nothing, and one arriving during a read makes
/// the next read immediate rather than starting a second one beside it.
///
/// A nudge is only ever a shortcut. The cadence above is unchanged, so a
/// nudge that never arrives -- lost to a lagging event stream, a
/// reconnect, a backgrounded phone -- costs the wait it would have cut,
/// and nothing else.
///
/// # Replacement, and re-anchoring on ids
///
/// A page answered from a cursor the file no longer matches comes back
/// `rewritten` (compaction or truncation rewrote history behind it), and
/// every page held is stale. The held pages are REPLACED, never spliced
/// onto. The reader is kept on the message they were looking at by ID:
/// if the replacement does not hold it, older pages are read until it
/// is found, within the residency bound, and only then is the new list
/// published -- so the viewer, which pins its window by id, never sees
/// an intermediate list that lost the reader's place.
///
/// # Residency, and which end is let go
///
/// Pages stay contiguous: `mergeWindows` refuses a gap, because a gap
/// would pair a result with a call across records nobody read. So when
/// the bound binds, a whole page is dropped from whichever END is
/// farther from the viewer's window, never one that holds a message the
/// window shows. Following at the bottom, that is the oldest page.
/// Reading far back, it is the newest -- and then the follow is detached
/// from the live edge (`paused`), because appending to a run that no
/// longer reaches it would leave a hole. Scrolling back down pages
/// forward again; reaching the end re-attaches.
///
/// Live growth is no exception. The page a tick appends is not one the
/// reader asked for, so it is not protected: a reader whose window holds
/// the oldest page -- parked on a new turn's prompt while output arrives
/// below it -- detaches the follow once the bound binds, exactly as
/// reading far back does (#1524). Protecting it left neither end
/// droppable, and memory grew with the session.
///
/// # States
///
/// Six, and none may render as another (#846, #1042, #1050):
///
/// - `loading` -- nothing read yet. A skeleton, not an empty transcript.
/// - `following` -- reading, and the transcript grew recently.
/// - `idle` -- reading, and it has not grown for `RECENT_GROWTH_MS`.
/// - `paused` -- NOT reading, by choice: the view is hidden, or the
///   reader is far from the live edge. We did not ask; it is not that
///   the session said nothing.
/// - `stopped` -- not following, because the session is not running.
/// - `could-not-read` -- the last read failed. Messages already held
///   stay; a transient failure is not the session stopping.

import { replaceEqualDeep } from "@tanstack/react-query";
import type { TranscriptMasking } from "../types/pr";
import type {
  PageCursor,
  RemoteTranscriptWindow,
  TranscriptMessage,
  TranscriptPageAnchor,
  TranscriptPageDirection,
  TranscriptPosition,
} from "../types/transcript";
import { mergeWindows } from "./transcriptPages";

/// One paged read: `claudeTranscriptPage` with the path (and the phone's
/// `reveal`) bound.
export type FetchPage = (
  anchor: TranscriptPageAnchor,
  direction: TranscriptPageDirection,
) => Promise<RemoteTranscriptWindow>;

/// The cadence while the session is writing. The issue's 750 ms-1 s.
export const FAST_MS = 750;
/// How long after the last growth the fast cadence holds.
export const RECENT_GROWTH_MS = 60_000;
/// The idle backoff's first step, and its ceiling.
export const IDLE_MIN_MS = 5_000;
export const IDLE_MAX_MS = 15_000;
/// The most messages held at once (#1476's "~2,000"). Replaces #1474's
/// interim 1,000-message cap on the old follow.
const MAX_RESIDENT = 2_000;
/// What a phone keeps when it is backgrounded: iOS reclaims memory from
/// suspended web views first, so the phone lets go of everything but the
/// reader's own pages then, and reads the rest again on return.
const PRESSURE_RESIDENT = 600;
/// Pages one tick reads before yielding to the next.
export const CATCH_UP_PAGES = 5;
/// How far before what is held a `seek` target may be and still be
/// reached by prepending pages rather than by replacing them: three
/// pages' worth of bytes (`transcript_page::PAGE_BYTES` is 256 KB).
const SEEK_NEAR_BYTES = 3 * 256 * 1024;
/// Pages one `loadOlderUntil` reads before it stops and lets the reader
/// ask again.
export const SEEK_PAGES = 10;

export type FollowStatus =
  | "loading"
  | "following"
  | "idle"
  | "paused"
  | "stopped"
  | "could-not-read";

/// Whether the session is writing, as liveness says.
export type FollowLive = "running" | "unknown" | "not-running";

/// Loading older messages, which can fail on its own without the follow
/// failing.
type OlderState = { state: "idle" } | { state: "loading" } | { state: "failed"; error: unknown };

export interface FollowSnapshot {
  /// `undefined` until the first page has been read -- Pending, which is
  /// not an empty transcript.
  messages: readonly TranscriptMessage[] | undefined;
  status: FollowStatus;
  /// The last read's failure, or `null` when it succeeded.
  error: unknown;
  /// When a read last succeeded (epoch ms), or `null` before one did.
  lastReadAt: number | null;
  /// Older messages exist that are not held: never read, or let go.
  hasOlder: boolean;
  /// The held messages reach the newest the file had at the last read.
  atLiveEdge: boolean;
  older: OlderState;
  /// The file's size at the last read.
  fileBytes: number | null;
  /// On a phone: what the desktop masked across every held page.
  masking: TranscriptMasking | undefined;
  /// Bumped each time the held list was REPLACED rather than extended.
  replacements: number;
  /// Where the held messages sit in the whole transcript, from the
  /// first page's start to the last page's end. Label it with
  /// `positionLabel`, which says "estimate" when it is one.
  position: TranscriptPosition | null;
}

export interface FollowConfig {
  /// Most messages held; `MAX_RESIDENT` by default.
  maxResident?: number;
  /// What `relievePressure` keeps.
  pressureResident?: number;
  /// Open at this message id when it is within reach (#1486's deep link).
  openAt?: string | null;
  now?: () => number;
}

type Viewport = { first: string; last: string };

const IDLE_OLDER: OlderState = { state: "idle" };

function cursor(c: PageCursor): TranscriptPageAnchor {
  return { kind: "cursor", offset: c.offset, behind_digest: c.behind_digest };
}

function count(pages: readonly RemoteTranscriptWindow[]): number {
  let n = 0;
  for (const p of pages) n += p.page.messages.length;
  return n;
}

/// The page each message id the viewer can report came from: a page's
/// own messages, the results absorbed into its calls (a merge may stand
/// them back up), and the derived model-change marker before a message.
function pageOf(pages: readonly RemoteTranscriptWindow[], id: string): number {
  const bare = id.endsWith("/model") ? id.slice(0, -"/model".length) : id;
  for (let i = 0; i < pages.length; i++) {
    for (const m of pages[i].page.messages) {
      if (m.id === bare) return i;
      for (const b of m.blocks) {
        if (b.kind === "tool_call" && b.result !== null && b.result.message_id === bare) return i;
      }
    }
  }
  return -1;
}

export class TranscriptFollower {
  private pages: RemoteTranscriptWindow[] = [];
  private attached = true;
  private lastGrowthAt: number | null = null;
  private idleDelay = IDLE_MIN_MS;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private queue: Promise<void> = Promise.resolve();
  private viewport: Viewport | null = null;
  private live: FollowLive = "not-running";
  private visible = true;
  private running = false;
  private lastMasking: TranscriptMasking | undefined = undefined;
  /// Reads queued or in flight.
  private busy = 0;
  /// A nudge arrived while a read was queued or in flight.
  private nudged = false;
  private snap: FollowSnapshot = {
    messages: undefined,
    status: "loading",
    error: null,
    lastReadAt: null,
    hasOlder: false,
    atLiveEdge: true,
    older: IDLE_OLDER,
    fileBytes: null,
    masking: undefined,
    replacements: 0,
    position: null,
  };
  /// The snapshot subscribers were last given. `snap` is the working
  /// copy: `read` writes `lastReadAt` into it before `publish` runs, so
  /// comparing against `snap` would miss an idle read, and "Last read
  /// at" would stop advancing while the poll kept going (#1525).
  private delivered: FollowSnapshot = this.snap;
  private readonly listeners = new Set<() => void>();
  private readonly max: number;
  private readonly pressure: number;
  private readonly now: () => number;
  private openAt: string | null;

  constructor(
    private readonly fetchPage: FetchPage,
    config: FollowConfig = {},
  ) {
    this.max = config.maxResident ?? MAX_RESIDENT;
    this.pressure = config.pressureResident ?? PRESSURE_RESIDENT;
    this.now = config.now ?? Date.now;
    this.openAt = config.openAt ?? null;
  }

  // ---- the store, for `useSyncExternalStore` ----

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): FollowSnapshot => this.delivered;

  // ---- conditions ----

  /// Start (or resume) reading. Idempotent: StrictMode's second effect
  /// run, and a re-render with the same conditions, change nothing.
  start(): void {
    if (this.running) return;
    this.running = true;
    this.kick();
  }

  /// Stop reading. A read in flight still lands -- what it retrieved is
  /// an answer -- but nothing further is scheduled.
  stop(): void {
    this.running = false;
    this.clearTimer();
    this.publish();
  }

  setLive(live: FollowLive): void {
    if (live === this.live) return;
    const wasOff = this.live !== "running";
    this.live = live;
    // A session that started running is presumed to be writing.
    if (live === "running" && wasOff) this.lastGrowthAt = this.now();
    this.kick();
  }

  setVisible(visible: boolean): void {
    if (visible === this.visible) return;
    this.visible = visible;
    this.kick();
  }

  /// What the viewer has mounted, by id. Only eviction reads it, so it
  /// changes nothing on its own.
  setViewport(first: string, last: string): void {
    this.viewport = { first, last };
  }

  /// The desktop saw this transcript change (#1477): read now rather
  /// than at the end of the current delay, if the follow would read at
  /// all. `size` is the file's size when it was seen; a nudge for a size
  /// already read is ignored.
  nudge(size: number | null = null): void {
    if (!this.running || !this.visible || this.pages.length === 0 || !this.follows()) return;
    if (size !== null && size === this.snap.fileBytes) return;
    if (this.busy > 0) {
      this.nudged = true;
      return;
    }
    this.clearTimer();
    void this.enqueue(() => this.tick());
  }

  // ---- the reader's actions ----

  /// The viewer reached the oldest message held.
  loadOlder(): Promise<void> {
    return this.enqueue(() => this.older());
  }

  /// The viewer reached the newest message held. At the live edge the
  /// follow's own cadence brings the next messages; away from it, this
  /// reads the next page forward.
  loadNewer(): Promise<void> {
    if (this.attached) return Promise.resolve();
    return this.enqueue(() => this.newer());
  }

  /// "Jump to latest" from a run that no longer reaches the live edge:
  /// open on the newest page again.
  jumpToLatest(): Promise<void> {
    if (this.attached) return Promise.resolve();
    return this.enqueue(async () => {
      const w = await this.read({ kind: "end" }, "before");
      if (w === null) return;
      this.pages = [w];
      this.attached = true;
      this.viewport = null;
      this.bump();
      this.publish();
    });
  }

  /// Hold the message `id` (#1484): a turn chosen from the outline, a
  /// find's hit, the "since you left" marker. Resolves to whether it is
  /// held once the read settles, so the caller scrolls only to a row
  /// that exists.
  ///
  /// Already held: nothing is read. Within `SEEK_NEAR_BYTES` before
  /// what is held: older pages are prepended until it is, so the run the
  /// reader was in stays whole. Otherwise, with `at` (the hit's cursor):
  /// the held pages are REPLACED by the page starting at it, detached
  /// from the live edge -- the reader pages on from there in either
  /// direction, and "jump to latest" re-attaches. A cursor the file no
  /// longer matches comes back `rewritten`, which is a replacement from
  /// the end as any other.
  seek(id: string, at: PageCursor | null): Promise<boolean> {
    return this.enqueue(async () => {
      if (this.pages.length === 0) return;
      if (pageOf(this.pages, id) >= 0) return;
      const first = this.pages[0];
      const near =
        at === null || (at.offset < first.start.offset && first.start.offset - at.offset <= SEEK_NEAR_BYTES);
      if (near) await this.reachBack(id);
      if (pageOf(this.pages, id) < 0 && at !== null) {
        const w = await this.read(cursor(at), "after");
        if (w === null) return;
        if (w.rewritten) {
          await this.replace(w);
          await this.reachBack(id);
        } else {
          this.pages = [w];
          this.attached = w.at_end;
          this.viewport = null;
          this.bump();
        }
      }
      this.publish();
    }).then(() => this.pages.length > 0 && pageOf(this.pages, id) >= 0);
  }

  /// Read older pages until one holds a message `wanted` accepts, the
  /// start of the file is reached, or `SEEK_PAGES` pages were read --
  /// "Load earlier" for a result whose call is not held, and the
  /// previous turn when none is (#1484). Resolves to whether one is
  /// held. Stopping at `SEEK_PAGES` is not the end: the next call goes
  /// on from there, so the button that asked still works.
  loadOlderUntil(wanted: (m: TranscriptMessage) => boolean): Promise<boolean> {
    const held = () => this.pages.some((p) => p.page.messages.some(wanted));
    return this.enqueue(async () => {
      if (this.pages.length === 0 || held()) return;
      this.snap = { ...this.snap, older: { state: "loading" } };
      this.publish();
      for (let i = 0; i < SEEK_PAGES && !held() && !this.pages[0].at_start; i++) {
        const first = this.pages[0];
        try {
          const w = await this.fetchPage(cursor(first.start), "before");
          if (w.masking !== undefined) this.lastMasking = w.masking;
          this.snap = { ...this.snap, older: IDLE_OLDER, lastReadAt: this.now() };
          if (w.rewritten || w.end.offset !== first.start.offset) {
            await this.replace(w);
            break;
          }
          this.pages = [w, ...this.pages];
          this.evict(this.max, "head");
        } catch (e) {
          this.snap = { ...this.snap, older: { state: "failed", error: e } };
          break;
        }
      }
      if (this.snap.older.state === "loading") this.snap = { ...this.snap, older: IDLE_OLDER };
      this.publish();
    }).then(held);
  }

  /// Read now, whatever the cadence says: pull to refresh, "Try again".
  refresh(): Promise<void> {
    this.clearTimer();
    return this.enqueue(() => this.tick());
  }

  /// Let go of everything but the reader's own pages (the phone, when
  /// backgrounded). Read again as the reader scrolls.
  relievePressure(): void {
    if (this.pages.length === 0) return;
    const before = this.pages.length;
    this.evict(this.pressure, null);
    if (this.pages.length !== before) this.publish();
  }

  // ---- internals ----

  private enqueue(op: () => Promise<void>): Promise<void> {
    this.busy++;
    const next = this.queue.then(op).finally(() => {
      this.busy--;
      this.schedule();
    });
    // The queue itself never rejects: each op records its own failure.
    this.queue = next.catch(() => undefined);
    return next;
  }

  private clearTimer(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  /// Re-decide the next read after a condition changed.
  private kick(): void {
    this.clearTimer();
    if (!this.running) return;
    if (this.visible && (this.pages.length === 0 || this.follows())) {
      void this.enqueue(() => this.tick());
    } else {
      this.publish();
    }
  }

  private follows(): boolean {
    return this.attached && this.live !== "not-running";
  }

  /// The delay before the next read, or `null` for none.
  private delay(): number | null {
    if (!this.running || !this.visible) return null;
    if (this.pages.length === 0) return this.idleDelay; // retrying the first read
    if (!this.follows()) return null;
    // Before the failure backoff: the file changed, which is worth a
    // read even after one failed -- and nudges come at most once a second.
    if (this.nudged) return 0;
    if (this.snap.error !== null) return this.idleDelay;
    if (this.catchingUp) return 0;
    if (this.live === "running" && this.recentlyGrew()) return FAST_MS;
    return this.idleDelay;
  }

  private catchingUp = false;

  private recentlyGrew(): boolean {
    return this.lastGrowthAt !== null && this.now() - this.lastGrowthAt < RECENT_GROWTH_MS;
  }

  private schedule(): void {
    this.clearTimer();
    const d = this.delay();
    this.publish();
    if (d === null) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.enqueue(() => this.tick());
    }, d);
  }

  /// One read, recording its outcome. `null` on failure.
  private async read(
    anchor: TranscriptPageAnchor,
    direction: TranscriptPageDirection,
  ): Promise<RemoteTranscriptWindow | null> {
    try {
      const w = await this.fetchPage(anchor, direction);
      this.snap = { ...this.snap, error: null, lastReadAt: this.now(), fileBytes: w.page.file_bytes };
      if (w.masking !== undefined) this.lastMasking = w.masking;
      return w;
    } catch (e) {
      this.snap = { ...this.snap, error: e };
      this.backOff();
      return null;
    }
  }

  private backOff(): void {
    this.idleDelay = Math.min(IDLE_MAX_MS, this.idleDelay * 2);
  }

  private async tick(): Promise<void> {
    if (this.pages.length === 0) {
      await this.open();
      return;
    }
    if (!this.attached) return;
    // This read covers every nudge before it; one arriving during it
    // sets the flag again.
    this.nudged = false;
    let grew = false;
    this.catchingUp = false;
    for (let i = 0; i < CATCH_UP_PAGES; i++) {
      const last = this.pages[this.pages.length - 1];
      const w = await this.read(cursor(last.end), "after");
      if (w === null) return;
      if (w.rewritten || w.start.offset !== last.end.offset) {
        await this.replace(w);
        grew = true;
        break;
      }
      if (w.end.offset === w.start.offset) break; // nothing new
      grew = true;
      this.append(w);
      // Nothing protects the page just appended: the reader did not ask
      // for it. Where their window holds the oldest page, the newest is
      // the one that goes, and the follow detaches (#1524) -- protecting
      // it too left neither end droppable, and growth held without bound.
      this.evict(this.max, null);
      if (!this.attached) break;
      if (w.at_end) break;
      if (i === CATCH_UP_PAGES - 1) this.catchingUp = true;
    }
    if (grew) {
      this.lastGrowthAt = this.now();
      this.idleDelay = IDLE_MIN_MS;
    } else if (!this.recentlyGrew()) {
      // Backed off one step per quiet read, once the fast window closed.
      if (this.snap.status === "idle") this.backOff();
    }
    this.publish();
  }

  /// The first page: the newest, and then back to `openAt` if asked.
  private async open(): Promise<void> {
    const w = await this.read({ kind: "end" }, "before");
    if (w === null) {
      this.publish();
      return;
    }
    this.idleDelay = IDLE_MIN_MS;
    this.pages = [w];
    this.attached = true;
    if (this.live === "running") this.lastGrowthAt = this.now();
    const target = this.openAt;
    this.openAt = null;
    if (target !== null) await this.reachBack(target);
    this.publish();
  }

  /// Replace every held page with `w` (read from the end), keeping the
  /// reader on the message they were looking at if it can be reached.
  private async replace(w: RemoteTranscriptWindow): Promise<void> {
    const target = this.viewport?.first ?? null;
    this.pages = [w];
    this.attached = true;
    if (target !== null) await this.reachBack(target);
    this.bump();
  }

  /// Prepend older pages until `id` is held, the start is reached, or
  /// the bound would bind. Does not publish: the caller publishes once,
  /// so no intermediate list is ever seen.
  private async reachBack(id: string): Promise<void> {
    while (pageOf(this.pages, id) < 0 && !this.pages[0].at_start && count(this.pages) < this.max) {
      const first = this.pages[0];
      const w = await this.read(cursor(first.start), "before");
      if (w === null || w.rewritten || w.end.offset !== first.start.offset) return;
      this.pages = [w, ...this.pages];
    }
  }

  private async older(): Promise<void> {
    if (this.pages.length === 0 || this.pages[0].at_start) return;
    this.snap = { ...this.snap, older: { state: "loading" } };
    this.publish();
    const first = this.pages[0];
    try {
      const w = await this.fetchPage(cursor(first.start), "before");
      if (w.masking !== undefined) this.lastMasking = w.masking;
      this.snap = { ...this.snap, older: IDLE_OLDER, lastReadAt: this.now() };
      if (w.rewritten || w.end.offset !== first.start.offset) {
        await this.replace(w);
      } else {
        this.pages = [w, ...this.pages];
        this.evict(this.max, "head");
      }
    } catch (e) {
      this.snap = { ...this.snap, older: { state: "failed", error: e } };
    }
    this.publish();
  }

  private async newer(): Promise<void> {
    if (this.pages.length === 0 || this.attached) return;
    const last = this.pages[this.pages.length - 1];
    const w = await this.read(cursor(last.end), "after");
    if (w === null) {
      this.publish();
      return;
    }
    if (w.rewritten || w.start.offset !== last.end.offset) {
      await this.replace(w);
    } else {
      this.append(w);
      if (w.at_end) this.attached = true;
      this.evict(this.max, "tail");
    }
    this.publish();
  }

  private append(w: RemoteTranscriptWindow): void {
    const last = this.pages[this.pages.length - 1];
    if (w.page.messages.length === 0) {
      // Only bookkeeping records: move the end on, without a page that
      // holds nothing.
      this.pages = [...this.pages.slice(0, -1), { ...last, end: w.end, at_end: w.at_end }];
    } else {
      this.pages = [...this.pages, w];
    }
  }

  /// Drop whole pages until at most `max` messages are held, from the
  /// end farther from the viewer's window. Never a page the window
  /// shows, and never the page just `loaded` -- the reader asked for it.
  /// When both of those forbid every drop the bound is exceeded by that
  /// page until the reader moves on, rather than losing what they are
  /// reading.
  private evict(max: number, loaded: "head" | "tail" | null): void {
    while (count(this.pages) > max && this.pages.length > 1) {
      const n = this.pages.length;
      const v = this.viewport;
      const a = v === null ? -1 : pageOf(this.pages, v.first);
      const b = v === null ? -1 : pageOf(this.pages, v.last);
      const shown = a >= 0 && b >= 0 ? { lo: Math.min(a, b), hi: Math.max(a, b) } : null;
      const canHead = loaded !== "head" && (shown === null || shown.lo > 0);
      const canTail = loaded !== "tail" && (shown === null || shown.hi < n - 1);
      if (!canHead && !canTail) break;
      // Both possible: the farther end goes; a tie, or nothing shown,
      // keeps the live edge.
      const dropTail =
        canHead && canTail ? shown !== null && n - 1 - shown.hi > shown.lo : canTail;
      if (dropTail) {
        this.pages = this.pages.slice(0, -1);
        this.attached = false;
      } else {
        this.pages = this.pages.slice(1);
      }
    }
  }

  private bump(): void {
    this.snap = { ...this.snap, replacements: this.snap.replacements + 1 };
  }

  private status(): FollowStatus {
    if (this.pages.length === 0) {
      return this.snap.error !== null ? "could-not-read" : "loading";
    }
    if (this.snap.error !== null) return "could-not-read";
    if (!this.attached || !this.running || !this.visible) return "paused";
    if (this.live === "not-running") return "stopped";
    if (this.live === "running" && this.recentlyGrew()) return "following";
    return "idle";
  }

  private position(): TranscriptPosition | null {
    const first = this.pages[0];
    const last = this.pages[this.pages.length - 1];
    if (first === undefined) return null;
    const a = first.position;
    const b = last.position;
    // Unchanged when nothing moved, so the snapshot compares equal.
    const held = this.snap.position;
    const next: TranscriptPosition = {
      first: a.first,
      last: b.last,
      total: b.total,
      exact: this.pages.every((p) => p.position.exact),
      basis: b.basis,
    };
    return held !== null && JSON.stringify(held) === JSON.stringify(next) ? held : next;
  }

  private masking(): TranscriptMasking | undefined {
    let any = false;
    let hidden = 0;
    let revealed = true;
    let withheld = false;
    for (const p of this.pages) {
      if (p.masking === undefined) continue;
      any = true;
      hidden += p.masking.hidden;
      revealed = revealed && p.masking.revealed;
      withheld = withheld || p.masking.withheld;
    }
    const latest = this.lastMasking;
    if (!any && latest === undefined) return undefined;
    return {
      hidden,
      revealed: any ? revealed : (latest?.revealed ?? false),
      // What the desktop says NOW: the latest answer's.
      reveal_allowed: latest?.reveal_allowed ?? false,
      withheld: withheld || (latest?.withheld ?? false),
    };
  }

  private merged: { pages: RemoteTranscriptWindow[]; messages: TranscriptMessage[] } | null = null;

  private publish(): void {
    let messages = this.snap.messages;
    if (this.pages.length > 0 && this.merged?.pages !== this.pages) {
      // Unchanged messages keep their identity, so the rows that show
      // them do not re-render; a message that did change -- a call whose
      // result arrived, the last message updated in place -- is new.
      const next = replaceEqualDeep(this.snap.messages ?? [], mergeWindows(this.pages));
      this.merged = { pages: this.pages, messages: next };
      messages = next;
    }
    const first = this.pages[0];
    const next: FollowSnapshot = {
      ...this.snap,
      messages,
      status: this.status(),
      hasOlder: first !== undefined && !first.at_start,
      atLiveEdge: this.attached,
      masking: this.masking(),
      position: this.position(),
    };
    this.snap = next;
    // Against what was delivered, not the working copy (#1525). An idle
    // read changes only `lastReadAt`; `messages` keeps its identity, so
    // no row re-renders.
    if (shallowEqual(next, this.delivered)) return;
    this.delivered = next;
    for (const l of this.listeners) l();
  }
}

function shallowEqual(a: FollowSnapshot, b: FollowSnapshot): boolean {
  const ka = Object.keys(a) as (keyof FollowSnapshot)[];
  for (const k of ka) {
    if (k === "masking") {
      if (JSON.stringify(a.masking) !== JSON.stringify(b.masking)) return false;
    } else if (a[k] !== b[k]) {
      return false;
    }
  }
  return true;
}
