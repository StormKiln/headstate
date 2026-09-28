import { describe, expect, it } from "vitest";
import { PR_FIXTURES } from "../fixtures/prs";
import type { PullRequest } from "@/types/pr";
import { READY_BATCH_PROMPT, readyBatchPrompt } from "./readyClaudify";
import { type ReadyRow, readyListMarkdown } from "./readyMarkdown";

const NOW = new Date("2026-09-28T12:00:00Z");

/// #1579's instruction, typed out here independently of the source, so a
/// change to either is caught. Byte for byte: plain ASCII, `\n` line ends,
/// no trailing newline.
const ISSUE_TEXT =
  "Review all of these PRs for eligibility to approve and merge. For each PR, if all of the below conditions are satisfied, then approve and merge it:\n" +
  "- CI is green\n" +
  "- There are no merge conflicts\n" +
  "- The PR is ready for review (not in draft)\n" +
  "- There are no concerns or issues identified in comments (e.g. other reviewers, wiz, ai reviews, etc) that are relevant to the latest state of the branch\n" +
  "- I did not push most recently to the branch";

const pr = (number: number, over: Partial<PullRequest> = {}): PullRequest => ({
  ...PR_FIXTURES[0],
  number,
  title: `Change ${number}`,
  repo: "acme/widget",
  url: `https://github.com/acme/widget/pull/${number}`,
  head_ref: `feature/${number}`,
  base_ref: "main",
  head_oid: `${number}`.repeat(40).slice(0, 40),
  ready_at: "2026-09-27T12:00:00Z",
  unresolved_threads: 0,
  unresolved_threads_floor: false,
  ...over,
});

const ROWS: ReadyRow[] = [
  { pr: pr(1), lastPusher: { state: "known", login: "carol" } },
  { pr: pr(2), lastPusher: { state: "unknown" } },
  { pr: pr(3), lastPusher: { state: "not-checked" } },
  { pr: pr(4) },
];

const entryFor = (prompt: string, n: number): string[] => {
  const lines = prompt.split("\n");
  const at = lines.findIndex((l) => l.startsWith(`- [Change ${n}]`));
  expect(at).toBeGreaterThan(-1);
  return lines.slice(at, at + 4);
};

describe("readyBatchPrompt (#1579)", () => {
  it("opens with #1579's instruction, byte for byte", () => {
    expect(READY_BATCH_PROMPT).toBe(ISSUE_TEXT);
    const prompt = readyBatchPrompt(ROWS, "me", NOW);
    expect(prompt.startsWith(`${ISSUE_TEXT}\n\n`)).toBe(true);
  });

  it("says who the viewer is", () => {
    expect(readyBatchPrompt(ROWS, "someone-else", NOW)).toContain("\nI am @someone-else on GitHub.\n");
  });

  it("follows with the SAME formatter's list, in its agent form, and nothing else", () => {
    const prompt = readyBatchPrompt(ROWS, "me", NOW);
    expect(prompt).toBe(
      `${ISSUE_TEXT}\n\nI am @me on GitHub.\n\n${readyListMarkdown(ROWS, { now: NOW, forAgent: true })}`,
    );
  });

  it("lists exactly the rows given, in order, and says how many", () => {
    const prompt = readyBatchPrompt([ROWS[2], ROWS[0]], "me", NOW);
    expect(prompt).toContain("2 pull requests");
    expect(prompt.indexOf("[Change 3]")).toBeLessThan(prompt.indexOf("[Change 1]"));
    expect(prompt).not.toContain("[Change 2]");
  });

  it("gives Claude what it needs to check each one, labelled as a snapshot", () => {
    const prompt = readyBatchPrompt(ROWS, "me", NOW);
    expect(prompt).toMatch(/snapshot/);
    expect(prompt).toMatch(/Re-check each one yourself before acting on it/);
    const one = entryFor(prompt, 1);
    expect(one[0]).toContain("`feature/1` → `main`");
    expect(one[1]).toBe("  - repository: acme/widget, number: 1");
    expect(one[2]).toBe(`  - head commit: \`${"1".repeat(40)}\``);
  });

  it("states every pusher, and never an unknown one as not the viewer", () => {
    const prompt = readyBatchPrompt(ROWS, "me", NOW);
    expect(entryFor(prompt, 1)[3]).toBe("  - last pusher: @carol");
    expect(entryFor(prompt, 2)[3]).toBe("  - last pusher: unknown (checked, could not be determined)");
    expect(entryFor(prompt, 3)[3]).toBe("  - last pusher: not checked");
    // Absent is nobody-asked, the same as not-checked.
    expect(entryFor(prompt, 4)[3]).toBe("  - last pusher: not checked");
    expect(prompt).not.toMatch(/not (me|you)\b/i);
  });

  it("marks an absent head commit rather than inventing one", () => {
    const prompt = readyBatchPrompt([{ pr: pr(5, { head_oid: "" }) }], "me", NOW);
    expect(entryFor(prompt, 5)[2]).toBe("  - head commit: unknown");
  });
});
