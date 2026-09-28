import { describe, expect, it } from "vitest";

import type {
  TranscriptMessage,
  TranscriptPosition,
  TranscriptWindow,
} from "../types/transcript";
import golden from "./transcriptPages.golden.json";
import { mergeWindows, positionLabel } from "./transcriptPages";

/// Written by `transcript_page.rs` (`the_merge_golden_file_is_current`),
/// which fails when it goes stale: pages of one generated conversation at
/// several page sizes, and one read of the whole file.
const G = golden as unknown as {
  whole: TranscriptMessage[];
  cases: { limit: number; windows: TranscriptWindow[] }[];
};

describe("mergeWindows", () => {
  /// The cross-language proof: pages the Rust side read separately,
  /// merged here, are the Rust side's single read -- pairing, turns and
  /// model changes included.
  it.each(G.cases.map((c) => [c.limit, c] as const))(
    "pages of at most %i messages merge into the single read",
    (_limit, c) => {
      expect(mergeWindows(c.windows)).toEqual(G.whole);
    },
  );

  /// Guards the test above: were the pages already equal to the single
  /// read when laid end to end, it would pass with a merge that did
  /// nothing. At one message a page, every seam is split.
  it("has seams to merge: concatenation alone is not the single read", () => {
    const smallest = G.cases.find((c) => c.limit === 1);
    expect(smallest).toBeDefined();
    const flat = smallest!.windows.flatMap((w) => w.page.messages);
    expect(flat).not.toEqual(G.whole);
    // Each of the three seam effects is exercised.
    const standing = flat.filter((m) => m.kind.kind === "tool_results").length;
    const unturned = flat.filter((m) => m.turn_id === null).length;
    const changes = G.whole.filter((m) => m.kind.kind === "model_change").length;
    expect(standing).toBeGreaterThan(0);
    expect(unturned).toBeGreaterThan(1);
    expect(changes).toBeGreaterThan(0);
  });

  it("refuses pages that do not tile", () => {
    const c = G.cases.find((x) => x.windows.length >= 3)!;
    expect(() => mergeWindows([c.windows[0], c.windows[2]])).toThrow(/do not tile/);
  });

  it("does not mutate the pages it is given", () => {
    const c = G.cases.find((x) => x.limit === 1)!;
    const before = JSON.stringify(c.windows);
    mergeWindows(c.windows);
    expect(JSON.stringify(c.windows)).toBe(before);
  });

  /// A record written twice (a resumed session re-recording its head)
  /// can land in two pages. The first occurrence wins, as in a single
  /// read -- and a result the second one's page had absorbed is its own
  /// record, so it stands again rather than vanishing.
  it("keeps the first of a duplicated record and loses none of its results", () => {
    const c = G.cases.find((x) => x.limit === 200)!;
    const [page] = c.windows;
    const call = page.page.messages.find((m) =>
      m.blocks.some((b) => b.kind === "tool_call" && b.result !== null),
    )!;
    // A second page holding only a re-recorded copy of that call, which
    // absorbed a result the first page never saw.
    const copy: TranscriptMessage = {
      ...call,
      blocks: call.blocks.map((b) =>
        b.kind === "tool_call" && b.result !== null
          ? { ...b, result: { ...b.result, message_id: "late-result", oversized_bytes: 900_000 } }
          : b,
      ),
    };
    const second: TranscriptWindow = {
      ...page,
      start: page.end,
      end: { offset: page.end.offset + 100, behind_digest: "x" },
      seam: { first_model: null, last_model: null },
      page: { ...page.page, messages: [copy] },
    };
    const merged = mergeWindows([page, second]);
    expect(merged.filter((m) => m.id === call.id)).toHaveLength(1);
    const late = merged.find((m) => m.id === "late-result");
    expect(late?.kind.kind).toBe("tool_results");
    // Standing again, it is still the oversized record it was (#1476).
    expect(late?.oversized_bytes).toBe(900_000);
  });

  /// #1476: a result too large for its page, paired into its call across
  /// pages, keeps saying so -- its own message is folded away.
  it("carries an oversized result's size onto the call it merges into", () => {
    const c = G.cases.find((x) => x.limit === 1)!;
    const at = c.windows.findIndex((w) =>
      w.page.messages.some((m) => m.blocks.some((b) => b.kind === "tool_result")),
    );
    expect(at).toBeGreaterThan(0);
    const windows = c.windows.map((w, i) =>
      i !== at
        ? w
        : {
            ...w,
            page: {
              ...w.page,
              messages: w.page.messages.map((m) => ({
                ...m,
                blocks: m.blocks.map((b) =>
                  b.kind === "tool_result" ? { ...b, oversized_bytes: 700_000 } : b,
                ),
              })),
            },
          },
    );
    const results = mergeWindows(windows).flatMap((m) =>
      m.blocks.flatMap((b) => (b.kind === "tool_call" && b.result ? [b.result] : [])),
    );
    expect(results.some((r) => r.oversized_bytes === 700_000)).toBe(true);
  });
});

describe("positionLabel", () => {
  const pos = (p: Partial<TranscriptPosition>): TranscriptPosition => ({
    first: 1,
    last: 1,
    total: 1,
    exact: false,
    basis: "bytes",
    ...p,
  });
  const n = (v: number) => v.toLocaleString();

  it("labels an estimate as one", () => {
    expect(positionLabel(pos({ first: 4200, last: 4400, total: 16700 }))).toBe(
      `messages ~${n(4200)}–${n(4400)} of ~${n(16700)} (estimate)`,
    );
  });

  it("states a count plainly only when it is exact", () => {
    expect(
      positionLabel(pos({ first: 1, last: 120, total: 120, exact: true, basis: "whole_file" })),
    ).toBe("messages 1–120 of 120");
  });

  it("says one message in the singular", () => {
    expect(positionLabel(pos({ first: 7, last: 7, total: 9 }))).toBe(
      "message ~7 of ~9 (estimate)",
    );
  });

  /// Absent is not zero: a page with nothing in it is not "message 0",
  /// and a total that could not be estimated is left out.
  it("never shows an absent figure as zero", () => {
    expect(positionLabel(pos({ first: null, last: null, total: null }))).toBe("no messages here");
    expect(positionLabel(pos({ first: null, last: null, total: 40 }))).toBe(
      "no messages here, of ~40 (estimate)",
    );
    expect(positionLabel(pos({ first: 3, last: 5, total: null }))).toBe(
      "messages ~3–5 (estimate)",
    );
  });
});
