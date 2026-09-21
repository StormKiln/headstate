import type { ClaudeMdAdviceResult } from "@/types/pr";

/// What the advice tab is showing, composed from TWO queries (#1290).
///
/// # Why this is a function and not a render-time ternary chain
///
/// The tab is driven by two calls to the same command with different
/// `mode`s, and the interesting states are combinations of the two.
/// Written inline the chain is six nested conditionals with the cached
/// query's `isError` and the fresh query's `isFetching` interleaved, and
/// the arm that matters most -- "a cached report is on screen while a
/// fresh run is in flight" -- is the one easiest to lose to a stray
/// `&&`. Here it is one exhaustive union a test can enumerate.
///
/// # The state the backend deliberately does not have
///
/// `ClaudeMdAdviceFreshness` has no `"refreshing"` member, and
/// `types/pr.ts` says why: one synchronous call cannot be both the
/// cached answer and the running one, and a backend claiming "a refresh
/// is happening" would be a claim about a future it cannot observe.
///
/// "From cache, refreshing" is therefore composed HERE, where both facts
/// are actually in hand: the cached call's `{ state: "cached", stale:
/// true }` is a fact the backend observed, and "a fresh call is in
/// flight" is a fact this client observed about its own request. Neither
/// half is invented.
///
/// # What must not collapse
///
/// "Never asked" and "asked and got nothing" are different claims with
/// different remedies (#846), and a tab the user has not visited while
/// the fetch is running is BUILDING, not empty. `"idle"` therefore means
/// only one thing -- no query has been enabled -- and every other
/// absence of a report is `"building"`.
export type AdviceState =
  /// No query has been asked. Reached only before a repository is
  /// selected: selecting one starts the cached call (#1290).
  | { kind: "idle" }
  /// Asked, nothing to show yet. The producers are running for the first
  /// time on this repository, or the store had nothing to serve. NOT
  /// "no findings": a query in flight has established nothing.
  | { kind: "building" }
  /// A report, and the honest account of where it came from. `refreshing`
  /// is true only while a `"fresh"` call this client issued is actually
  /// in flight -- the composed third state.
  | { kind: "report"; result: ClaudeMdAdviceResult; refreshing: boolean }
  /// The command was rejected. A failure, never "nothing found" (#846).
  /// `stale` carries a report that is still on screen underneath, when
  /// the failure was a refresh of something already served.
  | { kind: "failed"; error: unknown; stale: ClaudeMdAdviceResult | undefined };

/// One query's observable state, as this module needs it. A structural
/// subset of what `useQuery` returns, so the composer can be tested
/// without React and without TanStack.
export interface AdviceQuery {
  data: ClaudeMdAdviceResult | undefined;
  isError: boolean;
  error: unknown;
  isFetching: boolean;
  /// Whether the query is switched on at all. A disabled query is not a
  /// query that answered nothing.
  enabled: boolean;
}

/// Whether a served result is one a refresh could improve.
///
/// TRUE for `{ state: "cached", stale: true }`, which is the backend
/// saying a tracked input changed since the stored run -- the report is
/// real, and a newer one exists to be computed.
///
/// FALSE for `"unverified"`, and this is the load-bearing case. An
/// unverified result means an input could not be READ, so a second run
/// would hit the same unreadable input and return the same unverified
/// answer. Auto-firing a full producer run against it would spend the
/// whole-body read of every session under the repository to learn
/// nothing -- the cost this issue exists to avoid. The user can still
/// press Refresh; what is refused is doing it unprompted, forever, on
/// every visit to a repository with one unreadable file in it.
///
/// FALSE for a fresh or a current cached report, which have nothing to
/// improve on.
export function needsRefresh(result: ClaudeMdAdviceResult | undefined): boolean {
  return result?.freshness.state === "cached" && result.freshness.stale;
}

/// Compose the two queries into the one thing the tab renders.
///
/// The cached call leads, always. It is the one that answers at cache
/// speed, and the fresh call exists only to replace what it served.
///
/// Ordering, and why each arm is where it is:
///
/// 1. A FRESH result wins outright when it has landed. It is strictly
///    newer than whatever the cached call served, so preferring the
///    cached one here would show an older report than the client
///    already holds.
/// 2. A cached result is then shown, with `refreshing` true exactly
///    while the fresh call is in flight. This is the composed state.
/// 3. A failure is only a failure when there is NOTHING to show. A
///    refresh that was rejected over a report already on screen keeps
///    the report -- withdrawing a real answer because the attempt to
///    better it failed serves nobody -- but says the failure happened,
///    via `stale`, so the surface can report it without blanking.
/// 4. `"building"` covers every remaining asked-and-unanswered case, and
///    `"idle"` only the never-asked one.
export function adviceState(cached: AdviceQuery, fresh: AdviceQuery): AdviceState {
  if (fresh.data !== undefined) {
    return { kind: "report", result: fresh.data, refreshing: false };
  }
  if (cached.data !== undefined) {
    // `isFetching` rather than `isLoading`, because the fresh query may
    // have been asked before: a second refresh of an already-refreshed
    // repository is still a refresh in flight, and `isLoading` is false
    // for it.
    //
    // `fresh.enabled` is checked too. A disabled query reports
    // `isFetching: false`, but relying on that would make this arm
    // depend on a TanStack implementation detail to stay honest about a
    // claim the user reads as "something is happening right now".
    const refreshing = fresh.enabled && fresh.isFetching;
    // A refresh that failed over a served report is reported as a
    // failure WITH the report, not as a bare report -- silently swapping
    // "refreshing" for "from cache" would make a rejected refresh
    // indistinguishable from one that never ran.
    if (fresh.isError) {
      return { kind: "failed", error: fresh.error, stale: cached.data };
    }
    return { kind: "report", result: cached.data, refreshing };
  }
  if (cached.isError) {
    return { kind: "failed", error: cached.error, stale: undefined };
  }
  // The fresh call can fail while the cached one has not answered --
  // a manual Refresh pressed during the first build. That is a real
  // failure with nothing underneath it.
  if (fresh.isError) {
    return { kind: "failed", error: fresh.error, stale: undefined };
  }
  if (cached.enabled || fresh.enabled) return { kind: "building" };
  return { kind: "idle" };
}
