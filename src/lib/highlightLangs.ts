/// What the main thread needs to know about highlighting: which grammars
/// exist and which blocks are too big. No grammar is imported here --
/// the loaders live in `highlightCore.ts`, which only the worker (and the
/// tests) import, so the main bundle carries none of them.

/// Above this many lines a block renders as plain monospace.
export const MAX_LINES = 1_000;
/// Above this many characters a block renders as plain monospace.
export const MAX_CHARS = 60_000;

/// Grammars a block can be highlighted with, by refractor's name. A
/// deliberate subset: what a Claude Code session actually prints. A
/// fence tagged with anything else renders plain, labelled with its tag.
/// `highlightCore.ts` has one loader per name, which the type enforces.
const GRAMMAR_NAMES = [
  "bash",
  "c",
  "cpp",
  "csharp",
  "css",
  "diff",
  "docker",
  "go",
  "graphql",
  "ini",
  "java",
  "javascript",
  "json",
  "jsx",
  "kotlin",
  "makefile",
  "markdown",
  "markup",
  "python",
  "ruby",
  "rust",
  "sql",
  "swift",
  "toml",
  "tsx",
  "typescript",
  "yaml",
] as const;

export type GrammarName = (typeof GRAMMAR_NAMES)[number];

/// Fence tags people actually write, mapped to a grammar above.
const ALIASES: Record<string, GrammarName> = {
  "c++": "cpp",
  cjs: "javascript",
  cs: "csharp",
  dockerfile: "docker",
  golang: "go",
  htm: "markup",
  html: "markup",
  js: "javascript",
  jsonc: "json",
  kt: "kotlin",
  md: "markdown",
  mjs: "javascript",
  mts: "typescript",
  patch: "diff",
  py: "python",
  rb: "ruby",
  rs: "rust",
  sh: "bash",
  shell: "bash",
  svg: "markup",
  ts: "typescript",
  xml: "markup",
  yml: "yaml",
  zsh: "bash",
};

/// The grammar a fence tag names, or null when there is none to load.
export function grammarFor(tag: string | undefined): GrammarName | null {
  if (!tag) return null;
  const t = tag.toLowerCase();
  // Own keys only: an inherited name such as `constructor` is not a tag.
  // (Not `Object.hasOwn`: the build targets Safari 15, which lacks it.)
  if (Object.prototype.hasOwnProperty.call(ALIASES, t)) return ALIASES[t];
  return (GRAMMAR_NAMES as readonly string[]).includes(t) ? (t as GrammarName) : null;
}

/// Why a block is not highlighted, when it is too big to be.
export function oversize(code: string): "lines" | "chars" | null {
  if (code.length > MAX_CHARS) return "chars";
  return countLines(code) > MAX_LINES ? "lines" : null;
}

/// Lines in `code`, counted without splitting: a 60,000-character block
/// split into an array only to be measured would allocate the very cost
/// being capped.
export function countLines(code: string): number {
  let lines = 1;
  for (let i = code.indexOf("\n"); i !== -1; i = code.indexOf("\n", i + 1)) lines++;
  return lines;
}
