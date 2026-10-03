import { describe, expect, it } from "vitest";
import { commentKeys } from "./commentIdentity";
import type { PrComment } from "@/types/pr";
const comment = (body: string, id?: string): PrComment => ({ author: "alice", created_at: "2026-01-01T00:00:00Z", body, id, author_is_bot: false });
describe("comment identity compatibility", () => {
  it("keeps durable IDs distinct across edits, collisions, reorder and bounded-window removal", () => {
    const before = commentKeys([comment("same", "A"), comment("same", "B")]);
    expect(new Set(before).size).toBe(2);
    expect(commentKeys([comment("edited", "B"), comment("same", "A")])).toEqual([before[1], before[0]]);
    expect(commentKeys([comment("edited again", "B")])).toEqual([before[1]]);
  });
  it("preserves unique legacy author/time through edits and unrelated insertion", () => {
    const before = commentKeys([comment("before")])[0];
    expect(commentKeys([{ ...comment("other"), author: "bob" }, comment("after")])[1]).toBe(before);
  });
  it("does not conflate ambiguous legacy comments and conservatively resets changed buckets", () => {
    const before = commentKeys([comment("first"), comment("second"), comment("first")]);
    expect(new Set(before).size).toBe(3);
    expect(commentKeys([comment("second"), comment("first"), comment("first")])).toEqual([before[1], before[0], before[2]]);
    expect(commentKeys([comment("edited"), comment("second"), comment("first")])[0]).not.toBe(before[0]);
    expect(commentKeys([comment("second")])[0]).not.toBe(before[1]);
  });
});
