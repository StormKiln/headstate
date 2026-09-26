import { describe, expect, it } from "vitest";
import { PAIRS, palette } from "./palette";

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
describe("transcript tool palette contrast", () => {
  it("meets 4.5:1 on every pair the renderers draw", () => {
    for (const [fg, bg] of PAIRS) {
      expect(ratio(palette[fg], palette[bg]), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5);
    }
  });
});
