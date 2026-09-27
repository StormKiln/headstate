/// The colours the transcript tool renderers use (#1483).
///
/// One place, so `palette.test.ts` can hold every foreground to WCAG AA
/// (4.5:1) against the background it is drawn on (#1489). Colour is
/// never the only signal: diff lines carry `+`/`-`, word changes are
/// underlined, errors say "error" in words.
export const palette = {
  ground: "#0d1117",
  surface: "#161b22",
  border: "#30363d",
  text: "#e6edf3",
  muted: "#8b949e",
  error: "#f85149",
  warn: "#d29922",
  ok: "#3fb950",
  link: "#58a6ff",
  addedBg: "#0f2e17",
  addedText: "#7ee787",
  addedWordBg: "#1b4721",
  addedWordText: "#aff5b4",
  removedBg: "#3a1418",
  removedText: "#ff7b72",
  removedWordBg: "#6e1f24",
  removedWordText: "#ffdcd7",
  /// The desktop renderer's (#1480). Claude Code's own accent, the
  /// orange its `>` prompt and bullets wear in a terminal, and the
  /// user turn's band: one step lighter than the ground, so the turn
  /// reads as a separator even before its bar is seen.
  accent: "#d97757",
  userBand: "#1c2128",
} as const;

/// Every foreground/background pair the renderers draw, by name.
export const PAIRS: [fg: keyof typeof palette, bg: keyof typeof palette][] = [
  ["text", "ground"],
  ["text", "surface"],
  ["muted", "ground"],
  ["muted", "surface"],
  ["error", "ground"],
  ["error", "surface"],
  ["warn", "ground"],
  ["ok", "ground"],
  ["link", "ground"],
  ["link", "surface"],
  ["addedText", "addedBg"],
  ["removedText", "removedBg"],
  ["addedWordText", "addedWordBg"],
  ["removedWordText", "removedWordBg"],
  ["muted", "addedBg"],
  ["muted", "removedBg"],
  // The desktop renderer (#1480): the user band, its accent bar and
  // `>` glyph, and the status colours on the surfaces it draws them on.
  ["text", "userBand"],
  ["muted", "userBand"],
  ["accent", "userBand"],
  ["accent", "ground"],
  ["error", "userBand"],
  // A pending message's "not confirmed" (#1491).
  ["warn", "userBand"],
  ["warn", "surface"],
  ["ok", "surface"],
  ["link", "userBand"],
];
