import type { Refractor, Syntax } from "refractor/core";
import type { GrammarName } from "./highlightLangs";

/// The highlighting itself: grammar loaders and one job. Pure -- no DOM,
/// no worker plumbing -- so the worker (`highlight.worker.ts`) and the
/// tests run exactly this code. `highlight.ts` explains the design.
///
/// Only the worker imports this in the app. Every loader below is a
/// dynamic import, so each grammar is its own chunk, fetched the first
/// time a block needs it.

/// One loader per grammar name; a name without one does not compile.
const GRAMMARS: Record<GrammarName, () => Promise<{ default: Syntax }>> = {
  bash: () => import("refractor/bash"),
  c: () => import("refractor/c"),
  cpp: () => import("refractor/cpp"),
  csharp: () => import("refractor/csharp"),
  css: () => import("refractor/css"),
  diff: () => import("refractor/diff"),
  docker: () => import("refractor/docker"),
  go: () => import("refractor/go"),
  graphql: () => import("refractor/graphql"),
  ini: () => import("refractor/ini"),
  java: () => import("refractor/java"),
  javascript: () => import("refractor/javascript"),
  json: () => import("refractor/json"),
  jsx: () => import("refractor/jsx"),
  kotlin: () => import("refractor/kotlin"),
  makefile: () => import("refractor/makefile"),
  markdown: () => import("refractor/markdown"),
  markup: () => import("refractor/markup"),
  python: () => import("refractor/python"),
  ruby: () => import("refractor/ruby"),
  rust: () => import("refractor/rust"),
  sql: () => import("refractor/sql"),
  swift: () => import("refractor/swift"),
  toml: () => import("refractor/toml"),
  tsx: () => import("refractor/tsx"),
  typescript: () => import("refractor/typescript"),
  yaml: () => import("refractor/yaml"),
};

export type Tree = ReturnType<Refractor["highlight"]>;

let core: Promise<Refractor> | null = null;
const grammars = new Map<GrammarName, Promise<void>>();

function loadCore(): Promise<Refractor> {
  core ??= import("refractor/core").then((m) => m.refractor);
  return core;
}

function loadGrammar(name: GrammarName): Promise<void> {
  let p = grammars.get(name);
  if (!p) {
    const loader = GRAMMARS[name];
    p = Promise.all([loadCore(), loader()]).then(([r, g]) => {
      r.register(g.default);
    });
    // A failed chunk load is not cached: the next block to ask retries.
    p.catch(() => grammars.delete(name));
    grammars.set(name, p);
  }
  return p;
}

/// Grammars requested so far in this context. Exposed for the test that
/// proves loading is on demand; nothing else should care.
export function loadedGrammars(): GrammarName[] {
  return [...grammars.keys()];
}

/// Highlights one block, loading its grammar first if need be.
export async function runJob(code: string, grammar: GrammarName): Promise<Tree> {
  await loadGrammar(grammar);
  const r = await loadCore();
  return r.highlight(code, grammar);
}
