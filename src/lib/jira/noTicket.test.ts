import { describe, expect, it } from "vitest";
import type { JiraLink, LinkIndex } from "@/types/jira";
import { filterNoTicket, noTicketIndex, ticketState, titleWithKey } from "./noTicket";

const url = (n: number) => `https://github.com/octocat/hello-world/pull/${n}`;

function link(n: number, key: string, current = true, role: "authored" | "reviewing" = "authored"): JiraLink {
  return {
    key,
    found_in: "title",
    current,
    target: {
      kind: "pr",
      source: { provider: "github", host: "github.com" },
      role,
      repo: "octocat/hello-world",
      number: n,
      title: `${key} x`,
      url: url(n),
      head_ref: "b",
      is_draft: false,
      state: null,
      author: "octocat",
    },
  };
}

const index = (links: JiraLink[], checked: LinkIndex["checked"]) => noTicketIndex({ links, checked });

describe("ticketState", () => {
  const ix = index(
    [link(1, "ABC-1"), link(3, "ABC-3", false), link(4, "ABC-4", true, "reviewing")],
    [
      { url: url(1), title: "ABC-1 x", head_ref: "b" },
      { url: url(2), title: "plain", head_ref: "b" },
      { url: url(3), title: "renamed", head_ref: "b" },
      { url: url(4), title: "review", head_ref: "b" },
    ],
  );

  it("names a key when the index read one, as the row is now", () => {
    expect(ticketState(ix, { url: url(1), title: "ABC-1 x", head_ref: "b" })).toBe("has");
  });

  it("is no ticket only when the index read the row as it is now and found none", () => {
    expect(ticketState(ix, { url: url(2), title: "plain", head_ref: "b" })).toBe("none");
  });

  // Pending is not Unknown, and is not "none" (#1042).
  it("is pending for a pull request the index has not read", () => {
    expect(ticketState(ix, { url: url(9), title: "new", head_ref: "b" })).toBe("pending");
  });

  it("is pending when the title or branch moved since the read", () => {
    expect(ticketState(ix, { url: url(2), title: "plain, renamed", head_ref: "b" })).toBe("pending");
    expect(ticketState(ix, { url: url(2), title: "plain", head_ref: "other" })).toBe("pending");
  });

  it("counts only current, authored links", () => {
    expect(ticketState(ix, { url: url(3), title: "renamed", head_ref: "b" })).toBe("none");
    expect(ticketState(ix, { url: url(4), title: "review", head_ref: "b" })).toBe("none");
  });

  it("knows nothing from an index without the checked list (an older desktop)", () => {
    const old = noTicketIndex({ links: [] });
    expect(ticketState(old, { url: url(2), title: "plain", head_ref: "b" })).toBe("pending");
  });
});

describe("filterNoTicket", () => {
  it("keeps the no-ticket rows and counts the undecided ones, never as no ticket", () => {
    const ix = index([link(1, "ABC-1")], [
      { url: url(1), title: "ABC-1 x", head_ref: "b" },
      { url: url(2), title: "plain", head_ref: "b" },
    ]);
    const rows = [
      { url: url(1), title: "ABC-1 x", head_ref: "b", n: 1 },
      { url: url(2), title: "plain", head_ref: "b", n: 2 },
      { url: url(3), title: "unread", head_ref: "b", n: 3 },
    ];
    const got = filterNoTicket(rows, ix);
    expect(got.shown.map((r) => r.n)).toEqual([2]);
    expect(got.pending).toBe(1);
  });
});

describe("titleWithKey", () => {
  it("puts the key in front", () => {
    expect(titleWithKey("Fix login", "ABC-1")).toBe("ABC-1: Fix login");
    expect(titleWithKey("  Fix login", "ABC-1")).toBe("ABC-1: Fix login");
  });

  it("is a no-op when the title already names the key", () => {
    expect(titleWithKey("ABC-1: Fix login", "ABC-1")).toBeNull();
    expect(titleWithKey("Fix login (ABC-1)", "ABC-1")).toBeNull();
  });

  it("is idempotent: composing again changes nothing", () => {
    const once = titleWithKey("Fix login", "ABC-1")!;
    expect(titleWithKey(once, "ABC-1")).toBeNull();
  });

  it("does not take a longer key for this one", () => {
    expect(titleWithKey("ABC-1x Fix", "ABC-1")).toBe("ABC-1: ABC-1x Fix");
    expect(titleWithKey("XABC-1 Fix", "ABC-1")).toBe("ABC-1: XABC-1 Fix");
  });

  it("keeps a GitLab draft marker first, so the MR stays a draft", () => {
    expect(titleWithKey("Draft: Fix login", "ABC-1")).toBe("Draft: ABC-1: Fix login");
    expect(titleWithKey("[Draft] Fix", "XY-2")).toBe("[Draft] XY-2: Fix");
  });
});
