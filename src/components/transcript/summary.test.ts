import { describe, expect, it } from "vitest";
import { call, DEAD, LIVE, output, UNKNOWN } from "./fixtures";
import {
  bashOutcome,
  callState,
  countLabel,
  durationBetween,
  formatDuration,
  mcpName,
  readSpan,
  searchSummary,
} from "./summary";

const BASH = { tool: "bash", command: "true", description: null, truncated: false } as const;

describe("callState", () => {
  /// The issue's states, kept apart: "still running" is said only of a
  /// live session, and "could not tell" is not a shade of "never came
  /// back".
  it("keeps paired, running, not recorded, unknown and unkeyed distinct", () => {
    expect(callState(call("Bash", BASH, output()), DEAD).state).toBe("paired");
    expect(callState(call("Bash", BASH), LIVE).state).toBe("running");
    expect(callState(call("Bash", BASH), DEAD).state).toBe("not_recorded");
    expect(callState(call("Bash", BASH), UNKNOWN).state).toBe("unknown");
    expect(callState(call("Bash", BASH, null, null), LIVE).state).toBe("unkeyed");
  });
});

describe("bashOutcome", () => {
  it("reads the exit code from an error result", () => {
    expect(bashOutcome(output({ is_error: true, text: "Exit code 2\nboom" }))).toEqual({
      kind: "error",
      code: 2,
    });
    expect(bashOutcome(output({ is_error: true, text: "denied" }))).toEqual({
      kind: "error",
      code: null,
    });
  });

  /// A non-error result records no exit code; it is "completed", never
  /// a claimed "exit 0". An unrecorded flag is neither.
  it("does not invent an exit code for a non-error result", () => {
    expect(bashOutcome(output({ is_error: false }))).toEqual({ kind: "completed" });
    expect(bashOutcome(output({ is_error: null }))).toEqual({ kind: "not_recorded" });
  });
});

describe("searchSummary", () => {
  it("takes Grep's own file total, true even when the list was clipped", () => {
    const s = searchSummary("grep", null, "Found 12 files\na.ts\nb.ts", true);
    expect(s.label).toBe("12 files");
    expect(s.items).toEqual(["a.ts", "b.ts"]);
  });

  it("sums count mode into matches across files", () => {
    const s = searchSummary("grep", "count", "a.ts:3\nb.ts:4", false);
    expect(s.label).toBe("7 matches in 2 files");
  });

  it("calls content-mode output lines, a floor when clipped", () => {
    expect(searchSummary("grep", "content", "1:x\n2:y", true).label).toBe("at least 2 lines");
  });

  it("counts a Glob's files and says a measured nothing", () => {
    expect(searchSummary("glob", null, "a\nb\nc", false).label).toBe("3 files");
    expect(searchSummary("glob", null, "No files found", false).label).toBe("0 files");
    expect(searchSummary("grep", null, "No files found", false).label).toBe("no matches");
  });
});

describe("readSpan", () => {
  it("counts numbered lines and their range", () => {
    expect(readSpan("   120\tfn a() {}\n   121\t}\n")).toEqual({ count: 2, first: 120, last: 121 });
    expect(readSpan("File does not exist.")).toBeNull();
  });
});

describe("mcpName", () => {
  it("splits server and tool", () => {
    expect(mcpName("mcp__docs__search_pages")).toEqual({ server: "docs", tool: "search_pages" });
    expect(mcpName("Bash")).toBeNull();
  });
});

describe("durations and counts", () => {
  it("is null, not zero, when a timestamp is missing or backwards", () => {
    expect(durationBetween(null, "2026-01-01T00:00:01Z")).toBeNull();
    expect(durationBetween("2026-01-01T00:00:02Z", "2026-01-01T00:00:01Z")).toBeNull();
    expect(durationBetween("2026-01-01T00:00:00Z", "2026-01-01T00:00:01.5Z")).toBe(1500);
  });

  it("formats durations without a 60-second minute", () => {
    expect(formatDuration(420)).toBe("420 ms");
    expect(formatDuration(1500)).toBe("1.5 s");
    expect(formatDuration(119_600)).toBe("2 m 00 s");
  });

  it("qualifies a count from clipped text", () => {
    expect(countLabel(1, "line", false)).toBe("1 line");
    expect(countLabel(40, "line", true)).toBe("at least 40 lines");
  });
});
