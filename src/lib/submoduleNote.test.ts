import { describe, expect, it } from "vitest";
import { submoduleNote } from "./worktrees";

const sub = (over: Partial<{ total: number; dirty: number; out_of_sync: number }> = {}) => ({
  total: 1,
  dirty: 0,
  out_of_sync: 0,
  ...over,
});

describe("submoduleNote", () => {
  it("says nothing when there are no submodules", () => {
    // `null` is the state of 17 of the 18 repositories checked out
    // here, and a sentence on every row would be noise.
    expect(submoduleNote(null)).toBeNull();
    expect(submoduleNote(undefined)).toBeNull();
  });

  it("says nothing when every submodule is clean and in sync", () => {
    // The ordinary case for a repository that HAS submodules. A line
    // reading "0 submodules with changes" is the same noise.
    expect(submoduleNote(sub({ total: 3 }))).toBeNull();
  });

  it("names uncommitted submodule work", () => {
    // The whole point: the dirty count the user is already looking at
    // is right, but "1 uncommitted file" does not say that the file is
    // in a different repository with its own commit and push.
    expect(submoduleNote(sub({ dirty: 1 }))).toBe("1 submodule with uncommitted work");
    expect(submoduleNote(sub({ total: 3, dirty: 2 }))).toBe("2 submodules with uncommitted work");
  });

  it("keeps out-of-sync separate from dirty", () => {
    // Different remedies: one is a commit inside the submodule, the
    // other is `git submodule update`, and only the first loses work.
    expect(submoduleNote(sub({ out_of_sync: 1 }))).toBe("1 not at the recorded commit");
  });

  it("reports both when both are true, dirty first", () => {
    // Dirty leads because it is the half that means work would be
    // lost, and the half the count came from.
    const note = submoduleNote(sub({ total: 2, dirty: 1, out_of_sync: 1 }));
    expect(note).toBe("1 submodule with uncommitted work, 1 not at the recorded commit");
  });

  it("does not call an out-of-sync submodule out of date", () => {
    // It may be AHEAD of the recorded commit. "Out of date" would send
    // the user to `git submodule update`, which would DISCARD that.
    const note = submoduleNote(sub({ out_of_sync: 1 })) ?? "";
    expect(note).not.toMatch(/out of date|outdated|behind/i);
  });
});
