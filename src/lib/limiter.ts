/// At most N calls in flight at once, the rest waiting their turn (#1459).
///
/// # Why the phone needs this and the desktop does not
///
/// The All repositories view fires one `size_worktrees` per repository,
/// all at once. The desktop admits a few at a time behind its scan
/// permits and has no deadline, so the last one simply finishes last --
/// "the full set takes ~2 minutes" is the page's own measurement. The
/// phone's calls die at the companion's 120s `CALL_TIMEOUT`
/// (`src-mobile/src/client.rs`), and that clock starts when the call is
/// SENT, not when the desktop starts walking. So a call queued behind two
/// minutes of other walks times out before it measures anything, and the
/// row that wanted it goes from a skeleton to "not measured".
///
/// Queueing on the phone instead means each call's 120s is spent on its
/// own walk. The disk is the bottleneck either way, so the total is no
/// slower.
///
/// # Keys and `promote`
///
/// Each job carries a key so the view the user is LOOKING at can go to
/// the front: opening a repository while the all-repositories fan-out is
/// still queued should not wait behind thirty other repositories.
///
/// A queued job is still in flight as far as its caller can tell -- the
/// promise has not settled -- so a query waiting here reads `isFetching`.
/// That is honest: a number is coming.
export interface Limiter {
  /// Run `task` once a slot is free. Settles exactly as `task` does.
  run<T>(key: string, task: () => Promise<T>): Promise<T>;
  /// Move every queued job with this key to the front. A job already
  /// running, or no job at all, is left alone.
  promote(key: string): void;
}

export function createLimiter(concurrency: number): Limiter {
  interface Job {
    key: string;
    start: () => void;
  }
  const queue: Job[] = [];
  let running = 0;

  const pump = () => {
    while (running < concurrency && queue.length > 0) {
      const job = queue.shift() as Job;
      running += 1;
      job.start();
    }
  };

  return {
    run<T>(key: string, task: () => Promise<T>): Promise<T> {
      return new Promise<T>((resolve, reject) => {
        queue.push({
          key,
          start: () => {
            // `new Promise` around the call rather than `task()` bare: a
            // task that throws synchronously must still free its slot,
            // or one bad call would shrink the pool for the life of the
            // page.
            new Promise<T>((res) => res(task()))
              .then(resolve, reject)
              .finally(() => {
                running -= 1;
                pump();
              });
          },
        });
        pump();
      });
    },
    promote(key: string) {
      const mine = queue.filter((j) => j.key === key);
      if (mine.length === 0) return;
      const rest = queue.filter((j) => j.key !== key);
      queue.splice(0, queue.length, ...mine, ...rest);
    },
  };
}

/// Reject if `promise` has not settled within `ms`.
///
/// For a call whose transport already has its own deadline, this is the
/// backstop for that deadline's answer never arriving -- a promise that
/// never settles leaves a query fetching forever, which renders as a
/// skeleton nothing will ever move (#1042, #1459). The timer is cleared
/// on settlement, so a call that answers costs nothing.
export function withDeadline<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    promise.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e: unknown) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}
