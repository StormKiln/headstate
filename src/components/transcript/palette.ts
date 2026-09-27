/// The colours the transcript draws (#1483, #1489).
///
/// One place, so `palette.test.ts` can hold every foreground to WCAG AA
/// (4.5:1) against every background it is drawn on. The app is dark
/// only (`<html class="dark">`), so these are the only values.
///
/// **Every colour the transcript's source uses is here.** A hex literal
/// in a Tailwind class (`text-[#8b949e]`) must be one of these values,
/// and a token used in a style (`palette.muted`) must appear in `PAIRS`
/// or `NON_TEXT`; `palette.test.ts` scans the source for both, so a new
/// colour cannot ship unchecked.
///
/// Colour is never the only signal: diff lines carry `+`/`-`, word
/// changes are underlined, errors say "error" in words, a user turn has
/// its band, bar, `>` glyph and "You".
export const palette = {
  // Grounds.
  ground: "#0d1117",
  surface: "#161b22",
  /// A control's resting fill, a hovered row, a phone bubble, a pill.
  raised: "#21262d",
  border: "#30363d",
  // Text.
  text: "#e6edf3",
  muted: "#8b949e",
  error: "#f85149",
  warn: "#d29922",
  /// `warn` at 15% over `surface`: the "waiting for you" chip's fill,
  /// written out so its contrast is checked rather than blended at
  /// runtime.
  warnTint: "#322e22",
  ok: "#3fb950",
  link: "#58a6ff",
  /// Links inside rendered markdown, GitHub's own.
  markdownLink: "#4493f8",
  // The one solid button.
  primary: "#1f6feb",
  /// Darker than `primary`, not lighter: `#388bfd` carried white at
  /// 3.34:1 (the app's #693 made the same trade).
  primaryHover: "#316dca",
  onPrimary: "#ffffff",
  // Diffs.
  addedBg: "#0f2e17",
  addedText: "#7ee787",
  addedWordBg: "#1b4721",
  addedWordText: "#aff5b4",
  removedBg: "#3a1418",
  removedText: "#ff7b72",
  removedWordBg: "#6e1f24",
  removedWordText: "#ffdcd7",
  // Code highlighting (`TranscriptMarkdown`), on `surface`.
  syntaxString: "#a5d6ff",
  syntaxFunction: "#d2a8ff",
  syntaxClass: "#ffa657",
  syntaxConstant: "#79c0ff",
  /// The desktop renderer's (#1480). Claude Code's own accent, the
  /// orange its `>` prompt and bullets wear in a terminal, and the
  /// user turn's band: one step lighter than the ground, so the turn
  /// reads as a separator even before its bar is seen.
  accent: "#d97757",
  userBand: "#1c2128",
} as const;

type Token = keyof typeof palette;

/// Every text colour on every background it is drawn on, held to 4.5:1.
export const PAIRS: [fg: Token, bg: Token][] = [
  ["text", "ground"],
  ["text", "surface"],
  ["text", "raised"],
  ["muted", "ground"],
  ["muted", "surface"],
  ["muted", "raised"],
  ["error", "ground"],
  ["error", "surface"],
  ["error", "raised"],
  ["warn", "ground"],
  ["warn", "warnTint"],
  ["ok", "ground"],
  ["link", "ground"],
  ["link", "surface"],
  ["link", "raised"],
  ["markdownLink", "ground"],
  ["markdownLink", "surface"],
  ["markdownLink", "raised"],
  ["onPrimary", "primary"],
  ["onPrimary", "primaryHover"],
  ["addedText", "addedBg"],
  ["removedText", "removedBg"],
  ["addedWordText", "addedWordBg"],
  ["removedWordText", "removedWordBg"],
  ["muted", "addedBg"],
  ["muted", "removedBg"],
  ["text", "addedBg"],
  ["text", "removedBg"],
  // Highlighted code sits on `surface` in a code block.
  ["syntaxString", "surface"],
  ["syntaxFunction", "surface"],
  ["syntaxClass", "surface"],
  ["syntaxConstant", "surface"],
  ["addedText", "surface"],
  ["removedText", "surface"],
  ["ok", "surface"],
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
  ["link", "userBand"],
];

/// Colours that are not text, held to the 3:1 WCAG 1.4.11 asks of a
/// graphic that carries meaning: the user turn's bar, the unread
/// divider's rule, a dashed "could not tell" dot.
export const NON_TEXT: [fg: Token, bg: Token][] = [
  ["accent", "userBand"],
  ["accent", "ground"],
  ["muted", "surface"],
];

/// Colours that separate but carry no meaning of their own, so no
/// contrast is required of them: a rule between items, a control's
/// outline beside its label, which says what it is in words. Named so
/// the source scan can tell "decorative" from "forgot to check".
export const DECORATIVE: Token[] = ["border"];
