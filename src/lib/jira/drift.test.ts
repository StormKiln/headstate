import { describe, expect, it } from "vitest";
import type { JiraIssue, JiraLink, JiraStatusCategory, KeySearch, StatusExpectation } from "@/types/jira";
import {
  afterMerge,
  driftFor,
  fixForDrift,
  fixForVerdict,
  knownPrs,
  lookupComplete,
  startFix,
  type ExpectationMap,
  type KnownPr,
  type PrFate,
  type Verdict,
} from "./drift";
import facts from "../../../src-tauri/src/jira/fixtures/drift_facts.json";

const issue = (category: JiraStatusCategory, name: string): JiraIssue => ({
  key: "ABC-1",
  summary: "s",
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name, category },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: "ABC",
  url: null,
  updated: null,
});

describe("driftFor", () => {
  it.each<[JiraStatusCategory, string, string | null]>([
    ["new", "Backlog", "open-but-not-started"],
    ["in_progress", "In Testing", null],
    ["done", "Closed", "done-but-open"],
    // Absent is not zero: an unreported category is never To Do.
    ["unknown", "Odd", null],
  ])("%s (%s) → %s", (category, name, rule) => {
    expect(driftFor(issue(category, name), { draft: false, source: "title" })?.rule ?? null).toBe(rule);
  });

  it("names the project's own status in the message", () => {
    expect(driftFor(issue("new", "Backlog"), { draft: false, source: "branch" })?.message).toBe(
      "This pull request is open, but ABC-1 is still Backlog.",
    );
  });

  it("never flags a draft", () => {
    expect(driftFor(issue("new", "To Do"), { draft: true, source: "title" })).toBeNull();
    expect(driftFor(issue("done", "Done"), { draft: true, source: "title" })).toBeNull();
  });
});

/// Per-project expectations (#2034). The tests above pass no map and so
/// pin the defaults: a user who sets nothing sees exactly what they did.
describe("status expectations", () => {
  const pr = { draft: false, source: "title" as const };
  const named = (name: string): ExpectationMap => ({ ABC: { on_open: { kind: "status", name } } });

  it("matches a named status by name, whatever its category", () => {
    expect(driftFor(issue("in_progress", "In Review"), pr, named("In Review"))).toBeNull();
    expect(driftFor(issue("in_progress", "In Progress"), pr, named("In Review"))).toEqual({
      rule: "open-not-expected",
      message: "This pull request is open, but ABC-1 is In Progress, not In Review.",
    });
    expect(driftFor(issue("new", "To Do"), pr, named("In Review"))?.rule).toBe("open-but-not-started");
  });

  it("matches a category by category", () => {
    const notYet: ExpectationMap = { ABC: { on_open: { kind: "category", category: "new" } } };
    expect(driftFor(issue("new", "Backlog"), pr, notYet)).toBeNull();
    expect(driftFor(issue("in_progress", "Doing"), pr, notYet)?.message).toBe(
      "This pull request is open, but ABC-1 is Doing, not any To Do status.",
    );
  });

  it("applies only to the project it names", () => {
    expect(driftFor(issue("in_progress", "In Progress"), pr, { XY: { on_open: { kind: "status", name: "QA" } } })).toBeNull();
  });

  /// Absent is not a mismatch.
  it("decides nothing when the status it needs did not come back", () => {
    expect(driftFor(issue("in_progress", ""), pr, named("In Review"))).toBeNull();
    expect(afterMerge(issue("unknown", "Odd"), [pr_("merged")], true, { expectations: {}, now: NOW })).toBeNull();
  });

  it("compares a merged ticket against its own expectation", () => {
    const released: ExpectationMap = { ABC: { on_merged: { kind: "status", name: "Released" } } };
    const after = (i: JiraIssue) => afterMerge(i, [pr_("merged")], true, { expectations: released, now: NOW });
    expect(after(issue("done", "Released"))).toBeNull();
    expect(after(issue("done", "Done"))?.message).toBe(
      "A pull request for ABC-1 has merged, but the ticket is Done, not Released.",
    );
    // An unset side keeps its default.
    expect(driftFor(issue("in_progress", "Doing"), pr, released)).toBeNull();
  });
});

const NOW = new Date("2026-01-02T12:00:00Z");
const LONG_AGO = "2026-01-02T11:00:00Z";

let serial = 0;
function pr_(fate: PrFate, over: Partial<KnownPr> = {}): KnownPr {
  return { url: `u-${(serial += 1)}`, fate, draft: false, merged_at: fate === "merged" ? LONG_AGO : null, source: "title", ...over };
}

/// The rule table for pull requests no longer open (#2042): R2, R3, R5,
/// each with its partial and unknown cases. `complete` is whether the
/// lookup asked everywhere it could.
describe("afterMerge", () => {
  const at = (category: JiraStatusCategory, name: string, prs: KnownPr[], complete = true, graceMins?: number) =>
    afterMerge(issue(category, name), prs, complete, { now: NOW, graceMins });
  const shape = (v: ReturnType<typeof afterMerge>) => (v ? `${v.rule}/${v.level}` : null);

  it.each<[string, JiraStatusCategory, PrFate[], boolean, string | null]>([
    // R2: all merged, ticket not where a merge should leave it.
    ["R2 fact", "in_progress", ["merged"], true, "merged-not-done/fact"],
    ["R2 merged + closed is still R2", "in_progress", ["merged", "closed"], true, "merged-not-done/fact"],
    ["R2 met", "done", ["merged"], true, null],
    ["R2 partial lookup", "in_progress", ["merged"], false, "merged-not-done/qualified"],
    ["R2 an own pull request's fate unknown", "in_progress", ["merged", "unknown"], true, "merged-not-done/qualified"],
    ["R2 unknown category", "unknown", ["merged"], true, null],
    // R3: some merged, some open. A count, never a warning.
    ["R3", "in_progress", ["merged", "open", "open"], true, "some-merged/progress"],
    ["R3 with the ticket done", "done", ["merged", "open"], true, "some-merged/progress"],
    // R5: closed unmerged, nothing open, ticket in progress.
    ["R5 fact", "in_progress", ["closed"], true, "closed-not-merged/fact"],
    ["R5 partial lookup", "in_progress", ["closed"], false, "closed-not-merged/qualified"],
    ["R5 ticket to do", "new", ["closed"], true, null],
    ["R5 ticket done", "done", ["closed"], true, null],
    ["R5 unknown category", "unknown", ["closed"], true, null],
    // Nothing known is nothing to say.
    ["only unknown", "in_progress", ["unknown"], true, null],
    ["only open", "new", ["open"], true, null],
    ["none", "in_progress", [], true, null],
  ])("%s", (_, category, fates, complete, want) => {
    expect(shape(at(category, "S", fates.map((f) => pr_(f)), complete))).toBe(want);
  });

  it("says what it could not check, and never warns", () => {
    expect(at("in_progress", "In Review", [pr_("merged")], false)?.message).toBe(
      "A pull request for ABC-1 has merged and the ticket is still In Review; other pull requests for ABC-1 were not checked.",
    );
    expect(at("in_progress", "In Review", [pr_("merged"), pr_("unknown")])?.message).toBe(
      "A pull request for ABC-1 has merged and the ticket is still In Review; 1 other pull request for ABC-1 could not be checked.",
    );
    expect(at("in_progress", "In Review", [pr_("merged")])?.message).toBe(
      "A pull request for ABC-1 has merged, but the ticket is still In Review.",
    );
  });

  it("counts R3, and floors the count when the lookup was partial", () => {
    expect(at("in_progress", "x", [pr_("merged"), pr_("open"), pr_("open")])?.message).toBe("1 of 3 pull requests for ABC-1 merged.");
    expect(at("in_progress", "x", [pr_("merged"), pr_("open")], false)?.message).toBe("1 of at least 2 pull requests for ABC-1 merged.");
  });

  it("an open draft is still work in progress, so R2 waits for it", () => {
    expect(shape(at("in_progress", "x", [pr_("merged"), pr_("open", { draft: true })]))).toBe("some-merged/progress");
  });

  it("waits the grace period after the latest merge", () => {
    const recent = pr_("merged", { merged_at: "2026-01-02T11:50:00Z" });
    expect(at("in_progress", "Doing", [pr_("merged"), recent])).toEqual({
      rule: "merged-not-done",
      level: "waiting",
      message: "A pull request for ABC-1 merged 10 minutes ago; the ticket is still Doing for now.",
    });
    expect(shape(at("in_progress", "Doing", [recent], true, 5))).toBe("merged-not-done/fact");
    expect(shape(at("in_progress", "Doing", [recent], true, 0))).toBe("merged-not-done/fact");
  });

  it("a merge with no reported time cannot be timed, so is qualified", () => {
    expect(at("in_progress", "Doing", [pr_("merged", { merged_at: null })])?.level).toBe("qualified");
  });
});

const asked = (over: Partial<Extract<KeySearch, { state: "asked" }>> = {}): KeySearch => ({
  state: "asked", at: "2026-01-02T11:59:00Z", total: 1, read: 1, truncated: false, scope: ["octocat"], unsearched: [], ...over,
});

describe("lookupComplete", () => {
  it.each<[string, KeySearch, KeySearch | null, boolean]>([
    ["asked, whole, no GitLab", asked(), null, true],
    ["both halves whole", asked(), asked(), true],
    ["truncated", asked({ truncated: true }), null, false],
    ["owners left out", asked({ unsearched: ["acme"] }), null, false],
    ["GitLab project did not answer", asked(), asked({ unsearched: ["gitlab.example.test/a"] }), false],
    ["not asked: the database knew", { state: "not_asked", reason: "already known from your pull requests" }, null, false],
    ["failed", { state: "failed", error: "x" }, null, false],
    ["GitLab not asked", asked(), { state: "not_asked", reason: "the GitLab CLI (glab) is not installed" }, false],
  ])("%s", (_, search, gitlab, want) => {
    expect(lookupComplete({ search, gitlab })).toBe(want);
  });
});

describe("knownPrs", () => {
  const link = (over: Partial<JiraLink> & { t?: Partial<Extract<JiraLink["target"], { kind: "pr" }>> }): JiraLink => ({
    key: "ABC-1",
    found_in: "title",
    current: true,
    ...over,
    target: {
      kind: "pr", source: { provider: "github", host: "github.com" }, role: "authored", repo: "octocat/hello-world",
      number: 1, title: "ABC-1", url: "https://github.com/octocat/hello-world/pull/1", head_ref: "x",
      is_draft: false, state: null, author: null, merged_at: null, ...over.t,
    },
  });

  it("a listed pull request is open, whatever an older search said", () => {
    expect(knownPrs([link({}), link({ current: false, t: { role: "searched", state: "merged" } })]).map((p) => p.fate)).toEqual(["open"]);
  });

  it("one that left its list is what the search said, else unknown", () => {
    const left = link({ current: false });
    expect(knownPrs([left]).map((p) => p.fate)).toEqual(["unknown"]);
    const found = link({ current: false, t: { role: "searched", state: "merged", merged_at: LONG_AGO } });
    expect(knownPrs([left, found])).toEqual([
      { url: "https://github.com/octocat/hello-world/pull/1", fate: "merged", draft: false, merged_at: LONG_AGO, source: "title" },
    ]);
  });

  it("authored and reviewing rows of one pull request are one", () => {
    expect(knownPrs([link({}), link({ t: { role: "reviewing" } })])).toHaveLength(1);
  });

  it("skips worktrees and branches", () => {
    expect(knownPrs([{ key: "ABC-1", found_in: "branch", current: true, target: { kind: "worktree", repo: "r", path: "/p", branch: "b" } }])).toEqual([]);
  });
});

/// The facts table shared with the desktop's notifier
/// (`src-tauri/src/jira/drift.rs`, #2043): what the page warns about as a
/// fact is exactly what may be announced.
describe("facts shared with the desktop", () => {
  interface Case { name: string; category: JiraStatusCategory; status: string; expectations?: ExpectationMap; fact: boolean }
  const table = facts as unknown as {
    r1: (Case & { draft: boolean })[];
    r2: (Case & { prs: { fate: PrFate; merged_at?: string }[]; complete: boolean; now: string; grace_mins: number })[];
  };
  it.each(table.r1)("R1 $name", (c) => {
    const d = driftFor(issue(c.category, c.status), { draft: c.draft, source: "title" }, c.expectations);
    expect(d?.rule === "open-but-not-started").toBe(c.fact);
  });
  it.each(table.r2)("R2 $name", (c) => {
    const prs = c.prs.map((p, i) => ({ url: `u${i}`, fate: p.fate, draft: false, merged_at: p.merged_at ?? null, source: "title" as const }));
    const v = afterMerge(issue(c.category, c.status), prs, c.complete, {
      expectations: c.expectations,
      now: new Date(c.now),
      graceMins: c.grace_mins,
    });
    expect(v?.rule === "merged-not-done" && v.level === "fact").toBe(c.fact);
  });
});

/// The fix a drift offers (#2042, #2045): a named expectation is a status
/// to move to; a category is the reader's choice, never a made-up name.
describe("fixes", () => {
  const pr = { draft: false, source: "title" as const };
  const todo = issue("new", "To Do");
  const onOpen = (e: StatusExpectation): ExpectationMap => ({ ABC: { on_open: e } });
  const onMerged = (e: StatusExpectation): ExpectationMap => ({ ABC: { on_merged: e } });
  const verdict = (level: Verdict["level"], rule: Verdict["rule"] = "merged-not-done"): Verdict => ({
    rule,
    level,
    message: "m",
  });

  it("R1 moves to a named on-open status, and lets the reader choose for a category", () => {
    const named = onOpen({ kind: "status", name: "In Review" });
    expect(fixForDrift(todo, driftFor(todo, pr, named)!, named)).toEqual({ kind: "move", status: "In Review" });
    // No expectation set: the default is a category, so no status is named.
    expect(fixForDrift(todo, driftFor(todo, pr)!)).toEqual({ kind: "choose", expected: "any In Progress status" });
    const doing = onOpen({ kind: "category", category: "in_progress" });
    expect(fixForDrift(todo, driftFor(todo, pr, doing)!, doing)?.kind).toBe("choose");
  });

  it("R4 and an off-expectation in-progress status offer nothing", () => {
    const done = issue("done", "Done");
    expect(fixForDrift(done, driftFor(done, pr)!)).toBeNull();
    const named = onOpen({ kind: "status", name: "In Review" });
    const doing = issue("in_progress", "In Progress");
    expect(fixForDrift(doing, driftFor(doing, pr, named)!, named)).toBeNull();
  });

  it("R2 as fact moves to a named on-merged status, or the reader chooses", () => {
    const review = issue("in_progress", "In Review");
    const named = onMerged({ kind: "status", name: "Closed" });
    expect(fixForVerdict(review, verdict("fact"), named)).toEqual({ kind: "move", status: "Closed" });
    expect(fixForVerdict(review, verdict("fact"))).toEqual({ kind: "choose", expected: "any Done status" });
  });

  it("a qualified, waiting or progress verdict, or R5, offers nothing", () => {
    const review = issue("in_progress", "In Review");
    const named = onMerged({ kind: "status", name: "Closed" });
    for (const level of ["qualified", "waiting", "progress"] as const) {
      expect(fixForVerdict(review, verdict(level), named)).toBeNull();
    }
    expect(fixForVerdict(review, verdict("fact", "closed-not-merged"), named)).toBeNull();
  });

  it("the real afterMerge verdicts: fact gets the fix, partial does not", () => {
    const review = issue("in_progress", "In Review");
    const named = onMerged({ kind: "status", name: "Closed" });
    const merged: KnownPr = {
      url: "u",
      fate: "merged",
      draft: false,
      merged_at: "2026-01-01T00:00:00Z",
      source: "title",
    };
    const now = new Date("2026-01-02T00:00:00Z");
    const fact = afterMerge(review, [merged], true, { expectations: named, now })!;
    expect(fact.level).toBe("fact");
    expect(fixForVerdict(review, fact, named)).toEqual({ kind: "move", status: "Closed" });
    const partial = afterMerge(review, [merged], false, { expectations: named, now })!;
    expect(partial.level).toBe("qualified");
    expect(fixForVerdict(review, partial, named)).toBeNull();
  });

  it("start work offers the on-open move only while the ticket does not meet it", () => {
    expect(startFix(todo)).toEqual({ kind: "choose", expected: "any In Progress status" });
    expect(startFix(todo, onOpen({ kind: "status", name: "In Progress" }))).toEqual({
      kind: "move",
      status: "In Progress",
    });
    expect(startFix(issue("in_progress", "In Progress"))).toBeNull();
    // Whether an unreported category meets it cannot be told: no offer.
    expect(startFix(issue("unknown", "Odd"))).toBeNull();
  });
});
