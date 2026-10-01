import type { TranscriptPage, TranscriptWindow } from "../types/transcript";

type StoredPage = { start: number; end: number; read: () => TranscriptPage };

/// The browser harness retains only replay recipes for generated chunks,
/// so its file history does not keep the viewer's evicted messages alive.
export class SyntheticTranscript {
  private pages: StoredPage[] = [];
  size: number;

  constructor(private readonly base: TranscriptPage) {
    // Whole parser fixtures have no filesystem metadata (both sizes are
    // zero). Give them a synthetic byte span before assigning cursors.
    const bytes = base.bytes_read || JSON.stringify(base.messages).length;
    this.size = Math.max(base.file_bytes, bytes);
    const raw = JSON.stringify({ ...base, bytes_read: bytes });
    this.pages.push({ start: this.size - bytes, end: this.size, read: () => JSON.parse(raw) });
  }

  append(page: TranscriptPage, replay?: () => TranscriptPage): void {
    const start = this.size;
    const bytes = page.bytes_read || JSON.stringify(page.messages).length;
    this.size += bytes;
    // Small test fixtures can supply pages directly; the browser supplies
    // a replay recipe closing over only generation/turn/size parameters.
    const raw = replay ? null : JSON.stringify(page);
    this.pages.push({ start, end: this.size, read: replay ?? (() => JSON.parse(raw!)) });
  }

  end(): TranscriptWindow {
    return this.window(this.pages[this.pages.length - 1]);
  }

  after(offset: number): TranscriptWindow {
    if (offset === this.size) return this.window({ start: offset, end: offset, read: () => ({ ...this.base, messages: [], bytes_read: 0 }) });
    const page = this.pages.find((p) => p.start === offset);
    if (!page) throw new Error(`harness: unknown cursor ${offset}`);
    return this.window(page);
  }

  private window(p: StoredPage): TranscriptWindow {
    return {
      page: { ...p.read(), file_bytes: this.size, bytes_read: p.end - p.start },
      start: { offset: p.start, behind_digest: "harness" },
      end: { offset: p.end, behind_digest: "harness" },
      at_start: p.start === 0,
      at_end: p.end === this.size,
      rewritten: false,
      position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
      seam: { first_model: null, last_model: null },
      bytes_scanned: 0,
    };
  }
}
