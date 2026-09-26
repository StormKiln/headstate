import { describe, expect, it } from "vitest";
// `?raw` rather than `node:fs`: the project carries no `@types/node`
// (see mirroredConstants.test.ts).
import privacyRs from "../../src-tauri/src/remote/privacy.rs?raw";
import {
  MARKER_CLOSE,
  MARKER_OPEN,
  MASK_KINDS,
  MASK_LABELS,
  maskingNote,
  splitMasked,
} from "./masked";

describe("splitMasked", () => {
  it("returns text without a marker whole", () => {
    expect(splitMasked("nothing here")).toEqual([{ text: "nothing here" }]);
    expect(splitMasked("")).toEqual([{ text: "" }]);
  });

  it("splits each marker into a hidden part, keeping the text around it", () => {
    expect(splitMasked("API_KEY=⟦hidden:secret⟧ and ⟦hidden:github-token⟧!")).toEqual([
      { text: "API_KEY=" },
      { hidden: "secret" },
      { text: " and " },
      { hidden: "github-token" },
      { text: "!" },
    ]);
  });

  it("keeps a marker of an unknown kind as text rather than dropping it", () => {
    expect(splitMasked("⟦hidden:future-kind⟧")).toEqual([{ text: "⟦hidden:future-kind⟧" }]);
  });

  it("states the response's hidden count, and nothing when nothing was hidden", () => {
    const m = { hidden: 2, revealed: false, reveal_allowed: false, withheld: false };
    expect(maskingNote(m)).toBe(
      "2 likely secrets were hidden before this text left the computer.",
    );
    expect(maskingNote({ ...m, hidden: 1 })).toContain("1 likely secret was");
    expect(maskingNote({ ...m, hidden: 0 })).toBeNull();
    expect(maskingNote({ ...m, revealed: true })).toBeNull();
    expect(maskingNote(undefined)).toBeNull();
  });

  it("labels every kind", () => {
    for (const kind of MASK_KINDS) expect(MASK_LABELS[kind]).toBeTruthy();
  });
});

/// The marker and its kinds exist in Rust (which writes them) and here
/// (which reads them). Read from the Rust source so changing either side
/// alone fails -- the #850 rule for mirrored constants.
describe("the marker is mirrored in remote/privacy.rs", () => {
  it("agrees on the marker's delimiters", () => {
    expect(privacyRs).toContain('pub const MARKER_OPEN: &str = "\\u{27e6}hidden:";');
    expect(privacyRs).toContain('pub const MARKER_CLOSE: &str = "\\u{27e7}";');
    expect(MARKER_OPEN).toBe("⟦hidden:");
    expect(MARKER_CLOSE).toBe("⟧");
  });

  it("agrees on the kinds", () => {
    const block = privacyRs.split("pub const KINDS: &[&str] = &[")[1]?.split("];")[0] ?? "";
    const rust = [...block.matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
    expect(rust.length, "KINDS must be readable from privacy.rs").toBeGreaterThan(0);
    expect(rust).toEqual([...MASK_KINDS]);
  });
});
