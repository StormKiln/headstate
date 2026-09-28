import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import type { PullRequest } from "@/types/pr";
import { escapeMarkdownText, lastPusherOf, readyListMarkdown } from "./readyMarkdown";

const NOW = new Date("2026-09-28T12:00:00Z");

const pr = (over: Partial<PullRequest>): PullRequest => ({
  ...PR_FIXTURES[0],
  review: "none",
  ...over,
});

const ONE = pr({
  number: 12,
  title: "Add retry to the client",
  url: "https://github.com/acme/widget/pull/12",
  repo: "acme/widget",
  author: "alice",
  head_ref: "feature/retry",
  base_ref: "main",
  head_oid: "0123456789abcdef0123456789abcdef01234567",
  ready_at: "2026-09-26T12:00:00Z",
  ci: "success",
  merge: "mergeable",
  is_draft: false,
  unresolved_threads: 2,
});

const TWO = pr({
  number: 7,
  title: "Fix the flaky test",
  url: "https://github.com/acme/gadget/pull/7",
  repo: "acme/gadget",
  author: "bob",
  head_ref: "fix/flaky",
  base_ref: "develop",
  ready_at: "2026-09-28T09:30:00Z",
  ci: "pending",
  merge: "checking",
  unresolved_threads: 0,
});

describe("readyListMarkdown (#1578)", () => {
  it("renders a known fixture exactly", () => {
    expect(readyListMarkdown([{ pr: ONE }, { pr: TWO }], { now: NOW })).toBe(
      [
        "**Ready for review**: 2 pull requests, as of 2026-09-28 12:00 UTC.",
        "",
        "- [Add retry to the client](https://github.com/acme/widget/pull/12) — acme/widget #12 · by @alice · `feature/retry` → `main` · ready 2d · CI green · no conflicts · not a draft · 2 unresolved conversations",
        "- [Fix the flaky test](https://github.com/acme/gadget/pull/7) — acme/gadget #7 · by @bob · `fix/flaky` → `develop` · ready 2h · CI pending · conflicts not yet computed · not a draft · no unresolved conversations",
        "",
      ].join("\n"),
    );
  });

  it("lists exactly the rows given, in the order given", () => {
    const md = readyListMarkdown([{ pr: TWO }, { pr: ONE }], { now: NOW });
    expect(md.indexOf("acme/gadget #7")).toBeLessThan(md.indexOf("acme/widget #12"));
    expect(readyListMarkdown([{ pr: TWO }], { now: NOW })).not.toContain("acme/widget");
    expect(readyListMarkdown([{ pr: TWO }], { now: NOW })).toContain("1 pull request,");
  });

  it("escapes a title so it cannot break the link or the list", () => {
    const md = readyListMarkdown(
      [{ pr: pr({ title: "Fix [a] | `b` \\ c\nd" }) }],
      { now: NOW },
    );
    expect(md).toContain("- [Fix \\[a\\] \\| \\`b\\` \\\\ c d](");
    expect(escapeMarkdownText("a]b")).toBe("a\\]b");
    // One entry line: the newline did not end the list item.
    expect(md.split("\n").filter((l) => l.startsWith("- ")).length).toBe(1);
  });

  it("fences a branch that contains a backtick", () => {
    const md = readyListMarkdown([{ pr: pr({ head_ref: "odd`name" }) }], { now: NOW });
    expect(md).toContain("``odd`name`` →");
  });

  it("marks or omits unknowns and never guesses them", () => {
    const md = readyListMarkdown(
      [
        {
          pr: pr({
            ready_at: null,
            author: "",
            unresolved_threads: undefined as unknown as number,
          }),
          lastPusher: { state: "unknown" },
        },
      ],
      { now: NOW },
    );
    expect(md).toContain("ready time unknown");
    expect(md).not.toMatch(/ready \d|ready <1m/);
    expect(md).not.toContain("by @");
    expect(md).toContain("unresolved conversations unknown");
    expect(md).not.toContain("no unresolved");
    // An undecided pusher is left out entirely -- never "not you".
    expect(md).not.toMatch(/push/i);
  });

  it("names the last pusher only when known", () => {
    const known = readyListMarkdown([{ pr: ONE, lastPusher: { state: "known", login: "carol" } }], {
      now: NOW,
    });
    expect(known).toContain("last push by @carol");
    for (const lastPusher of [undefined, { state: "not-checked" as const }]) {
      expect(readyListMarkdown([{ pr: ONE, lastPusher }], { now: NOW })).not.toMatch(/push/i);
    }
  });

  it("qualifies an unresolved count that is a floor", () => {
    const md = readyListMarkdown([{ pr: ONE, unresolvedIsFloor: true }], { now: NOW });
    expect(md).toContain("at least 2 unresolved conversations");
  });

  it("maps the strip's pusher: not-checked apart from unknown, a known login named", () => {
    expect(lastPusherOf({ state: "pending" })).toEqual({ state: "not-checked" });
    expect(lastPusherOf({ state: "unknown" })).toEqual({ state: "unknown" });
    expect(lastPusherOf({ state: "viewer", login: "me" })).toEqual({ state: "known", login: "me" });
    expect(lastPusherOf({ state: "other", login: "bob" })).toEqual({ state: "known", login: "bob" });
  });

  it("says when a pull request is a draft or has conflicts", () => {
    const md = readyListMarkdown(
      [{ pr: pr({ is_draft: true, merge: "conflicted", ci: "failure" }) }],
      { now: NOW },
    );
    expect(md).toContain("· draft ·");
    expect(md).toContain("has conflicts");
    expect(md).toContain("CI failing");
  });
});
