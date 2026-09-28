import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { ClaudePrLink } from "@/types/pr";

/// Every number the backend was asked about, as `#N`.
///
/// The POINT of the feature's cost story: ordinary prose must not reach
/// this, and typing a reference must not reach it once per character.
const asked = vi.hoisted(() => [] as string[]);
/// The link table the fake backend answers from, and an optional
/// rejection standing in for a database that could not be read.
const answer = vi.hoisted(
  () =>
    ({
      table: [] as unknown[],
      reject: null as string | null,
    }) as { table: unknown[]; reject: string | null },
);

vi.mock("./tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  // The lookup by number alone (#1545): every repository's links for it,
  // exactly as `store::sessions_for_pr_number` answers.
  claudeSessionsForPrNumber: (number: number) => {
    asked.push(`#${number}`);
    return answer.reject !== null
      ? Promise.reject(new Error(answer.reject))
      : Promise.resolve(answer.table.filter((l) => (l as ClaudePrLink).number === number));
  },
  getPullRequests: () => Promise.resolve([]),
}));

import {
  useClaudeSessionsForPr,
  useClaudeSessionsForPrQuery,
  type PrQueryState,
  type PrSessionsState,
} from "./hooks";

/// The hook's answer, rendered as JSON so an assertion can read the
/// whole state value rather than a flag derived from it.
function Probe({ query }: { query: string }) {
  const q: PrQueryState = useClaudeSessionsForPrQuery(query, true);
  return <output data-testid="out">{JSON.stringify(q)}</output>;
}

let qc: QueryClient;

function wrap(node: ReactNode) {
  return <QueryClientProvider client={qc}>{node}</QueryClientProvider>;
}

const got = (): PrQueryState => JSON.parse(screen.getByTestId("out").textContent ?? "null");

const link = (session_id: string, repo: string, number: number): ClaudePrLink => ({
  session_id,
  repo,
  number,
  url: `https://github.com/${repo}/pull/${number}`,
  first_seen_at: "2026-09-11T12:00:00Z",
});

beforeEach(() => {
  vi.useFakeTimers();
  asked.length = 0;
  answer.table = [];
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
  /// `asked` fills with `#0` entries and this fails.
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
    expect(asked).toEqual(["#1234"]);
  });

  /// A lookup that ran and found nothing. `done` with an empty `links`
  /// is a FINDING, and the caller words it as one.
  it("reports an empty lookup as a completed lookup", async () => {
    render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(got()).toEqual({ state: "done", ref: "acme/api#1234", links: [], elsewhere: [] });
  });

  /// A lookup that REJECTED. Never `done`, and it carries the reason.
  ///
  /// SABOTAGE: return `done` from the error branch and this fails, which
  /// is the #846 collapse the whole feature is about.
  it("reports a rejected lookup as a failure, not as an empty one", async () => {
    answer.reject = "database is locked";
    render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    const q = got();
    expect(q.state).toBe("failed");
    expect(q.state === "failed" && q.error).toBe("database is locked");
  });

  /// **#1545: every form finds its session, open or merged.**
  ///
  /// `acme/api#7` is OPEN -- it is in the tracked pull request cache the
  /// old resolution read. `acme/api#1081` is MERGED and in no cache, the
  /// usual state of "the PR that session made" by the time anyone looks.
  /// Before #1545 the bare forms of the merged one were `unresolved` and
  /// asked nothing.
  ///
  /// SABOTAGE: resolve a bare number through `["prs"]` again (return
  /// `off` when the cache lacks it) and the merged PR's `#1081` and
  /// `1081` cases fail, while the open PR's still pass -- which is why
  /// the bug read as intermittent.
  describe.each([
    { pr: "open", number: 7 },
    { pr: "merged", number: 1081 },
  ])("a session's $pr pull request", ({ number }) => {
    it.each([
      `https://github.com/acme/api/pull/${number}`,
      `acme/api#${number}`,
      `#${number}`,
      `${number}`,
    ])("is found by %s", async (query) => {
      qc.setQueryData(["prs"], [{ repo: "acme/api", number: 7 }]);
      answer.table = [link("s-open", "acme/api", 7), link("s-merged", "acme/api", 1081)];
      render(wrap(<Probe query={query} />));
      await settle();
      const q = got();
      expect(q.state).toBe("done");
      expect(q.state === "done" && q.links.map((l) => `${l.session_id} ${l.repo}#${l.number}`)).toEqual([
        `${number === 7 ? "s-open" : "s-merged"} acme/api#${number}`,
      ]);
    });
  });

  /// A bare number is every repository's: two repos can both hold a
  /// `#7`, and choosing one would be a guess. A qualified one is only
  /// its own repository's.
  it("answers a bare number from every repository and a qualified one from its own", async () => {
    answer.table = [link("s1", "acme/api", 7), link("s2", "acme/ui", 7)];
    const { rerender } = render(wrap(<Probe query="#7" />));
    await settle();
    const bare = got();
    expect(bare.state === "done" && bare.links.map((l) => l.repo)).toEqual(["acme/api", "acme/ui"]);

    rerender(wrap(<Probe query="acme/ui#7" />));
    await settle();
    const own = got();
    expect(own.state === "done" && own.links.map((l) => l.session_id)).toEqual(["s2"]);
  });

  /// GitHub compares `owner/repo` case-insensitively, and a pasted or
  /// typed slug need not match the case the link recorded.
  ///
  /// SABOTAGE: compare `l.repo === query.repo` in `matchPrLinks` and this
  /// fails.
  it("matches a qualified repository regardless of case", async () => {
    answer.table = [link("s1", "Acme/API", 7)];
    render(wrap(<Probe query="acme/api#7" />));
    await settle();
    const q = got();
    expect(q.state === "done" && q.links.map((l) => l.session_id)).toEqual(["s1"]);
  });

  /// A transferred repository: the link was written under the old owner,
  /// and the reader searched the new one. NOT a match -- that would be a
  /// guess -- but carried as `elsewhere` for the note to state.
  it("carries the same repository under another owner as elsewhere, not as a match", async () => {
    answer.table = [link("s1", "old-owner/api", 7), link("s2", "acme/ui", 7)];
    render(wrap(<Probe query="acme/api#7" />));
    await settle();
    const q = got();
    expect(q.state === "done" && q.links).toEqual([]);
    expect(q.state === "done" && q.elsewhere.map((l) => l.repo)).toEqual(["old-owner/api"]);
  });

  /// Typing away from a reference and back again does not re-ask.
  ///
  /// A minute's `staleTime` (#1557) plus react-query's ordinary cache
  /// retention means the second visit is served from cache -- and the timer
  /// scheduled for the intermediate query is cleared rather than firing
  /// late against a query that has moved on. Both halves matter: a
  /// debounce that fired for `acme/api#99` after the user had already
  /// gone back would ask about a pull request nobody is looking at.
  it("does not ask again for a reference the user returns to", async () => {
    const { rerender } = render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(asked).toEqual(["#1234"]);

    // Away, briefly -- not long enough for the intermediate reference to
    // settle -- and back.
    rerender(wrap(<Probe query="acme/api#99" />));
    act(() => {
      vi.advanceTimersByTime(50);
    });
    rerender(wrap(<Probe query="acme/api#1234" />));
    await settle();

    expect(asked).toEqual(["#1234"]);
    expect(got().state).toBe("done");
  });

  /// A query that stops being a reference goes back to `off` and takes
  /// its answer with it, rather than leaving a stale finding on screen
  /// under text that no longer names a pull request.
  it("goes back to off when the query stops naming a pull request", async () => {
    const { rerender } = render(wrap(<Probe query="acme/api#1234" />));
    await settle();
    expect(got().state).toBe("done");

    rerender(wrap(<Probe query="notarization" />));
    await settle();
    expect(got()).toEqual({ state: "off" });
  });
});

/// The PR detail panel's lookup (#1557): the same number lookup and the
/// same `matchPrLinks`, so a transferred repository's older links are
/// found and stated rather than missed.
function PanelProbe({ repo, number }: { repo: string; number: number }) {
  const q: PrSessionsState = useClaudeSessionsForPr(repo, number, true);
  return <output data-testid="panel">{JSON.stringify(q)}</output>;
}

const panel = (): PrSessionsState => JSON.parse(screen.getByTestId("panel").textContent ?? "null");

describe("useClaudeSessionsForPr", () => {
  /// SABOTAGE: match the exact repository only -- or drop `elsewhere` --
  /// and the old-owner row is lost.
  it("returns the same repository under another owner as elsewhere", async () => {
    answer.table = [link("s1", "old-owner/api", 7), link("s2", "acme/ui", 7)];
    render(wrap(<PanelProbe repo="acme/api" number={7} />));
    await settle();
    const q = panel();
    expect(q.state === "done" && q.links).toEqual([]);
    expect(q.state === "done" && q.elsewhere.map((l) => l.session_id)).toEqual(["s1"]);
  });

  it("reports a rejected lookup as a failure, not as no session", async () => {
    answer.reject = "database is locked";
    render(wrap(<PanelProbe repo="acme/api" number={7} />));
    await settle();
    expect(panel()).toEqual({ state: "failed", error: "database is locked" });
  });

  /// One cached answer per number: whichever of the search and the panel
  /// asks first warms the other.
  it("shares the search's cached answer for the same number", async () => {
    answer.table = [link("s1", "acme/api", 7)];
    render(
      wrap(
        <>
          <Probe query="#7" />
          <PanelProbe repo="acme/api" number={7} />
        </>,
      ),
    );
    await settle();
    expect(asked).toEqual(["#7"]);
    const q = panel();
    expect(q.state === "done" && q.links.map((l) => l.session_id)).toEqual(["s1"]);
  });

  /// A PR opened since the last read: the live pass adds its link within
  /// a minute, and the panel re-asks on that cadence while it has no
  /// session for its PR -- then stops once it has one.
  ///
  /// SABOTAGE: `refetchInterval: false` and the late link is never seen.
  it("re-asks while it has no session, and stops once it finds one", async () => {
    render(wrap(<PanelProbe repo="acme/api" number={7} />));
    await settle();
    expect(panel()).toEqual({ state: "done", links: [], elsewhere: [] });

    answer.table = [link("s1", "acme/api", 7)];
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    await settle();
    const q = panel();
    expect(q.state === "done" && q.links.map((l) => l.session_id)).toEqual(["s1"]);

    const before = asked.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(180_000);
    });
    expect(asked.length).toBe(before);
  });
});
