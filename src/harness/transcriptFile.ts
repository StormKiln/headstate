import type { PageCursor, TranscriptPage, TranscriptWindow } from "../types/transcript";

type StoredPage = { start: number; end: number; startCursor: PageCursor; endCursor: PageCursor; read: () => TranscriptWindow };

/// The browser harness retains only replay recipes for generated chunks,
/// so its file history does not keep the viewer's evicted messages alive.
export class SyntheticTranscript {
  private pages: StoredPage[] = [];
  size: number;
  private readonly emptyPage: string;

  constructor(base: TranscriptWindow) {
    if (!base.page || !base.at_end || base.page.messages.length > 200) {
      throw new Error("harness: expected a production end window (at most 200 messages)");
    }
    this.size = base.page.file_bytes;
    this.emptyPage = JSON.stringify({ ...base.page, messages: [], bytes_read: 0 });
    const raw = JSON.stringify(base);
    this.pages.push({ start: base.start.offset, end: base.end.offset, startCursor: base.start, endCursor: base.end, read: () => JSON.parse(raw) });
  }

  append(page: TranscriptPage, replay?: () => TranscriptPage): void {
    const start = this.pages[this.pages.length - 1].end;
    // Generated growth has a synthetic JSON span, never the reader's I/O cost.
    const bytes = new TextEncoder().encode(JSON.stringify(page.messages)).length;
    this.size += bytes;
    // Small test fixtures can supply pages directly; the browser supplies
    // a replay recipe closing over only generation/turn/size parameters.
    const raw = replay ? null : JSON.stringify(page);
    const read = replay ?? (() => JSON.parse(raw!));
    const previousEnd = this.pages[this.pages.length - 1].endCursor;
    const end = this.size;
    const endCursor = { offset: end, behind_digest: `harness-growth-${end}` };
    this.pages.push({ start, end, startCursor: previousEnd, endCursor, read: () => ({
      page: { ...read(), file_bytes: end, bytes_read: bytes },
      start: previousEnd,
      end: endCursor,
      at_start: false, at_end: true, rewritten: false,
      position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
      seam: { first_model: null, last_model: null }, bytes_scanned: 0,
    }) });
  }

  end(): TranscriptWindow {
    return this.window(this.pages[this.pages.length - 1]);
  }

  after(offset: number, digest?: string): TranscriptWindow {
    const last = this.pages[this.pages.length - 1];
    const page = this.pages.find((p) => p.start === offset);
    const cursor = offset === last.end ? last.endCursor : page?.startCursor;
    if (!cursor || (digest !== undefined && cursor.behind_digest !== digest)) {
      throw new Error(`harness: unknown or stale cursor ${offset}`);
    }
    if (offset === last.end) {
      return { page: { ...JSON.parse(this.emptyPage), file_bytes: this.size }, rewritten: false,
        start: cursor, end: cursor, at_start: offset === 0, at_end: true,
        bytes_scanned: 0, position: {first: null, last: null, total: null, exact: false, basis: "bytes"},
        seam: {first_model: null, last_model: null} };
    }
    return this.window(page!);
  }

  private window(p: StoredPage): TranscriptWindow {
    const result = p.read();
    return { ...result, page: { ...result.page, file_bytes: this.size }, at_end: p === this.pages[this.pages.length - 1] };
  }
}
