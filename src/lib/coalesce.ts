/// Batching a burst of streamed events into one state update (#1150).
///
/// Both streaming hooks called `setState` per event with a fresh `Map`,
/// which is right for React identity and wrong per event: on a machine
/// with ~295 worktrees the initial burst is ~295 full re-renders, each
/// rebuilding every derived row and re-sorting the list. The page feels
/// unresponsive precisely while it is filling, which is when the user is
/// watching it.
///
/// # Why a scheduler is injected
///
/// So the flush is deterministic in a test. A hard-coded
/// `requestAnimationFrame` cannot be driven by fake timers, and a
/// hard-coded `setTimeout` measures a delay rather than the batching --
/// which is the property that matters. `splash.ts`'s tests make the same
/// choice for the same reason.
export type Scheduler = (flush: () => void) => () => void;

/// The default: one commit per animation frame.
///
/// A frame rather than a fixed delay, because the cost being avoided is
/// a RENDER, and a frame is exactly the interval in which more than one
/// render is wasted. Falls back to a ~33ms timeout where
/// `requestAnimationFrame` does not exist -- jsdom, and any headless
/// context.
const frameScheduler: Scheduler = (flush) => {
  if (typeof requestAnimationFrame === "function") {
    const id = requestAnimationFrame(() => flush());
    return () => cancelAnimationFrame(id);
  }
  const id = setTimeout(flush, 33);
  return () => clearTimeout(id);
};

/// Accumulate values and commit them in batches.
///
/// `push` buffers; the scheduler decides when `commit` runs. The buffer
/// is handed over WHOLE rather than one entry at a time, so a caller
/// building a `Map` allocates one per batch instead of one per event.
///
/// Ordering is preserved, and later entries for one key win -- the same
/// last-write-wins a per-event `Map.set` gave, so no caller has to think
/// about it.
export function createCoalescer<T>(
  commit: (batch: T[]) => void,
  schedule: Scheduler = frameScheduler,
) {
  let buffer: T[] = [];
  let cancel: (() => void) | null = null;

  const flush = () => {
    cancel = null;
    if (buffer.length === 0) return;
    // Taken BEFORE commit, so an event arriving during the commit lands
    // in the next batch rather than being dropped or double-counted.
    const batch = buffer;
    buffer = [];
    commit(batch);
  };

  return {
    push(value: T) {
      buffer.push(value);
      // Armed on the FIRST event of a burst, not on every one:
      // re-arming per event would push the flush ever further out under
      // a sustained stream and the list would never settle.
      if (cancel === null) cancel = schedule(flush);
    },
    /// Flush anything buffered and stop. Called on unmount, so a batch
    /// in flight is not silently lost when the page closes.
    stop() {
      if (cancel) {
        cancel();
        cancel = null;
      }
      buffer = [];
    },
    /// For tests: how many entries are waiting.
    pending() {
      return buffer.length;
    },
  };
}
