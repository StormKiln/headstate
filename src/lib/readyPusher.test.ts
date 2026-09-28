import { describe, expect, it } from "vitest";
import type { RowPusher } from "@/types/pr";
import {
  approvalWontCount,
  partitionReady,
  partitionSummary,
  readyPusher,
  type ReadyPusher,
} from "./readyPusher";

const HEAD = "1111111111111111111111111111111111111111";

type Rules = RowPusher["rules"];
type Last = RowPusher["last_pusher"];

const RULE_ON: Rules = {
  state: "read",
  require_last_push_approval: true,
  required_review_thread_resolution: false,
};
const RULE_OFF: Rules = { ...RULE_ON, require_last_push_approval: false };
const RULE_DECLINED: Rules = { state: "declined", reason: "budget" };
const RULE_UNREADABLE: Rules = { state: "unreadable", reason: "404" };

const known = (login: string): Last => ({ state: "known", login });
const DECLINED: Last = { state: "declined", reason: "budget" };
const UNKNOWN: Last = { state: "unknown", reason: "log lags" };

const answer = (rules: Rules, last: Last, head_oid = HEAD): RowPusher => ({
  repo: "acme/widget",
  number: 1,
  head_oid,
  rules,
  last_pusher: last,
});

const row = { head_oid: HEAD };

describe("readyPusher (#1576)", () => {
  it("names the viewer only when the log names the viewer", () => {
    expect(readyPusher(row, answer(RULE_ON, known("me")), "me").pusher).toEqual({
      state: "viewer",
      login: "me",
    });
    expect(readyPusher(row, answer(RULE_ON, known("them")), "me").pusher).toEqual({
      state: "other",
      login: "them",
    });
  });

  it("treats a declined lookup as not checked, never as unknown", () => {
    expect(readyPusher(row, answer(RULE_ON, DECLINED), "me").pusher.state).toBe("pending");
    expect(readyPusher(row, answer(RULE_ON, UNKNOWN), "me").pusher.state).toBe("unknown");
  });

  it("is not checked before an answer arrives or while the viewer loads", () => {
    expect(readyPusher(row, undefined, "me").pusher.state).toBe("pending");
    expect(readyPusher(row, answer(RULE_ON, known("me")), undefined).pusher.state).toBe(
      "pending",
    );
  });

  // "Could not read who I am" is checked-and-failed, not "not yet".
  it("is unknown when the viewer's login could not be read", () => {
    expect(readyPusher(row, answer(RULE_ON, known("me")), null).pusher.state).toBe("unknown");
  });

  // The pusher of an old head says nothing about the new one.
  it("drops an answer about a head the row has moved off", () => {
    const stale = answer(RULE_ON, known("me"), "2222222222222222222222222222222222222222");
    expect(readyPusher(row, stale, "me")).toEqual({
      pusher: { state: "pending" },
      rule: "unread",
    });
  });

  it("reads the rule only when the rules were read", () => {
    expect(readyPusher(row, answer(RULE_ON, DECLINED), "me").rule).toBe("required");
    expect(readyPusher(row, answer(RULE_OFF, DECLINED), "me").rule).toBe("not-required");
    expect(readyPusher(row, answer(RULE_DECLINED, DECLINED), "me").rule).toBe("unread");
    expect(readyPusher(row, answer(RULE_UNREADABLE, DECLINED), "me").rule).toBe("unread");
  });

  it("says the approval won't count only with the viewer's push under a read rule", () => {
    const r = (rules: Rules, last: Last) => approvalWontCount(readyPusher(row, answer(rules, last), "me"));
    expect(r(RULE_ON, known("me"))).toBe(true);
    expect(r(RULE_OFF, known("me"))).toBe(false);
    expect(r(RULE_UNREADABLE, known("me"))).toBe(false);
    expect(r(RULE_ON, known("them"))).toBe(false);
    expect(r(RULE_ON, UNKNOWN)).toBe(false);
  });
});

describe("partitionReady (#1576)", () => {
  const rp = (rules: Rules, last: Last): ReadyPusher => readyPusher(row, answer(rules, last), "me");
  const rows: [string, ReadyPusher][] = [
    ["mine-rule-on", rp(RULE_ON, known("me"))],
    ["mine-rule-off", rp(RULE_OFF, known("me"))],
    ["mine-rule-unread", rp(RULE_UNREADABLE, known("me"))],
    ["theirs-rule-on", rp(RULE_ON, known("them"))],
    ["declined-rule-on", rp(RULE_ON, DECLINED)],
    ["unknown-rule-on", rp(RULE_ON, UNKNOWN)],
    ["declined-rule-off", rp(RULE_OFF, DECLINED)],
    ["unknown-rule-declined", rp(RULE_DECLINED, UNKNOWN)],
  ];
  const run = (mode: "auto" | "hide" | "show") =>
    partitionReady(rows, ([, p]) => p, mode);
  const names = (xs: [string, ReadyPusher][]) => xs.map(([n]) => n);

  // Only the viewer's push under a rule READ as true is hidden
  // automatically. A rule not read, a pusher not known: never.
  it("auto hides only a known viewer push under a rule read as true", () => {
    const p = run("auto");
    expect(p.hidden).toBe(1);
    expect(names(p.shown)).not.toContain("mine-rule-on");
    expect(names(p.shown)).toContain("mine-rule-off");
    expect(names(p.shown)).toContain("mine-rule-unread");
    expect(names(p.shown)).toContain("declined-rule-on");
    expect(names(p.shown)).toContain("unknown-rule-on");
  });

  it("auto counts as undecided only the rows it might have hidden", () => {
    const p = run("auto");
    // declined-rule-on is not checked. mine-rule-unread (rule unread),
    // unknown-rule-on and unknown-rule-declined could not be decided.
    // declined-rule-off is decided: that base never hides.
    expect(p.notChecked).toBe(1);
    expect(p.unknown).toBe(3);
  });

  it("hide hides every known viewer push, and counts undecided pushers", () => {
    const p = run("hide");
    expect(p.hidden).toBe(3);
    expect(names(p.shown)).toContain("theirs-rule-on");
    expect(p.notChecked).toBe(2);
    expect(p.unknown).toBe(2);
    // Undecided rows are SHOWN, never hidden on a guess.
    expect(names(p.shown)).toEqual(
      expect.arrayContaining(["declined-rule-on", "unknown-rule-on", "declined-rule-off"]),
    );
  });

  it("show hides nothing and counts nothing", () => {
    const p = run("show");
    expect(p.shown).toHaveLength(rows.length);
    expect([p.hidden, p.notChecked, p.unknown]).toEqual([0, 0, 0]);
  });

  it("keeps the rows' order", () => {
    expect(names(run("show").shown)).toEqual(names(rows));
  });
});

describe("partitionSummary (#1576)", () => {
  it("says how many were hidden and how many could not be decided", () => {
    expect(partitionSummary({ hidden: 3, notChecked: 2, unknown: 1 }, "hide")).toBe(
      "3 hidden: you pushed last · 2 not checked yet · 1 could not be decided",
    );
    expect(partitionSummary({ hidden: 1, notChecked: 0, unknown: 0 }, "auto")).toBe(
      "1 hidden: you pushed last and your approval can't count",
    );
  });

  it("says nothing when there is nothing to say", () => {
    expect(partitionSummary({ hidden: 0, notChecked: 0, unknown: 0 }, "auto")).toBeNull();
    expect(partitionSummary({ hidden: 2, notChecked: 1, unknown: 0 }, "show")).toBeNull();
  });
});
