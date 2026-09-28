import { describe, expect, it } from "vitest";
import { DECORATIVE, NON_TEXT, PAIRS, palette } from "./palette";

/// WCAG relative luminance and contrast ratio, as `lib/contrast.test.ts`
/// writes them out.
function luminance(hex: string): number {
  const h = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);
  const f = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}

function ratio(a: string, b: string): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/// #1489: body text, muted metadata, diff and status colours all meet
/// AA on the background each is drawn on.
describe("transcript palette contrast", () => {
  it("meets 4.5:1 on every text pair the renderers draw", () => {
    for (const [fg, bg] of PAIRS) {
      expect(ratio(palette[fg], palette[bg]), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("meets 3:1 on every graphic that carries meaning", () => {
    for (const [fg, bg] of NON_TEXT) {
      expect(ratio(palette[fg], palette[bg]), `${fg} on ${bg}`).toBeGreaterThanOrEqual(3);
    }
  });

  /// The two this pass replaced, recorded so neither comes back.
  it("rejects the colours #1489 replaced", () => {
    expect(ratio("#ffffff", "#388bfd"), "white on the old hover blue").toBeLessThan(4.5);
    expect(ratio("#8b949e", "#30363d"), "the old masked pill").toBeLessThan(4.5);
  });
});

// ---------------------------------------------------------------------
// The source scan: every colour the transcript USES is one checked here.
// ---------------------------------------------------------------------

/// The transcript's production source, and the two shared components
/// it draws all its text through. `import.meta.glob`, as
/// `emptyStateGuard.test.ts` argues: a list of imports is the
/// enumeration that lets the next file go unchecked.
const globbed = import.meta.glob(
  ["./**/*.ts", "./**/*.tsx", "../TranscriptMarkdown.tsx", "../MaskedText.tsx"],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

/// Tests and test helpers draw nothing; this file's own palette is the
/// definition, not a use.
const sources: Record<string, string> = Object.fromEntries(
  Object.entries(globbed).filter(
    ([path]) =>
      !path.includes(".test.") &&
      !path.endsWith("/palette.ts") &&
      !path.endsWith("/fixtures.ts") &&
      !path.endsWith("/scrollShim.ts"),
  ),
);

/// Code only: `///` and `//` comments cite colours and issue numbers.
function code(src: string): string {
  return src
    .split("\n")
    .filter((l) => !/^\s*\/\//.test(l))
    .join("\n");
}

/// A hex colour where it is used: a Tailwind arbitrary value
/// (`text-[#8b949e]`) or a string (`"#8b949e"`).
const HEX_USE = /(?<=\[|["'`])#[0-9a-fA-F]{6}\b/g;
/// Tailwind's own colour scales and named colours: unchecked by design.
const NAMED =
  /\b(?:text|bg|border|fill|stroke|decoration|outline|ring|divide|from|to|via)-(?:white|black|(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3})\b/g;
/// A faded colour at rest: its contrast is whatever the blend gives.
/// A class, an alpha on an arbitrary colour, or a style. Variants
/// (`disabled:opacity-50`, `hover:opacity-100`) are not at rest --
/// WCAG exempts a disabled control, and fading is how it says so.
const FADED = /(?<![\w:-])opacity-\d+|\[#[0-9a-fA-F]{6}\]\/\d+|\bopacity:\s*0?\.\d/g;

function matches(re: RegExp): [file: string, hit: string][] {
  return Object.entries(sources).flatMap(([file, src]) =>
    [...code(src).matchAll(re)].map((m) => [file, m[0]] as [string, string]),
  );
}

const byValue = new Map<string, keyof typeof palette>(
  Object.entries(palette).map(([k, v]) => [v.toLowerCase(), k as keyof typeof palette]),
);
const checked = new Set<string>([
  ...PAIRS.flat(),
  ...NON_TEXT.flat(),
  ...DECORATIVE,
]);

describe("the transcript uses only checked colours", () => {
  it("scans the files it means to", () => {
    // Self-check: a glob that quietly matched nothing would pass all
    // of the below.
    for (const f of [
      "./TerminalMessage.tsx",
      "./phone/PhoneMessage.tsx",
      "./TranscriptHeader.tsx",
      "./navigation.tsx",
      "../TranscriptMarkdown.tsx",
      "../MaskedText.tsx",
    ]) {
      expect(Object.keys(sources), f).toContain(f);
    }
    expect(matches(HEX_USE).length).toBeGreaterThan(20);
  });

  it("draws every hex colour from the palette", () => {
    const stray = matches(HEX_USE).filter(([, hex]) => !byValue.has(hex.toLowerCase()));
    expect(stray, "add the colour to palette.ts, with its pairs").toEqual([]);
  });

  it("checks the contrast of every palette colour it uses", () => {
    const used = new Set<string>();
    for (const [, hex] of matches(HEX_USE)) {
      const token = byValue.get(hex.toLowerCase());
      if (token) used.add(token);
    }
    for (const [, ref] of matches(/\bpalette\.([a-zA-Z]+)\b/g)) used.add(ref.slice("palette.".length));
    for (const [, ref] of matches(/\bpalette\[\s*"([a-zA-Z]+)"\s*\]/g)) used.add(ref);
    const unchecked = [...used].filter((t) => !checked.has(t));
    expect(unchecked, "add each to PAIRS, NON_TEXT or DECORATIVE in palette.ts").toEqual([]);
  });

  it("names no Tailwind colour outside the palette", () => {
    expect(matches(NAMED)).toEqual([]);
  });

  it("fades no colour with opacity", () => {
    expect(matches(FADED)).toEqual([]);
  });
});
