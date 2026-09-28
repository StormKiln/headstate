import { describe, expect, it } from "vitest";
import type { PrComment } from "@/types/pr";
import { commentKind, foldSuperseded, normaliseSignature, supersededLabel } from "./supersededComments";

/// #1581. Generic accounts throughout: `coverage-bot` and `review-bot`
/// are bots, `alice` and `bob` are people.
const at = (n: number) => `2026-01-${String(n).padStart(2, "0")}T00:00:00Z`;
const bot = (author: string, day: number, body: string): PrComment => ({
  author,
  created_at: at(day),
  body,
  author_is_bot: true,
});
const human = (author: string, day: number, body: string): PrComment => ({
  author,
  created_at: at(day),
  body,
  author_is_bot: false,
});
const coverage = (day: number, pct: string, sha: string) =>
  bot(
    "coverage-bot",
    day,
    `## Coverage report: ${pct}% (+0.${day}%) at ${sha}\n\n| file | cov |\n|---|---|\n| a.ts | ${day}0% |\n\n[Details](https://ci.example.com/acme/widget/runs/${day}000)`,
  );

describe("foldSuperseded", () => {
  /// The case the issue is about: one bot's report each round, its
  /// numbers, hash and link different every time.
  it("folds bot repeats with changing numbers into one group, newest shown", () => {
    const list = [coverage(1, "80.1", "a1b2c3d4"), coverage(2, "80.9", "e5f6a7b8"), coverage(3, "81.4", "0a9b8c7d")];
    const out = foldSuperseded(list);
    expect(out).toHaveLength(1);
    expect(out[0].comment).toBe(list[2]);
    // Oldest first, as they happened.
    expect(out[0].superseded).toEqual([list[0], list[1]]);
  });

  /// Two kinds from one app: an AI review and its walkthrough are
  /// different comments that both repeat. Folding them together would
  /// hide the newest of one kind under the other.
  it("keeps two kinds of comment from the same bot apart", () => {
    const list = [
      bot("review-bot", 1, "<!-- review-summary -->\n## Summary\nLooks fine."),
      bot("review-bot", 2, "<!-- review-walkthrough -->\n## Walkthrough\nThree files."),
      bot("review-bot", 3, "<!-- review-summary -->\n## Summary\nTwo nits."),
      bot("review-bot", 4, "<!-- review-walkthrough -->\n## Walkthrough\nFour files."),
    ];
    const out = foldSuperseded(list);
    expect(out.map((e) => e.comment)).toEqual([list[2], list[3]]);
    expect(out[0].superseded).toEqual([list[0]]);
    expect(out[1].superseded).toEqual([list[1]]);
  });

  /// Different headings with no marker are different kinds, too.
  it("keeps a bot's differently headed comments apart when it embeds no marker", () => {
    const list = [
      bot("ci-bot", 1, "## Preview deployed\nhttps://preview.example.com/1"),
      bot("ci-bot", 2, "## Build failed\nStep 3 of 7"),
    ];
    expect(foldSuperseded(list).map((e) => e.superseded.length)).toEqual([0, 0]);
  });

  /// The same heading from two accounts is never one kind.
  it("never folds across accounts", () => {
    const list = [bot("coverage-bot", 1, "## Coverage report\n80%"), bot("other-bot", 2, "## Coverage report\n81%")];
    expect(foldSuperseded(list).map((e) => e.superseded.length)).toEqual([0, 0]);
  });

  /// A person repeating themselves is still two decisions.
  it("never folds a human's comments, even identical ones", () => {
    const list = [human("alice", 1, "LGTM"), human("alice", 2, "LGTM"), human("alice", 3, "## Review\n1 issue"), human("alice", 4, "## Review\n2 issues")];
    const out = foldSuperseded(list);
    expect(out).toHaveLength(4);
    expect(out.every((e) => e.superseded.length === 0)).toBe(true);
  });

  /// ...unless the same account carries the same hidden marker, exactly:
  /// a tool posting under a person's token.
  it("folds a human account's comments only on an exactly matching marker", () => {
    const list = [
      human("alice", 1, "<!-- lint-report 1 -->\n3 warnings"),
      human("alice", 2, "<!-- lint-report 2 -->\n2 warnings"),
      human("alice", 3, "<!-- lint-report 1 -->\n1 warning"),
    ];
    const out = foldSuperseded(list);
    // Days 1 and 3 match exactly. Day 2's marker differs only in a
    // number -- which a BOT's marker would have masked -- and a person's
    // marker is compared unmasked, so it stays apart.
    expect(out.map((e) => e.comment)).toEqual([list[1], list[2]]);
    expect(out[1].superseded).toEqual([list[0]]);
  });

  /// A bot's comment never absorbs a person's, even with their text.
  it("never folds a human comment into a bot's group", () => {
    const list = [coverage(1, "80", "a1b2c3d4"), human("coverage-bot-fan", 2, "## Coverage report\nnice"), coverage(3, "81", "b1c2d3e4")];
    const out = foldSuperseded(list);
    expect(out.map((e) => e.comment)).toEqual([list[1], list[2]]);
  });

  /// Everything not superseded keeps its place; the newest of a kind
  /// sits where the newest was posted, not where the first was.
  it("keeps chronological order for everything shown", () => {
    const list = [
      coverage(1, "80", "a1b2c3d4"),
      human("alice", 2, "Why this change?"),
      coverage(3, "81", "b1c2d3e4"),
      human("bob", 4, "Because of the retry."),
      coverage(5, "82", "c1d2e3f4"),
      human("alice", 6, "Thanks."),
    ];
    const out = foldSuperseded(list);
    expect(out.map((e) => e.comment)).toEqual([list[1], list[3], list[4], list[5]]);
    expect(out[2].superseded).toEqual([list[0], list[2]]);
  });

  /// A Windows-authored body is the same kind as its LF twin.
  it("treats a CRLF body as the same kind as its LF twin", () => {
    const list = [bot("ci-bot", 1, "## Build 41 passed\r\nall green"), bot("ci-bot", 2, "## Build 42 passed\nall green")];
    expect(foldSuperseded(list)).toHaveLength(1);
  });

  /// Line ends are normalised BEFORE the body is split on `\n`. `trim()`
  /// hides a CRLF's `\r`, so only a lone `\r` shows the difference:
  /// unnormalised, each body is one "first line" carrying its different
  /// second line, and the two rounds would never be the same kind.
  it("splits a body on a lone carriage return too", () => {
    const list = [bot("ci-bot", 1, "## Build 41 passed\rall green"), bot("ci-bot", 2, "## Build 42 passed\rtwo flaky")];
    expect(foldSuperseded(list)).toHaveLength(1);
  });

  it("returns nothing for nothing", () => {
    expect(foldSuperseded([])).toEqual([]);
  });
});

describe("the signature", () => {
  it("masks numbers, hashes, dates, times and URLs", () => {
    expect(
      normaliseSignature("Run 1,234 at 2026-01-02T03:04:05Z (12:30) for 3f2a9c1d: 81.4% https://ci.example.com/x/9"),
    ).toBe("Run <n> at <date> (<time>) for <hash>: <n> <url>");
  });

  /// Words made of hex letters are not hashes.
  it("does not take an ordinary word for a hash", () => {
    expect(normaliseSignature("added faded decade")).toBe("added faded decade");
  });

  it("gives a comment with no text and no marker no kind", () => {
    expect(commentKind(bot("ci-bot", 1, "   \n  "))).toBeNull();
    expect(commentKind(human("alice", 1, "no marker"))).toBeNull();
  });
});

describe("supersededLabel", () => {
  it("states the count when every comment arrived", () => {
    expect(supersededLabel(3, false)).toBe("Superseded (3 older)");
  });

  /// Partial is not nothing, and only-low is qualified: the fetch took
  /// the newest comments, so more older copies may sit beyond it.
  it("qualifies the count when the fetch was truncated", () => {
    expect(supersededLabel(3, true)).toBe("Superseded (at least 3 older)");
  });
});
