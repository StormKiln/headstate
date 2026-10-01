import { describe, expect, it } from "vitest";
import { SyntheticTranscript } from "./transcriptFile";
import type { TranscriptPage } from "../types/transcript";

const page = (id: string, bytes = 100) => ({ messages: [{ id, blocks: [] }], bytes_read: bytes, file_bytes: bytes, truncated: false }) as unknown as TranscriptPage;

describe("synthetic transcript file", () => {
  it("serves the newest grown page on end reads, including growth not polled yet", () => {
    const file = new SyntheticTranscript(page("original"));
    file.append(page("grown"));
    expect(file.end().page.messages[0].id).toBe("grown");
    expect(file.end().end.offset).toBe(200);
    expect(file.end().start.offset).toBe(100);
    expect(file.end().page.file_bytes).toBe(200);
  });
  it("gives whole parser fixtures with zero byte metadata a real synthetic cursor span", () => {
    const file = new SyntheticTranscript(page("original", 0));
    const oldEnd = file.size;
    expect(oldEnd).toBeGreaterThan(0);
    file.append(page("grown", 0));
    expect(file.size).toBeGreaterThan(oldEnd);
    expect(file.after(oldEnd).page.messages[0].id).toBe("grown");
  });
  it("replays contiguous grown pages from a lagging cursor and reports the actual live edge", () => {
    const file = new SyntheticTranscript(page("original"));
    file.append(page("one"));
    file.append(page("two"));
    expect(file.after(100)).toMatchObject({ at_end: false, start: { offset: 100 }, end: { offset: 200 }, page: { file_bytes: 300 } });
    expect(file.after(200).page.messages[0].id).toBe("two");
    expect(file.after(300).page.messages).toEqual([]);
    expect(() => file.after(150)).toThrow("cursor");
  });
});
