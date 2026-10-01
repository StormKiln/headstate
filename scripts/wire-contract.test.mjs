import { test } from "node:test";
import assert from "node:assert/strict";
import ts from "typescript";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { schemaBuilder, generate, checkGenerated, surfaces } from "./wire-contract.mjs";
import { validateWire } from "../src/api/wireValidator.ts";

function fixture(source, imported = "export type Imported = { imported: string };") {
  const dir = mkdtempSync(join(tmpdir(), "headstate-wire-"));
  try {
    writeFileSync(join(dir, "main.ts"), source);
    writeFileSync(join(dir, "imported.ts"), imported);
    const program = ts.createProgram([join(dir, "main.ts")], { strict: true, noEmit: true, skipLibCheck: true });
    const checker = program.getTypeChecker();
    const builder = schemaBuilder(checker);
    const roots = {};
    for (const node of program.getSourceFile(join(dir, "main.ts")).statements) {
      if (ts.isTypeAliasDeclaration(node) || ts.isInterfaceDeclaration(node)) roots[node.name.text] = builder.ref(checker.getTypeAtLocation(node));
    }
    return { nodes: builder.finish(), roots };
  } finally { rmSync(dir, { recursive: true, force: true }); }
}
const valid = (graph, name, value) => validateWire(graph.nodes, graph.roots[name], value).field === null;

test("compiler resolves imports, intersections, readonly arrays, optional tuples, dictionaries and recursive aliases", () => {
  const graph = fixture(`
    import type { Imported } from './imported';
    type Item = Imported & { tag: 'a'; count?: number | null };
    type Items = readonly Item[];
    type Tuple = [string, number?];
    type Dict = Record<string, Item>;
    type Recursive = { value: string; next?: Recursive | null };
    type Anything = unknown;
    type Ack = void;
  `);
  assert(valid(graph, "Items", [{ imported: "yes", tag: "a", extra: true }]));
  assert(!valid(graph, "Items", [{ imported: "yes", tag: "a", count: "wrong" }]));
  assert(valid(graph, "Tuple", ["ok"]));
  assert(valid(graph, "Tuple", ["ok", 1]));
  assert(!valid(graph, "Tuple", ["ok", 1, 2]));
  assert(!valid(graph, "Tuple", ["ok", "no"]));
  assert(valid(graph, "Dict", { arbitrary: { imported: "yes", tag: "a" } }));
  const failure = validateWire(graph.nodes, graph.roots.Dict, { "secret/key": { imported: 2, tag: "a" } });
  assert.match(failure.field, /\[entry\]\.imported/);
  assert(!failure.field.includes("secret"));
  assert(valid(graph, "Recursive", { value: "yes", next: { value: "next", next: null } }));
  const cycle = { value: "yes" }; cycle.next = cycle;
  assert(!valid(graph, "Recursive", cycle));
  assert(valid(graph, "Anything", { value: [1, null, "ok"] }));
  assert(!valid(graph, "Anything", cycle));
  assert(!valid(graph, "Anything", NaN));
  assert(valid(graph, "Ack", null));
  assert(valid(graph, "Ack", undefined));
  assert(!valid(graph, "Ack", {}));
});

test("literal discriminants dispatch without inspecting unrelated alternatives", () => {
  const graph = fixture(`type Result = { kind:'first'; value:string } | { kind:'second'; value:number }`);
  const union = graph.nodes[graph.roots.Result];
  assert.equal(union.tag, "kind");
  assert(valid(graph, "Result", { kind: "second", value: 5 }));
  assert(!valid(graph, "Result", { kind: "second", value: "5" }));
  assert(!valid(graph, "Result", { kind: "future", value: 5 }));
});

test("unsupported/unresolved types fail generation, never become permissive", () => {
  for (const source of [
    "type Bad = any", "type Bad = () => string", "type Bad = bigint", "type Bad = {}",
    "type Bad<T> = T", "type Bad = [string, ...number[]]",
    "type Bad = { [key: number]: string }", "type Bad = MissingImport",
    "type Bad = string & { brand: 'id' }",
  ]) assert.throws(() => fixture(source), /Unsupported/);
});

test("recursive ambiguous schemas cache failed subtrees instead of exponential traversal", () => {
  const graph = fixture(`type Bomb = { next: Bomb; a: string } | { next: Bomb; b: number };`);
  let value = {};
  // The alternatives share the same child but neither can succeed. Without
  // a global budget this produces exponential work before reaching the leaf.
  for (let i = 0; i < 35; i++) value = { next: value };
  const result = validateWire(graph.nodes, graph.roots.Bomb, value);
  assert.notEqual(result.field, null);
  assert(result.operations < 1000);
});

test("operation limits are shared across union alternatives and array items", () => {
  const nodes = [
    { t: "array", item: 1 },
    { t: "union", of: Array.from({ length: 64 }, (_, i) => i + 2) },
    ...Array.from({ length: 64 }, (_, i) => ({ t: "literal", v: i })),
  ];
  const result = validateWire(nodes, 0, Array(160_000).fill(63));
  assert.match(result.field, /too complex/);
  assert(result.operations <= 10_000_010);
});

test("bounded recursion rejects over-deep data without throwing", () => {
  const graph = fixture(`type Deep = { next?: Deep; value: number }`);
  let value = { value: 1 };
  for (let i = 0; i < 150; i++) value = { value: 1, next: value };
  assert.match(validateWire(graph.nodes, graph.roots.Deep, value).field, /too complex/);
});

const root = resolve(import.meta.dirname, "..");
const generatedPath = resolve(root, "src/api/wireContract.generated.ts");
const read = path => readFileSync(resolve(root, path), "utf8");

test("generated schema covers both remote surfaces/events and is fresh", () => {
  const output = generate(root);
  checkGenerated(output, readFileSync(generatedPath, "utf8"));
  const graph = JSON.parse(output.slice(output.indexOf("= ") + 2, -2));
  const wanted = surfaces(root);
  assert.deepEqual(Object.keys(graph.commands).sort(), wanted.commands);
  assert.deepEqual(Object.keys(graph.events).sort(), wanted.events);
  assert(graph.commands.claude_poll_live !== undefined);
  assert(!Object.hasOwn(graph.commands, "connection_state"));
  assert(!Object.hasOwn(graph.commands, "save_markdown"));
  assert(wanted.commands.length > 140 && wanted.events.length >= 19);
});

test("reachable nested type and wrapper sabotage invalidate generated output", () => {
  const actual = readFileSync(generatedPath, "utf8");
  for (const [path, before, after] of [
    ["src/types/pr.ts", "export interface PullRequest extends PrIdentity {", "export interface PullRequest extends PrIdentity { sabotage_required: string;"],
    ["src/api/tauri.ts", 'call<History>("get_history"', 'call<string>("get_history"'],
  ]) {
    const original = read(path);
    assert(original.includes(before));
    const output = generate(root, { [resolve(root, path)]: original.replace(before, after) });
    assert.throws(() => checkGenerated(output, actual), /stale/);
  }
});

test("new remote commands/events and class-table drift fail closed", () => {
  for (const [paths, marker, row, error] of [
    [["src-tauri/src/remote/surface.rs", "src-mobile/src/surface.rs"], '    ("get_cached", Class::Read),', '    ("new_uncovered_command", Class::Read),', /Uncovered remote command/],
    [["src-tauri/src/remote/events.rs", "src-mobile/src/events.rs"], '    "prs-updated",', '    "new-uncovered-event",', /Uncovered remote event/],
  ]) {
    const overrides = {};
    for (const path of paths) {
      assert(read(path).includes(marker));
      overrides[resolve(root, path)] = read(path).replace(marker, marker + "\n" + row);
    }
    assert.throws(() => generate(root, overrides), error);
    delete overrides[resolve(root, paths[1])];
    assert.throws(() => generate(root, overrides), /differ|mismatched/);
  }
});

test("actual CI lint and frontend jobs invoke freshness, not only local Make", () => {
  const workflow = read(".github/workflows/ci.yml");
  const frontend = workflow.slice(workflow.indexOf("  test-frontend:"), workflow.indexOf("  test-frontend:") + 3000);
  assert.match(frontend, /run: make check-wire-contract[\s\S]*run: bash scripts\/test-frontend-ci.sh/);
  const lint = workflow.slice(0, workflow.indexOf("  test-rust:"));
  assert.match(lint, /run: make check-wire-contract[\s\S]*run: yarn tsc/);
  const make = read("Makefile");
  assert.match(make, /test-ui: check-wire-contract/);
  assert.match(make, /lint-ui: check-wire-contract/);
  assert.match(make, /node scripts\/wire-contract.mjs --check/);
});


test("a client-local command cannot acquire a remote schema", () => {
  const path = "src/api/remote.ts";
  const overrides = { [resolve(root, path)]: read(path).replace('  "pair_from_qr",', '  "pair_from_qr",\n  "get_cached",') };
  assert.throws(() => generate(root, overrides), /both remote and client-local/);
});


test("Rust table parsing accepts multiline tuples and refuses unparsed rows", () => {
  const paths = ["src-tauri/src/remote/surface.rs", "src-mobile/src/surface.rs"];
  const before = '    ("get_cached", Class::Read),';
  const multiline = {}; const invalid = {};
  for (const path of paths) {
    multiline[resolve(root, path)] = read(path).replace(before, '(\n "get_cached",\n Class::Read,\n),');
    invalid[resolve(root, path)] = read(path).replace(before, before + '\n make_command!(),');
  }
  assert.deepEqual(surfaces(root, multiline), surfaces(root));
  assert.throws(() => surfaces(root, invalid), /Unsupported Rust command table/);
  const event = "src-tauri/src/remote/events.rs";
  assert.throws(() => surfaces(root, { [resolve(root, event)]: read(event).replace('    "prs-updated",', '    "prs-updated",\n EVENT_FROM_CONSTANT,') }), /Unsupported Rust event table/);
});
