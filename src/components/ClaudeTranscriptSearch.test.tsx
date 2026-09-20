/// The empty states of transcript search (#1203, epic #1121).
///
/// This file's subject is ONE distinction: an index that has not
/// finished must never produce the sentence an index that has finished
/// produces. Every other test here exists to keep that one honest --
/// a component that always said "still indexing" would pass the first
/// test and fail the second.
///
/// The assertions are on the STRINGS, deliberately, not on which branch
/// ran. The user reads a sentence, and a test that checked the branch
/// would pass a refactor that rendered both branches identically.
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ClaudeIndexCoverage, ClaudeSearchAnswer } from "../types/pr";

const state = vi.hoisted(() => ({
  answer: undefined as ClaudeSearchAnswer | undefined,
  coverage: undefined as ClaudeIndexCoverage | undefined,
  searchFailed: false,
  searchPending: false,
}));

vi.mock("../api/hooks", () => ({
  useClaudeTranscriptSearch: () => ({
    data: state.answer,
    isPending: state.searchPending,
    isError: state.searchFailed,
    error: "database is locked",
  }),
  useClaudeIndexCoverage: () => ({
    data: state.coverage,
    isPending: false,
    isError: false,
    error: null,
  }),
}));

import { ClaudeTranscriptSearch } from "./ClaudeTranscriptSearch";

function coverage(over: Partial<ClaudeIndexCoverage> = {}): ClaudeIndexCoverage {
  return {
    indexed: 1482,
    total: 1482,
    unreadable: [],
    truncated: 0,
    last_indexed_at: "2026-09-20T00:00:00Z",
    ...over,
  };
}

/// Type a query and submit, because the component only searches what was
/// submitted -- the tests must go through the same door the user does.
function searchFor(term: string) {
  render(<ClaudeTranscriptSearch />);
  fireEvent.change(screen.getByLabelText(/search transcript content/i), {
    target: { value: term },
  });
  fireEvent.click(screen.getByRole("button", { name: /^search$/i }));
}

afterEach(() => {
  cleanup();
  state.answer = undefined;
  state.coverage = undefined;
  state.searchFailed = false;
  state.searchPending = false;
});

describe("an unfinished index never says 'no matches'", () => {
  /// THE TEST. A partial index reports its coverage WITH THE NUMBER.
  ///
  /// Both halves are asserted: the number is present, and the settled
  /// sentence is absent. Only checking for the number would pass a
  /// component that rendered both sentences at once, which is worse
  /// than either alone.
  it("reports the partial coverage with its number and does not say 'no matches' alone", () => {
    const cov = coverage({ indexed: 340, total: 1482 });
    state.coverage = cov;
    state.answer = {
      verdict: { kind: "none_yet", indexed: 340, total: 1482 },
      coverage: cov,
    };
    searchFor("fsevents");

    // The sentence the whole ticket is about, with both numbers in it.
    expect(screen.getByText(/no matches in the 340 of 1,482 sessions indexed so far/i)).toBeTruthy();
    // And how many were NOT searched, so the shortfall is a number too.
    expect(screen.getByText(/remaining 1,142 have not been indexed yet/i)).toBeTruthy();

    // The settled sentence must be absent. `All N sessions were
    // searched` is the phrase that would be the lie here.
    expect(screen.queryByText(/all 1,482 sessions were searched/i)).toBeNull();
  });

  /// The other half: a complete index says the settled thing.
  it("says 'no matches' only when the whole corpus was searched", () => {
    const cov = coverage();
    state.coverage = cov;
    state.answer = { verdict: { kind: "none" }, coverage: cov };
    searchFor("fsevents");

    expect(screen.getByText(/no matches\. all 1,482 sessions were searched/i)).toBeTruthy();
    expect(screen.queryByText(/indexed so far/i)).toBeNull();
    expect(screen.queryByText(/have not been indexed yet/i)).toBeNull();
  });

  /// The two produce DIFFERENT strings, asserted side by side.
  ///
  /// The property is not "each case renders something" but "the two
  /// render differently": a component whose branches had drifted into
  /// the same sentence would pass both tests above read separately.
  it("gives the two empty results different sentences", () => {
    const partialCov = coverage({ indexed: 340, total: 1482 });
    state.coverage = partialCov;
    state.answer = {
      verdict: { kind: "none_yet", indexed: 340, total: 1482 },
      coverage: partialCov,
    };
    searchFor("fsevents");
    const partial = screen.getByRole("status").textContent ?? "";
    cleanup();

    const wholeCov = coverage();
    state.coverage = wholeCov;
    state.answer = { verdict: { kind: "none" }, coverage: wholeCov };
    searchFor("fsevents");
    const complete = screen.getByRole("status").textContent ?? "";

    expect(partial).not.toEqual(complete);
    expect(partial).toMatch(/indexed so far/i);
    expect(complete).not.toMatch(/indexed so far/i);
  });

  /// An empty box is not a search that found nothing.
  ///
  /// Over a COMPLETE index the wrong answer here is a confident "No
  /// matches. All 1,482 sessions were searched." painted under a box
  /// nobody has typed in -- a settled answer to a question nobody asked.
  it("says nothing about matches when no query was asked", () => {
    const cov = coverage();
    state.coverage = cov;
    state.answer = { verdict: { kind: "not_asked" }, coverage: cov };
    render(<ClaudeTranscriptSearch />);

    expect(screen.queryByText(/no matches/i)).toBeNull();
    expect(screen.queryByText(/were searched/i)).toBeNull();
    // The coverage line still shows: how much is searchable is worth
    // saying before anyone types.
    expect(screen.getByText(/all 1,482 sessions are searchable/i)).toBeTruthy();
  });

  /// A search that FAILED is neither empty state. #846's exact shape.
  it("renders a failed search as an error, not as an empty result", () => {
    state.coverage = coverage();
    state.searchFailed = true;
    searchFor("fsevents");

    expect(screen.getByText(/the transcript search could not run/i)).toBeTruthy();
    expect(screen.getByText(/database is locked/)).toBeTruthy();
    expect(screen.queryByText(/no matches/i)).toBeNull();
  });
});

describe("coverage is stated before anything is typed", () => {
  /// A search box that says nothing about its own readiness invites the
  /// first empty result to be read as settled.
  it("says how much is searchable while the index is still building", () => {
    state.coverage = coverage({ indexed: 340, total: 1482 });
    render(<ClaudeTranscriptSearch />);
    expect(screen.getByText(/340 of 1,482 sessions indexed so far/i)).toBeTruthy();
  });

  it("says so when everything is searchable", () => {
    state.coverage = coverage();
    render(<ClaudeTranscriptSearch />);
    expect(screen.getByText(/all 1,482 sessions are searchable/i)).toBeTruthy();
  });

  /// No pass has run. NOT "0 of 0 searchable", which would read as a
  /// complete index of an empty corpus.
  it("says the index has not run rather than reporting 0 of 0", () => {
    state.coverage = coverage({ indexed: 0, total: 0, last_indexed_at: null });
    render(<ClaudeTranscriptSearch />);
    expect(screen.getByText(/index has not run yet/i)).toBeTruthy();
    expect(screen.queryByText(/0 of 0/)).toBeNull();
    expect(screen.queryByText(/are searchable/i)).toBeNull();
  });

  /// An unreadable transcript is a gap the user is told about.
  it("states how many transcripts could not be read", () => {
    state.coverage = coverage({
      indexed: 1481,
      total: 1482,
      unreadable: ["/Users/acme/.claude/projects/x/y.jsonl: Permission denied"],
    });
    render(<ClaudeTranscriptSearch />);
    expect(screen.getByText(/1 transcript could not be read/i)).toBeTruthy();
  });

  /// A truncated transcript is stated too: a miss in one is not proof of
  /// absence, and the 8 MB bound is a fact the reader can act on.
  it("states how many transcripts were only partly indexed", () => {
    state.coverage = coverage({ truncated: 3 });
    render(<ClaudeTranscriptSearch />);
    expect(screen.getByText(/only the first 8 mb of 3 transcripts were indexed/i)).toBeTruthy();
  });
});

describe("hits", () => {
  it("renders each match with its snippet", () => {
    const cov = coverage();
    state.coverage = cov;
    state.answer = {
      verdict: {
        kind: "matches",
        hits: [
          { session_id: "abc-123", snippet: "the [fsevents] stream died", truncated: false },
        ],
      },
      coverage: cov,
    };
    searchFor("fsevents");

    expect(screen.getByText("abc-123")).toBeTruthy();
    expect(screen.getByText(/the \[fsevents\] stream died/)).toBeTruthy();
    expect(screen.queryByText(/no matches/i)).toBeNull();
  });

  /// A hit on a truncated session says so on the row: the whole
  /// transcript was not searched, and the reader is told which one.
  it("says when a hit's transcript was only partly indexed", () => {
    const cov = coverage({ truncated: 1 });
    state.coverage = cov;
    state.answer = {
      verdict: {
        kind: "matches",
        hits: [{ session_id: "big-1", snippet: "[fsevents]", truncated: true }],
      },
      coverage: cov,
    };
    searchFor("fsevents");
    expect(screen.getByText(/only the first 8 mb of this transcript was indexed/i)).toBeTruthy();
  });
});
