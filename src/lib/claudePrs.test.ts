import { describe, expect, it } from "vitest";
import type { ClaudePrLink } from "@/types/pr";
import { groupPrsByRepo, parsePrQuery, reposForNumber } from "./claudePrs";

const link = (repo: string, number: number): ClaudePrLink => ({
  session_id: `s-${repo}-${number}`,
  repo,
  number,
  url: `https://github.com/${repo}/pull/${number}`,
  first_seen_at: null,
});

describe("groupPrsByRepo", () => {
  /// The defect the grouping exists for: one session's pull requests
  /// arrived interleaved across repositories in whatever order the link
  /// table yielded, so finding one was a scan.
  it("puts one repository's pull requests together", () => {
    const groups = groupPrsByRepo([
      link("acme/api", 12),
      link("acme/ui", 3),
      link("acme/api", 4),
      link("acme/ui", 9),
      link("acme/api", 30),
    ]);

    expect(groups.map((g) => g.repo)).toEqual(["acme/api", "acme/ui"]);
    expect(groups[0].prs.map((p) => p.number)).toEqual([4, 12, 30]);
    expect(groups[1].prs.map((p) => p.number)).toEqual([3, 9]);
  });

  /// ASCENDING, which is the opposite of this app's default ordering
  /// everywhere else. Within one session the numbers were issued in
  /// roughly the order the work happened, so ascending reads as the
  /// sequence of that work; descending would present it back to front
  /// against the transcript beside it.
  ///
  /// `30` before `4` is the numeric-vs-lexical trap, and it is here on
  /// purpose: a comparator sorting these as strings passes a test using
  /// single digits and fails on every real session.
  it("sorts numbers ascending, numerically and not lexically", () => {
    const groups = groupPrsByRepo([
      link("acme/api", 30),
      link("acme/api", 4),
      link("acme/api", 100),
      link("acme/api", 9),
    ]);
    expect(groups[0].prs.map((p) => p.number)).toEqual([4, 9, 30, 100]);
  });

  /// The group order may not be left to the order the backend happened
  /// to send the links in. Two inputs holding the same links in
  /// different orders must produce the same groups in the same order --
  /// which is what an object keyed by repo, iterated in insertion order,
  /// would NOT give.
  it("orders the groups deterministically, whatever order the links arrive in", () => {
    const links = [link("acme/zulu", 2), link("acme/alpha", 8), link("acme/mike", 5), link("acme/alpha", 1)];
    const forward = groupPrsByRepo(links);
    const reversed = groupPrsByRepo([...links].reverse());

    expect(forward.map((g) => g.repo)).toEqual(["acme/alpha", "acme/mike", "acme/zulu"]);
    expect(reversed.map((g) => g.repo)).toEqual(forward.map((g) => g.repo));
    expect(reversed.map((g) => g.prs.map((p) => p.number))).toEqual(
      forward.map((g) => g.prs.map((p) => p.number)),
    );
  });

  /// Repository names that differ only in case still have one settled
  /// order, rather than falling back to the Map's insertion order the
  /// alphabetical sort exists to stop depending on.
  it("settles repositories differing only in case", () => {
    const a = groupPrsByRepo([link("acme/API", 1), link("acme/api", 2)]);
    const b = groupPrsByRepo([link("acme/api", 2), link("acme/API", 1)]);
    expect(a.map((g) => g.repo)).toEqual(b.map((g) => g.repo));
  });

  /// Same number in two repositories is two different pull requests, so
  /// neither group may absorb the other's row.
  it("keeps the same number in two repositories apart", () => {
    const groups = groupPrsByRepo([link("acme/api", 7), link("acme/ui", 7)]);
    expect(groups).toHaveLength(2);
    expect(groups[0].prs).toHaveLength(1);
    expect(groups[1].prs).toHaveLength(1);
  });

  it("returns nothing for no pull requests", () => {
    expect(groupPrsByRepo([])).toEqual([]);
  });

  /// The input is not mutated: it is a React prop, and sorting it in
  /// place would reorder the array the detail query holds in cache.
  it("does not reorder its input", () => {
    const links = [link("acme/api", 30), link("acme/api", 4)];
    groupPrsByRepo(links);
    expect(links.map((p) => p.number)).toEqual([30, 4]);
  });
});

describe("parsePrQuery", () => {
  /// The four shapes that reach the backend. `#1234` and `1234` name no
  /// repository, which is what `repo: null` says and what
  /// `reposForNumber` below is for.
  it.each([
    ["#1234", null, 1234],
    ["1234", null, 1234],
    ["  #1234  ", null, 1234],
    ["acme/api#7", "acme/api", 7],
    ["acme/my_repo-2.x#31", "acme/my_repo-2.x", 31],
    // A two-segment relative path is indistinguishable from a
    // repository reference without a list of the user's repositories,
    // so it parses -- and costs one lookup that returns empty while
    // text search is untouched.
    ["owner/notes#5", "owner/notes", 5],
    ["https://github.com/acme/api/pull/42", "acme/api", 42],
    ["http://github.com/acme/api/pull/42", "acme/api", 42],
    ["https://www.github.com/acme/api/pull/42", "acme/api", 42],
    ["https://github.com/acme/api/pull/42/files", "acme/api", 42],
    ["https://github.com/acme/api/pull/42#issuecomment-1", "acme/api", 42],
  ])("reads %s as a pull request reference", (q, repo, number) => {
    expect(parsePrQuery(q)).toEqual({ repo, number });
  });

  /// The constraint the issue states outright: plain-text search must
  /// not be broken, and an ordinary query must not pay for a backend
  /// call. Everything here stays prose and never reaches the lookup.
  it.each([
    "",
    "   ",
    "notarization",
    // A path deeper than two segments is not a repository.
    "owner/widgets/Foo.tsx#L12",
    "notarization/step two#3",
    "fix 1234",
    "v1234",
    "1234x",
    "#",
    "#abc",
    "# 1234",
    "/1234",
    "12.34",
    "-5",
    "#0",
    "0",
    "acme/api#0",
    "12345678",
    "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
    "https://github.com/acme/api/issues/42",
    "https://example.com/acme/api/pull/42",
  ])("does not read %j as a pull request reference", (q) => {
    expect(parsePrQuery(q)).toBeNull();
  });
});

describe("reposForNumber", () => {
  const prs = [
    { repo: "acme/ui", number: 7 },
    { repo: "acme/api", number: 7 },
    { repo: "acme/api", number: 8 },
    { repo: "acme/api", number: 7 },
  ];

  /// Every repository carrying that number, deduplicated and in one
  /// settled order -- picking one would be a guess about which pull
  /// request the user meant.
  it("names every repository holding that number", () => {
    expect(reposForNumber(prs, 7)).toEqual(["acme/api", "acme/ui"]);
  });

  it("names nothing for a number no tracked pull request has", () => {
    expect(reposForNumber(prs, 99)).toEqual([]);
  });

  /// `undefined` is the pull request list not having loaded, which is a
  /// different thing from it holding no match -- and the caller renders
  /// both as "could not tell which repository", never as "no session
  /// recorded". Guarded here so a `.filter` on undefined cannot throw.
  it("names nothing when the pull request list has not loaded", () => {
    expect(reposForNumber(undefined, 7)).toEqual([]);
  });
});
