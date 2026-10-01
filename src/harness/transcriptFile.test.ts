import { describe, expect, it } from "vitest";
import { SyntheticTranscript } from "./transcriptFile";
import type { TranscriptPage, TranscriptWindow } from "../types/transcript";

const page = (id: string, bytes = 100) => ({ messages: [{ id, blocks: [] }], bytes_read: bytes, file_bytes: bytes, truncated: false }) as unknown as TranscriptPage;

const window = (id: string): TranscriptWindow => ({
  page: page(id), start: {offset: 0, behind_digest: "start"}, end: {offset: 100, behind_digest: "end"},
  at_start: true, at_end: true, rewritten: false,
  position: {first: 1, last: 1, total: 1, exact: true, basis: "index"},
  seam: {first_model: null, last_model: null}, bytes_scanned: 300,
});

describe("synthetic transcript file", () => {
  it("serves the newest grown page on end reads, including growth not polled yet", () => {
    const file = new SyntheticTranscript(window("original"));
    file.append(page("grown"));
    expect(file.end().page.messages[0].id).toBe("grown");
    expect(file.end().end.offset).toBeGreaterThan(100);
    expect(file.end().start.offset).toBe(100);
    expect(file.end().page.file_bytes).toBe(file.end().end.offset);
  });
  it("preserves real cursor, seam, position and I/O cost on the initial end response", () => {
    const original = window("original");
    original.start = {offset: 900, behind_digest: "real-start"};
    original.end = {offset: 1000, behind_digest: "real-end"};
    original.page.file_bytes = 1100;
    original.page.bytes_read = 730;
    original.at_start = false;
    original.seam = {first_model: {message_id: "first", model: "recorded-first", timestamp: null}, last_model: "recorded-last"};
    const file = new SyntheticTranscript(original);
    expect(file.end()).toEqual(original);
    expect(file.size).toBe(1100);
    expect(file.after(1000).page.messages).toEqual([]);
    expect(file.end()).toEqual(original);
  });
  it("refuses legacy pages and oversized synthetic inputs as production windows", () => {
    expect(() => new SyntheticTranscript(page("legacy") as unknown as TranscriptWindow)).toThrow("production end window");
    const oversized = window("large");
    oversized.page.messages = Array.from({length: 201}, () => page("x").messages[0]);
    expect(() => new SyntheticTranscript(oversized)).toThrow("200");
  });
  it("does not reparse generated history during unchanged idle reads", () => {
    const file = new SyntheticTranscript(window("original"));
    let replays = 0;
    file.append(page("grown"), () => { replays++; return page("grown"); });
    for (let i = 0; i < 10; i++) expect(file.after(file.size).page.messages).toEqual([]);
    expect(replays).toBe(0);
    expect(file.end().page.messages[0].id).toBe("grown");
    expect(replays).toBe(1);
  });
  it("replays contiguous grown pages from a lagging cursor and reports the actual live edge", () => {
    const file = new SyntheticTranscript(window("original"));
    file.append(page("one"));
    const firstEnd = file.size;
    file.append(page("two"));
    expect(file.after(100)).toMatchObject({ at_end: false, start: { offset: 100 }, end: { offset: firstEnd }, page: { file_bytes: file.size } });
    expect(file.after(firstEnd).page.messages[0].id).toBe("two");
    expect(file.after(file.size).page.messages).toEqual([]);
    expect(() => file.after(150)).toThrow("cursor");
    expect(() => file.after(100, "stale")).toThrow("cursor");
    expect(file.after(100, "end").start.behind_digest).toBe("end");
  });
});
