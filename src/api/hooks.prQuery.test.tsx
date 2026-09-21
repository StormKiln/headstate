import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

/// Every `(repo, number)` pair the backend was asked about.
///
/// The POINT of the feature's cost story: ordinary prose must not reach
/// this, and typing a reference must not reach it once per character.
const asked = vi.hoisted(() => [] as string[]);
const answer = vi.hoisted(
  () =>
    ({
      value: [] as unknown,
      reject: null as string | null,
    }) as { value: unknown; reject: string | null },
);

vi.mock("./tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  claudeSessionsForPr: (repo: string, number: number) => {
    asked.push(`${repo}#${number}`);
    return answer.reject !== null
      ? Promise.reject(new Error(answer.reject))
      : Promise.resolve(answer.value);
  },
  // The tracked pull request list, which is how a bare `#1234` learns
  // its repository. Served rather than fetched: `staleTime: Infinity`
  // means the real hook reads it from cache, and seeding the cache
  // below is the same thing one layer down.
  getPullRequests: () => Promise.resolve([]),
}));

import { useClaudeSessionsForPrQuery, type PrQueryState } from "./hooks";

/// The hook's answer, rendered as JSON so an assertion can read the
/// whole five-state value rather than a flag derived from it.
function Probe({ query }: { query: string }) {
  const q: PrQueryState = useClaudeSessionsForPrQuery(query, true);
  return <output data-testid="out">{JSON.stringify(q)}</output>;
}

let qc: QueryClient;

function wrap(node: ReactNode) {
  return <QueryClientProvider client={qc}>{node}</QueryClientProvider>;
}

const got = (): PrQueryState => JSON.parse(screen.getByTestId("out").textContent ?? "null");

/// Seed the tracked pull request cache, which is what `reposForNumber`
/// reads. The real `usePullRequests` fills this once with
/// `staleTime: Infinity`, so writing it here is the same state the app
/// is in by the time anyone has typed into the search box.
const trackPrs = (prs: { repo: string; number: number }[]) => qc.setQueryData(["prs"], prs);

beforeEach(() => {
  vi.useFakeTimers();
  asked.length = 0;
  answer.value = [];
  answer.reject = null;
  // `retry: false` matches what the hook sets per query. `gcTime` is
  // left at react-query's DEFAULT rather than zeroed: the "returns to a
  // reference" test below is about the cache surviving a detour, and a
  // `gcTime: 0` client evicts on unmount and would make that test assert
  // the opposite of what the app does.
  qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
});

afterEach(() => {
  vi.useRealTimers();
  qc.clear();
});

/// Let the debounce land and every resolved promise settle.
async function settle() {
  // The debounce first, then several microtask drains: the query fires
  // inside an effect, resolves a promise, and react-query commits the
  // result in a further tick. One drain reaches `loading` and stops.
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400);
  });
  for (let i = 0; i < 5; i++) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
  }
}

describe("useClaudeSessionsForPrQuery", () => {
  /// **The cost claim.** Ordinary prose is rejected by the parse and
  /// never starts a timer, let alone a command.
  ///
  /// SABOTAGE: drop the `parsePrQuery` guard and send the raw query --
  /// `asked` fills with `#NaN` entries and this fails.
  it("does not reach the backend for a query that is not a pull request", async () => {
    render(wrap(<Probe query="notarization" />));
    await settle();
    expect(asked).toEqual([]);
    expect(got()).toEqual({ state: "off" });
  });

  /// **The debounce.** Typing `acme/api#1234` passes through prefixes
  /// that parse -- `acme/api#1`, `acme/api#12`, `acme/api#123` -- and
  /// only the one the typing settled on is asked.
  ///
  /// SABOTAGE: remove the `setTimeout` and set `settled` directly, and
  /// `asked` holds four entries rather than one.
  it("asks once for a reference the user typed a character at a time", async () => {
    const { rerender } = render(wrap(<Probe query="acme/api#1" />));
    for (const q of ["acme/api#12", "acme/api#123", "acme/api#1234"]) {
      act(() => {
        vi.advanceTimersByTime(40);
      });
      rerender(wrap(<Probe query={q} />));
    }
    await settle();
    expect(asked).toEqual(["acme/api#1234"]);
  });

  /// A lookup that ran and found nothing. `done` with an empty `links`
  /// is a FINDING, and the caller words it as one.
  it("reports an empty lookup as a completed lookup", async () => {
    answer.value = [];
    render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(got()).toEqual({ state: "done", ref: "acme/api#1234", links: [] });
  });

  /// A lookup that REJECTED. Never `done`, and it carries the reason.
  ///
  /// SABOTAGE: return `{ state: "done", ref, links }` from the error
  /// branch and this fails, which is the #846 collapse the whole feature
  /// is about.
  it("reports a rejected lookup as a failure, not as an empty one", async () => {
    answer.reject = "database is locked";
    render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    const q = got();
    expect(q.state).toBe("failed");
    expect(q.state === "failed" && q.error).toBe("database is locked");
  });

  /// A bare number whose repository the tracked list can name resolves
  /// and is looked up -- against that repository, not a guess.
  it("resolves a bare number through the tracked pull request list", async () => {
    trackPrs([
      { repo: "acme/api", number: 1234 },
      { repo: "acme/ui", number: 9 },
    ]);
    answer.value = [];
    render(wrap(<Probe query="#1234" />));
    await settle();
    expect(asked).toEqual(["acme/api#1234"]);
  });

  /// **"We did not ask" is not "they did not answer" (#1050).** A bare
  /// number no tracked pull request carries cannot be looked up at all,
  /// and the state says so rather than reporting an empty result.
  ///
  /// SABOTAGE: return `{ state: "done", ref, links: [] }` when `repos`
  /// is empty and this fails -- which would have the UI say "no session
  /// recorded" about a question it never asked.
  it("says a bare number is unresolved rather than reporting an empty lookup", async () => {
    trackPrs([{ repo: "acme/ui", number: 9 }]);
    render(wrap(<Probe query="#1234" />));
    await settle();
    expect(asked).toEqual([]);
    expect(got()).toEqual({ state: "unresolved", number: 1234 });
  });

  /// Typing away from a reference and back again does not re-ask.
  ///
  /// `staleTime: Infinity` plus react-query's ordinary cache retention
  /// means the second visit is served from cache -- and the timer
  /// scheduled for the intermediate query is cleared rather than firing
  /// late against a query that has moved on. Both halves matter: a
  /// debounce that fired for `acme/api#99` after the user had already
  /// gone back would ask about a pull request nobody is looking at.
  it("does not ask again for a reference the user returns to", async () => {
    answer.value = [];
    const { rerender } = render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(asked).toEqual(["acme/api#1234"]);

    // Away, briefly -- not long enough for the intermediate reference to
    // settle -- and back.
    rerender(wrap(<Probe query="acme/api#99" />));
    act(() => {
      vi.advanceTimersByTime(50);
    });
    rerender(wrap(<Probe query="acme/api#1234" />));
    await settle();

    expect(asked).toEqual(["acme/api#1234"]);
    expect(got().state).toBe("done");
  });

  /// A query that stops being a reference goes back to `off` and takes
  /// its answer with it, rather than leaving a stale finding on screen
  /// under text that no longer names a pull request.
  it("goes back to off when the query stops naming a pull request", async () => {
    answer.value = [];
    const { rerender } = render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(got().state).toBe("done");

    rerender(wrap(<Probe query="notarization" />));
    await settle();
    expect(got()).toEqual({ state: "off" });
  });
});
