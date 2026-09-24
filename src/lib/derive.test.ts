import { describe, expect, it } from "vitest";
import type { PullRequest } from "../types/pr";
import { PR_FIXTURES, prWithState } from "../fixtures/prs";
import {
  applyFilters, awaitingReview, changesRequested, deriveStacked, deriveStats,
  isStale, needsAttention, pendingReview, pendingReviewers, readyToQueue, sortPrs, STALE_DAYS,
  sortReadyForReview,
} from "./derive";

const [approved, broken, checking] = PR_FIXTURES;

describe("needsAttention", () => {
  it("flags failing CI", () => {
    expect(needsAttention(broken)).toBe(true);
  });

  it("does not flag a green PR", () => {
    expect(needsAttention(approved)).toBe(false);
  });

  /// The priorities strip must never cry wolf: a PR whose mergeability
  /// GitHub has not finished computing is not a conflict.
  it("never flags a PR whose merge state is still checking", () => {
    expect(needsAttention(checking)).toBe(false);
  });
});

describe("isStale", () => {
  it("flags a green approved PR untouched for more than 3 days", () => {
    expect(isStale(approved, new Date("2026-08-25T12:00:00Z"))).toBe(true);
  });

  it("does not flag one touched today", () => {
    expect(isStale(approved, new Date("2026-08-18T13:00:00Z"))).toBe(false);
  });

  it("does not flag a PR that is not yet approved", () => {
    expect(isStale(checking, new Date("2026-08-25T12:00:00Z"))).toBe(false);
  });

  it("honours a custom threshold instead of the STALE_DAYS default", () => {
    const now = new Date("2026-08-19T13:00:00Z"); // 1 day after approved's updated_at
    expect(isStale(approved, now, STALE_DAYS)).toBe(false);
    expect(isStale(approved, now, 1)).toBe(true);
  });
});

describe("categories", () => {
  it("classifies awaiting review, ready to queue, and blocked", () => {
    expect(readyToQueue(approved)).toBe(true);
    expect(changesRequested(broken)).toBe(true);
    expect(awaitingReview(approved)).toBe(false);
  });
});

describe("applyFilters", () => {
  it("returns everything by default", () => {
    expect(applyFilters(PR_FIXTURES, {}).length).toBe(3);
  });

  it("filters by repo", () => {
    const out = applyFilters(PR_FIXTURES, { repo: "octocat/spoon-knife" });
    expect(out.map((p) => p.number)).toEqual([7]);
  });

  it("hides drafts when readyOnly is set", () => {
    expect(applyFilters(PR_FIXTURES, { readyOnly: true }).some((p) => p.is_draft)).toBe(false);
  });

  it("includes by label", () => {
    const out = applyFilters(PR_FIXTURES, { includeLabels: ["bug"] });
    expect(out.map((p) => p.number)).toEqual([43]);
  });

  /// Excluding `dependencies` to silence dependabot is the dominant
  /// real-world case for label filtering.
  it("excludes by label", () => {
    const out = applyFilters(PR_FIXTURES, { excludeLabels: ["dependencies"] });
    expect(out.map((p) => p.number)).toEqual([42, 43]);
  });

  it("applies include and exclude together", () => {
    const out = applyFilters(PR_FIXTURES, {
      includeLabels: ["bug", "dependencies"],
      excludeLabels: ["dependencies"],
    });
    expect(out.map((p) => p.number)).toEqual([43]);
  });

  it("filters to only PRs that need attention", () => {
    // PR_FIXTURES[1] (#43) is the only one with failing CI/conflicted merge.
    const out = applyFilters(PR_FIXTURES, { needsAttentionOnly: true });
    expect(out.map((p) => p.number)).toEqual([43]);
  });

  it("filters to only PRs in the merge queue", () => {
    // PR_FIXTURES[2] (#7) is the only one with in_merge_queue: true.
    const out = applyFilters(PR_FIXTURES, { inMergeQueueOnly: true });
    expect(out.map((p) => p.number)).toEqual([7]);
  });

  /// `staleOnly` depends on wall-clock time via `isStale`. `applyFilters`
  /// must thread an explicit `now` through rather than calling `new Date()`
  /// internally, or this branch is untestable without depending on the
  /// machine clock relative to the fixtures' `updated_at` values.
  describe("staleOnly", () => {
    const stale = prWithState("success", "mergeable", "approved", {
      number: 100,
      in_merge_queue: false,
      updated_at: "2026-08-01T00:00:00Z",
    });
    const fresh = prWithState("success", "mergeable", "approved", {
      number: 101,
      in_merge_queue: false,
      updated_at: "2026-08-19T00:00:00Z",
    });
    const now = new Date("2026-08-20T00:00:00Z");

    it("keeps only PRs stale as of the given now", () => {
      const out = applyFilters([stale, fresh], { staleOnly: true }, now);
      expect(out.map((p) => p.number)).toEqual([100]);
    });

    it("excludes a PR that is not yet stale relative to now", () => {
      const out = applyFilters([fresh], { staleOnly: true }, now);
      expect(out).toEqual([]);
    });
  });
});

describe("sortReadyForReview", () => {
  // Every row OPENED at the same instant, so only `ready_at` can order
  // them: a sort still reading `created_at` returns its input untouched.
  const at = (number: number, ready_at: string | null | undefined): PullRequest => ({
    ...PR_FIXTURES[0],
    number,
    created_at: "2026-08-01T00:00:00Z",
    ready_at,
  });

  // Deliberately handed in NEWEST-first order, so a function that returns
  // its input untouched fails. Passing the list already oldest-first would
  // measure the fixture rather than the code.
  const NEWEST_FIRST = [
    at(3, "2026-09-03T00:00:00Z"),
    at(2, "2026-09-02T00:00:00Z"),
    at(1, "2026-09-01T00:00:00Z"),
  ];

  it("defaults to oldest ready first", () => {
    expect(sortReadyForReview(NEWEST_FIRST).map((pr) => pr.number)).toEqual([1, 2, 3]);
  });

  it("orders newest ready first when asked", () => {
    const oldestFirst = [...NEWEST_FIRST].reverse();
    expect(sortReadyForReview(oldestFirst, "newest-opened").map((pr) => pr.number)).toEqual(
      [3, 2, 1],
    );
  });

  it("does not mutate its input", () => {
    const original = [...NEWEST_FIRST];
    sortReadyForReview(NEWEST_FIRST);
    expect(NEWEST_FIRST).toEqual(original);
  });

  /// A `NaN` comparator result is treated as 0 by `Array.prototype.sort`,
  /// which leaves an undated row wherever it happened to be -- including
  /// the top of the queue, where it would claim to be the longest-waiting
  /// work and push genuinely old pull requests down. Last is the honest
  /// place for "we do not know".
  it("sorts an unparseable ready_at last, not first", () => {
    const withJunk = [at(9, "not a date"), ...NEWEST_FIRST];
    expect(sortReadyForReview(withJunk).map((pr) => pr.number)).toEqual([1, 2, 3, 9]);
  });

  it("sorts a missing ready_at last, not first", () => {
    const withEmpty = [at(9, ""), ...NEWEST_FIRST];
    expect(sortReadyForReview(withEmpty).map((pr) => pr.number)).toEqual([1, 2, 3, 9]);
  });

  // Last in BOTH directions. "Unknown" is not a date to be flipped -- on
  // newest-first it would otherwise land at the top for the same reason.
  it("keeps undated rows last under newest-first too", () => {
    const withJunk = [at(9, "not a date"), ...NEWEST_FIRST];
    expect(sortReadyForReview(withJunk, "newest-opened").map((pr) => pr.number)).toEqual(
      [3, 2, 1, 9],
    );
  });

  it("leaves several undated rows in a stable order among themselves", () => {
    const many = [at(8, ""), at(9, "nonsense"), ...NEWEST_FIRST];
    expect(sortReadyForReview(many).map((pr) => pr.number)).toEqual([1, 2, 3, 8, 9]);
  });

  // Unknown -- a snapshot from before the field existed, or a time GitHub
  // did not return -- is undated, and goes last. It must NOT fall back to
  // `created_at`, which would rank a week-long draft as the longest wait.
  it("sorts an absent or null ready_at last, not by when it was opened", () => {
    const unknown = [
      { ...at(8, undefined), created_at: "2020-01-01T00:00:00Z" },
      { ...at(9, null), created_at: "2020-01-01T00:00:00Z" },
      ...NEWEST_FIRST,
    ];
    expect(sortReadyForReview(unknown).map((pr) => pr.number)).toEqual([1, 2, 3, 8, 9]);
  });

  // #1407's point: a pull request opened long ago but marked ready
  // recently has waited only since then, and sorts as recent.
  it("sorts a long draft by when it became ready, not when it was opened", () => {
    const longDraft = { ...at(7, "2026-09-04T00:00:00Z"), created_at: "2026-06-01T00:00:00Z" };
    expect(sortReadyForReview([longDraft, ...NEWEST_FIRST]).map((pr) => pr.number)).toEqual([
      1, 2, 3, 7,
    ]);
  });
});

describe("sortPrs", () => {
  it("does not mutate its input", () => {
    const original = [...PR_FIXTURES];
    sortPrs(PR_FIXTURES, "oldest");
    expect(PR_FIXTURES).toEqual(original);
  });

  /// Moved from PrList.test.tsx when sorting moved out of the component.
  /// Deliberately passes the fixtures in the WRONG order: PR_FIXTURES is
  /// already newest-first, so feeding it in unchanged would pass even
  /// against a function that does nothing at all -- the assertion would be
  /// measuring the fixture, not the code.
  it("orders newest first even when handed the list reversed", () => {
    const oldestFirst = [...PR_FIXTURES].sort(
      (a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
    );
    const sorted = sortPrs(oldestFirst, "newest");
    expect(sorted.map((pr) => pr.title)).toEqual(
      [...PR_FIXTURES]
        .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
        .map((pr) => pr.title),
    );
  });

  it("defaults to newest first when no sort is given", () => {
    const oldestFirst = [...PR_FIXTURES].sort(
      (a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
    );
    expect(sortPrs(oldestFirst)).toEqual(sortPrs(oldestFirst, "newest"));
  });

  it("orders oldest first", () => {
    const sorted = sortPrs(PR_FIXTURES, "oldest");
    expect(sorted.map((pr) => pr.number)).toEqual([7, 43, 42]);
  });

  it("orders by most recently updated", () => {
    const sorted = sortPrs(PR_FIXTURES, "recently-updated");
    expect(sorted.map((pr) => pr.number)).toEqual([42, 43, 7]);
  });

  it("orders by least recently updated", () => {
    const sorted = sortPrs(PR_FIXTURES, "least-recently-updated");
    expect(sorted.map((pr) => pr.number)).toEqual([7, 43, 42]);
  });
});

describe("deriveStats", () => {
  it("counts each category from the list", () => {
    const s = deriveStats(PR_FIXTURES);
    expect(s.needs_attention).toBe(1);
    expect(s.in_merge_queue).toBe(1);
    expect(s.blocked_by_comments).toBe(1);
    expect(s.ready_to_queue).toBe(1);
  });
});

describe("free-text search", () => {
  const prs = PR_FIXTURES;

  it("matches on title, case-insensitively", () => {
    const hit = applyFilters(prs, { query: "RETRY" });
    expect(hit.length).toBeGreaterThan(0);
    expect(hit.every((p) => p.title.toLowerCase().includes("retry"))).toBe(true);
  });

  it("matches on repository", () => {
    const hit = applyFilters(prs, { query: "spoon" });
    expect(hit.every((p) => p.repo.includes("spoon"))).toBe(true);
  });

  // A person searching for a PR usually remembers its number.
  it("matches an exact PR number, with or without the hash", () => {
    expect(applyFilters(prs, { query: "42" }).map((p) => p.number)).toContain(42);
    expect(applyFilters(prs, { query: "#42" }).map((p) => p.number)).toContain(42);
  });

  it("does not partial-match numbers", () => {
    // "4" must not match #42 -- substring matching on numbers would make
    // a number search useless on a long list.
    expect(applyFilters(prs, { query: "4" }).map((p) => p.number)).not.toContain(42);
  });

  it("an empty or whitespace query filters nothing", () => {
    expect(applyFilters(prs, { query: "   " }).length).toBe(prs.length);
  });
});

/// Reported: "the left hand menu says 13 for the selected repo, Needs
/// your attention shows 4, Awaiting review shows 3 — where are the
/// missing 6?"
///
/// Two defects behind it, both fixed here. The shapes below are the
/// five that a live GraphQL probe actually returned for a real
/// account, not invented ones.
describe("the triage chips reconcile with the repo count", () => {
  const account = () => [
    ...Array(9).fill(0).map(() => prWithState("success", "mergeable", "none")),
    ...Array(3).fill(0).map(() => prWithState("success", "conflicted", "none")),
    prWithState("success", "mergeable", "none", { merge_status: "blocked" }),
    // A repository with no checks configured. This one was in NEITHER
    // chip before, because `awaitingReview` demanded `success`.
    prWithState("none", "mergeable", "none", { merge_status: "blocked" }),
    prWithState("failure", "conflicted", "none", { is_draft: true }),
  ];

  it("leaves no pull request in neither chip", () => {
    const all = account();
    const orphans = all.filter((p) => !needsAttention(p) && !awaitingReview(p));
    expect(orphans).toHaveLength(0);
  });

  /// A conflicted pull request with green CI used to satisfy BOTH: it
  /// is blocked on the author AND had nothing else disqualifying it.
  /// Counting it twice is how two chips could describe overlapping sets
  /// and reconcile with nothing.
  it("puts no pull request in both chips", () => {
    const all = account();
    const doubled = all.filter((p) => needsAttention(p) && awaitingReview(p));
    expect(doubled).toHaveLength(0);
  });

  it("sums to the total, which is the whole point", () => {
    const all = account();
    expect(all.filter(needsAttention).length + all.filter(awaitingReview).length).toBe(
      all.length,
    );
  });

  /// The specific miss: no checks configured is not "waiting on CI".
  /// `readyForReview` already treated it that way and these two must
  /// agree.
  it("counts a pull request with no CI as awaiting review", () => {
    expect(awaitingReview(prWithState("none", "mergeable", "none"))).toBe(true);
  });

  /// But a run still in progress does NOT count -- it may go red, and
  /// "awaiting review" would be the wrong thing to say about a pull
  /// request about to need the author instead.
  it("does not count a pull request whose CI is still running", () => {
    expect(awaitingReview(prWithState("pending", "mergeable", "none"))).toBe(false);
  });
});

/// "Waiting on a review" is a state; "waiting on octocat" is something
/// you can act on. `reviewDecision` cannot say the second -- it
/// collapses every reviewer into one verdict and names nobody.
describe("pendingReviewers", () => {
  /// Totals track the lists by default, so every test below describes a
  /// COMPLETE pull request unless it says otherwise. A helper that left
  /// them at 0 would make each of these rows silently truncated and the
  /// assertions would pass or fail for a reason nobody wrote down.
  const withReviewers = (
    requested: string[],
    reviews: { author: string; state: string }[] = [],
  ) => ({
    ...PR_FIXTURES[0],
    requested_reviewers: requested,
    latest_reviews: reviews,
    requested_reviewers_total: requested.length,
    latest_reviews_total: reviews.length,
  });

  it("names everyone still asked and not yet answered", () => {
    expect(pendingReviewers(withReviewers(["reviewer-one", "hubot"]))).toEqual([
      "reviewer-one",
      "hubot",
    ]);
  });

  /// The reason this is not just `requested_reviewers`: GitHub keeps a
  /// reviewer in that list after they respond in some workflows, and a
  /// re-request after a change puts an already-approved reviewer back
  /// into it. Showing them would tell the user to chase someone who
  /// already answered.
  it("drops a reviewer who has already given a verdict", () => {
    const pr = withReviewers(
      ["reviewer-one", "hubot"],
      [{ author: "reviewer-one", state: "APPROVED" }],
    );
    expect(pendingReviewers(pr)).toEqual(["hubot"]);
  });

  /// A COMMENTED review is an answer. They looked and said something
  /// without blocking, so the row should not suggest chasing them.
  it("treats a comment as an answer", () => {
    const pr = withReviewers(["reviewer-one"], [{ author: "reviewer-one", state: "COMMENTED" }]);
    expect(pendingReviewers(pr)).toEqual([]);
  });

  it("ignores reviews from people who were never requested", () => {
    const pr = withReviewers(["reviewer-one"], [{ author: "a-passerby", state: "APPROVED" }]);
    expect(pendingReviewers(pr)).toEqual(["reviewer-one"]);
  });

  /// The investigation that prompted this: `reviewRequests` is empty on
  /// 25 of 25 rust-lang/rust pull requests, because it assigns the
  /// reviewer instead. Without the fallback the feature shows nothing
  /// at all on whole repositories.
  it("falls back to assignees when no reviewer was requested", () => {
    const pr = { ...withReviewers([]), assignees: ["jieyouxu"] };
    expect(pendingReviewers(pr)).toEqual(["jieyouxu"]);
  });

  /// A FALLBACK, not an addition. On a repo that uses both, an assignee
  /// is often the author triaging their own pull request.
  it("prefers requested reviewers over assignees when both exist", () => {
    const pr = { ...withReviewers(["reviewer-one"]), assignees: ["someone-else"] };
    expect(pendingReviewers(pr)).toEqual(["reviewer-one"]);
  });

  /// The author assigning themselves is the common case on repos that
  /// use assignees for triage. "Waiting on yourself" is never useful.
  it("never lists the author as someone you are waiting on", () => {
    const base = withReviewers([]);
    const pr = { ...base, assignees: [base.author] };
    expect(pendingReviewers(pr)).toEqual([]);
  });

  it("still drops an assignee who has already reviewed", () => {
    const pr = {
      ...withReviewers([], [{ author: "jieyouxu", state: "APPROVED" }]),
      assignees: ["jieyouxu"],
    };
    expect(pendingReviewers(pr)).toEqual([]);
  });

  /// Empty is ORDINARY, not missing data -- measured at 0 of 25 on
  /// rust-lang/rust, which assigns reviewers through a bot. This must
  /// not throw or invent anything.
  it("returns nothing when no reviewer was requested", () => {
    expect(pendingReviewers(withReviewers([]))).toEqual([]);
  });

  /// A snapshot written before these fields existed deserialises
  /// without them, so the optional chaining is load-bearing rather than
  /// defensive padding.
  it("survives a pull request cached before these fields existed", () => {
    const old = { ...PR_FIXTURES[0] } as Record<string, unknown>;
    delete old.requested_reviewers;
    delete old.latest_reviews;
    expect(() => pendingReviewers(old as never)).not.toThrow();
    expect(pendingReviewers(old as never)).toEqual([]);
  });
});

/// #1089: the verdict list is PAGED, so the answered set can be short --
/// and every name subtracted against a short set is possibly wrong.
///
/// This is not an undercount like the others on the row. It names a
/// person and tells the reader to chase them, when that person has
/// already approved. The repo's rule is "qualify, or suppress":
/// possibly-wrong suppresses.
describe("pendingReview with a truncated verdict list", () => {
  /// The exact shape the issue describes: six reviewers were asked, the
  /// query's window returned five verdicts, and the SIXTH -- the one
  /// outside the window -- has approved.
  ///
  /// Every reviewer here has in fact answered, so the truthful roster is
  /// empty. The old code could only see five of the six verdicts, so
  /// `reviewer-six` survived the subtraction and was named as pending.
  const sixth = () => {
    const asked = [
      "reviewer-one",
      "reviewer-two",
      "reviewer-three",
      "reviewer-four",
      "reviewer-five",
      "reviewer-six",
    ];
    return {
      ...PR_FIXTURES[0],
      requested_reviewers: asked,
      requested_reviewers_total: asked.length,
      // What a `first: 5` window returns: the first five verdicts.
      latest_reviews: asked.slice(0, 5).map((author) => ({ author, state: "APPROVED" })),
      // What GitHub SAYS exists. `reviewer-six`'s approval is in the gap
      // between this number and the list above.
      latest_reviews_total: 6,
    };
  };

  /// THE test this issue exists for. A person who has approved must not
  /// be reported as someone you are waiting on.
  it("does not name a reviewer who approved outside the window as pending", () => {
    const { names, certain, atLeast } = pendingReview(sixth());
    expect(names).not.toContain("reviewer-six");
    // Pinned exactly, not just "does not contain": an empty array
    // satisfies `not.toContain` whatever the reason, so the assertion
    // above would pass on a function that returned nothing at all. This
    // says the suppression is total and deliberate.
    expect(names).toEqual([]);
    expect(certain).toBe(false);
    // And the row is not silenced: a review IS outstanding as far as
    // this data can tell, so the reader is still told to look -- just
    // not told WHO, which is the part that could be wrong.
    expect(atLeast).toBeGreaterThan(0);
  });

  /// `pendingReviewers`, the older list-only entry point, must get the
  /// same answer. Fixing this in the renderer alone would leave every
  /// other caller -- and the next one somebody writes -- with the wrong
  /// name, which is why the suppression lives in the data.
  it("suppresses the name through the list-only entry point too", () => {
    expect(pendingReviewers(sixth())).toEqual([]);
  });

  /// `certain: false` is what the row reads to suppress the names. If it
  /// were true, the caller would print `reviewer-six` in full confidence
  /// -- which is the defect, not the fix.
  it("reports the roster as unsettled rather than printing a name that may be wrong", () => {
    expect(pendingReview(sixth()).certain).toBe(false);
  });

  /// The counterpart, and what stops this from being a warning on every
  /// row: when the window covered every verdict, the names are exact and
  /// the caller prints them as before.
  it("stays certain when every verdict arrived", () => {
    const pr = {
      ...PR_FIXTURES[0],
      requested_reviewers: ["reviewer-one", "hubot"],
      requested_reviewers_total: 2,
      latest_reviews: [{ author: "reviewer-one", state: "APPROVED" }],
      latest_reviews_total: 1,
    };
    const { names, certain } = pendingReview(pr);
    expect(certain).toBe(true);
    expect(names).toEqual(["hubot"]);
  });

  /// Truncation only matters when a name survived. With nobody left to
  /// chase there is nothing that could be wrong, and warning about an
  /// empty list would put a caveat on every quiet row.
  it("stays certain when the subtraction left nobody, however truncated", () => {
    const pr = {
      ...PR_FIXTURES[0],
      requested_reviewers: ["reviewer-one"],
      requested_reviewers_total: 1,
      latest_reviews: [{ author: "reviewer-one", state: "APPROVED" }],
      latest_reviews_total: 40,
    };
    const { names, certain } = pendingReview(pr);
    expect(names).toEqual([]);
    expect(certain).toBe(true);
  });

  /// A snapshot cached before these totals existed must behave exactly
  /// as it did before them: names printed, nothing suppressed, no
  /// caveat. The upgrade path is the whole reason the fields are
  /// optional.
  ///
  /// Scoped honestly. The obvious framing is "absent is not zero", but
  /// sabotage says otherwise: swapping the `?? fetched` fallback for
  /// `?? 0` fails NOTHING, because `0 > fetched` is false anyway and
  /// `unseen` is clamped at 0. So this does not claim to guard that
  /// fallback -- it guards the BEHAVIOUR an upgrading user sees, which
  /// is what would actually be noticed if it broke.
  it("does not call a pull request cached before these totals truncated", () => {
    const old = {
      ...PR_FIXTURES[0],
      requested_reviewers: ["hubot"],
      latest_reviews: [{ author: "reviewer-one", state: "APPROVED" }],
    } as Record<string, unknown>;
    delete old.requested_reviewers_total;
    delete old.latest_reviews_total;
    const { names, certain } = pendingReview(old as never);
    expect(certain).toBe(true);
    expect(names).toEqual(["hubot"]);
  });

  /// The `+N` was a precise-looking number computed from a cut list:
  /// `requested_reviewers` is paged too, so `names.length` is a FLOOR.
  it("reports the outstanding count as a floor when the asked list was cut", () => {
    const pr = {
      ...PR_FIXTURES[0],
      // Seven were asked; the window returned five.
      requested_reviewers: ["a", "b", "c", "d", "e"],
      requested_reviewers_total: 7,
      latest_reviews: [],
      latest_reviews_total: 0,
    };
    const { names, atLeast, exact } = pendingReview(pr);
    expect(names).toHaveLength(5);
    expect(exact).toBe(false);
    // Five named plus two unseen, not the five the old row would divide.
    expect(atLeast).toBe(7);
  });

  it("reports an exact count when the asked list arrived whole", () => {
    const pr = {
      ...PR_FIXTURES[0],
      requested_reviewers: ["a", "b", "c"],
      requested_reviewers_total: 3,
      latest_reviews: [],
      latest_reviews_total: 0,
    };
    const { atLeast, exact } = pendingReview(pr);
    expect(exact).toBe(true);
    expect(atLeast).toBe(3);
  });

  /// The assignee fallback is paged too, and its total is a different
  /// field -- so a floor computed from the wrong one would be silently
  /// wrong on exactly the repositories the fallback exists for.
  it("counts the assignee fallback against the assignee total", () => {
    const pr = {
      ...PR_FIXTURES[0],
      requested_reviewers: [],
      requested_reviewers_total: 0,
      assignees: ["jieyouxu", "wesleywiser"],
      assignees_total: 6,
      latest_reviews: [],
      latest_reviews_total: 0,
    };
    const { names, atLeast, exact } = pendingReview(pr);
    expect(names).toEqual(["jieyouxu", "wesleywiser"]);
    expect(exact).toBe(false);
    expect(atLeast).toBe(6);
  });

  /// A total smaller than the list is LEGITIMATE, not a negative
  /// shortfall: the count and the nodes can come from slightly different
  /// moments. `ReviewThreads` takes the same care.
  it("does not invent a shortfall when the total lags the list", () => {
    const pr = {
      ...PR_FIXTURES[0],
      requested_reviewers: ["a", "b"],
      requested_reviewers_total: 1,
      latest_reviews: [],
      latest_reviews_total: 0,
    };
    const { atLeast, exact } = pendingReview(pr);
    expect(exact).toBe(true);
    expect(atLeast).toBe(2);
  });
});

/// #743: a stacked PR looked exactly like a standalone one, so users hit
/// "add to merge queue" on PRs GitHub then refused -- a stack has to be
/// enqueued through the asynchronous REST API instead.
describe("deriveStacked", () => {
  const base = prWithState("success", "mergeable", "none", {
    id: "PR_base",
    number: 10,
    head_ref: "stack/part-1",
    base_ref: "main",
  });
  const child = prWithState("success", "mergeable", "none", {
    id: "PR_child",
    number: 11,
    head_ref: "stack/part-2",
    base_ref: "stack/part-1",
  });

  it("names the PR a stacked one sits on", () => {
    expect(deriveStacked([base, child]).get(child.id)).toBe(10);
  });

  it("leaves the bottom of the stack unmarked", () => {
    expect(deriveStacked([base, child]).has(base.id)).toBe(false);
  });

  /// The whole point of the structural rule. Graphite, `spr`, `gh stack`
  /// and a hand-made stack all name their branches differently and all
  /// produce the same head-to-base shape, so the detection keys on the
  /// shape. Nothing here follows any tool's convention.
  it("recognises a stack whose branches follow no known naming scheme", () => {
    const bottom = prWithState("success", "mergeable", "none", {
      id: "PR_a",
      number: 20,
      head_ref: "refactor-the-parser",
      base_ref: "trunk",
    });
    const top = prWithState("success", "mergeable", "none", {
      id: "PR_b",
      number: 21,
      head_ref: "use-the-new-parser",
      base_ref: "refactor-the-parser",
    });
    expect(deriveStacked([bottom, top]).get(top.id)).toBe(20);
  });

  /// The failure mode of guessing from the base name alone, which is
  /// what the row used to do (`base_ref !== "main" && !== "master"`).
  /// A release train, a `develop` integration branch, or simply a repo
  /// whose default branch has a third name are all NOT stacks, and
  /// calling them stacked would put a marker on rows where merging is
  /// blocked on nothing.
  it("does not call a PR stacked merely because its base is not main", () => {
    const onRelease = prWithState("success", "mergeable", "none", {
      id: "PR_rel",
      number: 30,
      head_ref: "fix/hotfix",
      base_ref: "release/2026-09",
    });
    expect(deriveStacked([onRelease]).size).toBe(0);
  });

  /// Branch names are unique only within a repository, so two repos each
  /// with a `develop` would otherwise mark each other's PRs.
  it("never resolves a parent across repositories", () => {
    const other = prWithState("success", "mergeable", "none", {
      id: "PR_other",
      number: 40,
      repo: "octocat/spoon-knife",
      head_ref: "stack/part-1",
      base_ref: "main",
    });
    expect(deriveStacked([other, child]).size).toBe(0);
  });

  /// A three-deep stack: every PR above the bottom points at the one
  /// directly beneath it, not all of them at the root.
  it("resolves each level of a deeper stack to its immediate parent", () => {
    const third = prWithState("success", "mergeable", "none", {
      id: "PR_third",
      number: 12,
      head_ref: "stack/part-3",
      base_ref: "stack/part-2",
    });
    const out = deriveStacked([base, child, third]);
    expect(out.get(child.id)).toBe(10);
    expect(out.get(third.id)).toBe(11);
  });

  /// A parent that has merged is gone from the open list, so nothing can
  /// name it. Silence is the right answer -- a marker that cannot say
  /// which PR to go merge is worse than none.
  it("stays silent when the parent is not in the list", () => {
    expect(deriveStacked([child]).size).toBe(0);
  });

  /// The mapper defaults absent refs to "" (see
  /// `missing_branch_refs_map_to_empty_strings` in map.rs), and every
  /// such PR would otherwise match every other such PR.
  it("does not pair up PRs whose refs came back empty", () => {
    const a = prWithState("success", "mergeable", "none", {
      id: "PR_empty_a",
      number: 50,
      head_ref: "",
      base_ref: "",
    });
    const b = prWithState("success", "mergeable", "none", {
      id: "PR_empty_b",
      number: 51,
      head_ref: "",
      base_ref: "",
    });
    expect(deriveStacked([a, b]).size).toBe(0);
  });
});
