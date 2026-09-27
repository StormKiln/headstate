import { describe, expect, it } from "vitest";
import { changeFromArgs, creationFromWrite, linesOf, numberHunk, wordDiff } from "./diff";
import { RECORDED_CHANGE } from "./fixtures";

describe("numberHunk", () => {
  /// The walk: context advances both sides, a removal the old side, an
  /// addition the new side.
  it("numbers each line on the sides it is on", () => {
    const h = numberHunk(RECORDED_CHANGE.hunks[0]);
    expect(h.lines.map((l) => [l.op, l.oldNo, l.newNo])).toEqual([
      ["context", 10, 10],
      ["removed", 11, null],
      ["added", null, 11],
      ["added", null, 12],
      ["context", 12, 13],
    ]);
  });

  it("heads a recorded hunk with its counts", () => {
    // 3 old lines (2 context + 1 removed), 4 new (2 context + 2 added).
    expect(numberHunk(RECORDED_CHANGE.hunks[0]).header).toBe("@@ -10,3 +10,4 @@");
  });

  /// Counts from the shown lines would be wrong when lines were
  /// dropped, so the header keeps only the starts.
  it("drops the counts when the hunk is partly shown", () => {
    const h = numberHunk({ ...RECORDED_CHANGE.hunks[0], lines_omitted: 5 });
    expect(h.header).toBe("@@ -10 +10 @@");
    expect(h.linesOmitted).toBe(5);
    expect(numberHunk(RECORDED_CHANGE.hunks[0], { countsKnown: false }).header).toBe(
      "@@ -10 +10 @@",
    );
  });

  /// A reconstructed hunk recorded no numbers; inventing them from 1
  /// would be fabrication.
  it("gives a reconstructed hunk no numbers and no header", () => {
    const h = numberHunk({
      old_start: null,
      new_start: null,
      lines: [
        { op: "removed", text: "a" },
        { op: "added", text: "b" },
      ],
      lines_omitted: 0,
    });
    expect(h.header).toBeNull();
    expect(h.lines.every((l) => l.oldNo === null && l.newNo === null)).toBe(true);
  });

  it("marks the changed words of a paired removal and addition", () => {
    const h = numberHunk(RECORDED_CHANGE.hunks[0]);
    const removed = h.lines[1].segments;
    const added = h.lines[2].segments;
    expect(removed?.filter((s) => s.changed).map((s) => s.text)).toEqual(["2"]);
    expect(added?.filter((s) => s.changed).map((s) => s.text)).toEqual(["3"]);
    // The unpaired second addition is drawn whole.
    expect(h.lines[3].segments).toBeNull();
  });
});

describe("wordDiff", () => {
  it("returns nothing for lines that share nothing", () => {
    expect(wordDiff("alpha", "beta")).toBeNull();
  });

  it("refuses a pair too large to compare rather than stall", () => {
    const long = "x ".repeat(400);
    expect(wordDiff(long, `${long}y`)).toBeNull();
  });
});

describe("changeFromArgs", () => {
  it("reconstructs an edit as removed then added lines, labelled reconstructed", () => {
    const c = changeFromArgs({
      tool: "edit",
      file_path: "a.txt",
      old_string: "one\ntwo",
      new_string: "one\n2",
      replace_all: false,
      truncated: false,
    });
    expect(c?.source).toBe("reconstructed");
    expect(c?.hunks[0].old_start).toBeNull();
    expect(c?.hunks[0].lines.map((l) => l.op)).toEqual(["removed", "removed", "added", "added"]);
  });

  it("gives each replacement of a multi-edit its own hunk and carries the omitted count", () => {
    const c = changeFromArgs({
      tool: "multi_edit",
      file_path: "a.txt",
      edits: [
        { old_string: "a", new_string: "b", replace_all: false, truncated: false },
        { old_string: "c", new_string: "d", replace_all: false, truncated: false },
      ],
      edits_omitted: 3,
    });
    expect(c?.hunks).toHaveLength(2);
    expect(c?.hunks_omitted).toBe(3);
  });

  it("builds nothing for a tool that is not an edit", () => {
    expect(changeFromArgs({ tool: "none" })).toBeNull();
  });
});

describe("creationFromWrite", () => {
  it("numbers a new file's lines from 1 under a /dev/null-style header", () => {
    const c = creationFromWrite("new.txt", "a\nb\nc\n");
    const h = numberHunk(c.hunks[0]);
    expect(h.header).toBe("@@ -0,0 +1,3 @@");
    expect(h.lines.map((l) => l.newNo)).toEqual([1, 2, 3]);
    expect(c.created).toBe(true);
  });
});

describe("linesOf", () => {
  it("normalises CRLF and does not count a trailing newline", () => {
    expect(linesOf("a\r\nb\r\n")).toEqual(["a", "b"]);
    expect(linesOf("")).toEqual([]);
  });
});
