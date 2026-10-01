/** #711: the old unconstrained call<T> admitted malformed network replies.
 * Derive the remote-only contract from the compiler's resolved return types,
 * not a handwritten mirror. --check is non-mutating and gates CI as well as
 * local Make targets. Unsupported shapes and uncovered surface entries fail.
 */
import ts from "typescript";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function schemaBuilder(checker) {
  const nodes = [];
  const ids = new Map();
  function ref(type) {
    if (ids.has(type)) return ids.get(type);
    const id = nodes.length;
    ids.set(type, id);
    nodes.push(null); // Reserve before descent: recursive aliases remain refs.
    const f = type.flags;
    let node;
    if (f & ts.TypeFlags.Any) throw new Error("Unsupported any in wire contract");
    else if (f & ts.TypeFlags.Unknown) node = { t: "json" };
    else if (f & ts.TypeFlags.Never) node = { t: "never" };
    else if (f & ts.TypeFlags.Void) node = { t: "void" };
    else if (f & ts.TypeFlags.Undefined) node = { t: "undefined" };
    else if (f & ts.TypeFlags.Null) node = { t: "literal", v: null };
    else if (type.isLiteral()) node = { t: "literal", v: type.value };
    else if (f & ts.TypeFlags.BooleanLiteral) node = { t: "literal", v: type.intrinsicName === "true" };
    else if (f & ts.TypeFlags.String) node = { t: "string" };
    else if (f & ts.TypeFlags.Number) node = { t: "number" };
    else if (f & ts.TypeFlags.Boolean) node = { t: "boolean" };
    else if (type.isUnion()) node = { t: "union", of: type.types.map(ref) };
    else if (checker.isTupleType(type)) {
      const flags = type.target.elementFlags;
      if (flags.some(f => f & (ts.ElementFlags.Rest | ts.ElementFlags.Variadic))) {
        throw new Error("Unsupported variadic wire tuple");
      }
      node = { t: "tuple", items: checker.getTypeArguments(type).map(ref), min: flags.filter(f => f & ts.ElementFlags.Required).length };
    } else if (checker.isArrayType(type)) {
      node = { t: "array", item: ref(checker.getTypeArguments(type)[0]) };
    } else if (f & (ts.TypeFlags.Object | ts.TypeFlags.Intersection)) {
      if (type.getCallSignatures().length || type.getConstructSignatures().length) {
        throw new Error("Unsupported callable in wire contract");
      }
      // The checker resolves object intersections and mapped Record members.
      // Non-object intersections (brands) need an explicit wire representation.
      if (type.isIntersection() && type.types.some(t => !(t.flags & ts.TypeFlags.Object))) {
        throw new Error("Unsupported non-object wire intersection");
      }
      const indexes = checker.getIndexInfosOfType(type);
      if (indexes.some(i => !(i.keyType.flags & ts.TypeFlags.String))) {
        throw new Error("Unsupported non-string wire index signature");
      }
      const props = type.getProperties().map(p => {
        const declaration = p.valueDeclaration ?? p.declarations?.[0];
        if (!declaration || p.getName().startsWith("__@")) throw new Error("Unsupported wire property");
        return [p.getName(), ref(checker.getTypeOfSymbolAtLocation(p, declaration)), !!(p.flags & ts.SymbolFlags.Optional)];
      });
      if (!props.length && !indexes.length) throw new Error("Unsupported empty object type; use an explicit JSON wire shape");
      node = { t: "object", props };
      if (indexes.length) node.index = ref(indexes[0].type);
    } else {
      throw new Error(`Unsupported wire type: ${checker.typeToString(type)}`);
    }
    nodes[id] = node;
    return id;
  }
  function finish() {
    // Precompute common required literal tags so large discriminated unions
    // dispatch once, rather than traversing every alternative's body.
    for (const n of nodes) {
      if (n.t !== "union" || n.of.length < 2) continue;
      const arms = n.of.map(i => nodes[i]);
      if (!arms.every(a => a.t === "object")) continue;
      for (const [key, child, optional] of arms[0].props) {
        if (optional || nodes[child].t !== "literal") continue;
        const tags = arms.map(a => a.props.find(p => p[0] === key && !p[2]));
        if (tags.some(p => !p || nodes[p[1]].t !== "literal")) continue;
        const values = tags.map(p => nodes[p[1]].v);
        if (new Set(values.map(v => JSON.stringify(v))).size !== values.length) continue;
        n.tag = key;
        n.cases = values.map((v, i) => [v, n.of[i]]);
        break;
      }
    }
    return nodes;
  }
  return { ref, finish, nodes };
}

function table(source, name) {
  const match = source.match(new RegExp(`pub const ${name}[^=]*=\\s*&\\[([\\s\\S]*?)\\];`));
  if (!match) throw new Error(`Missing Rust ${name} table`);
  return match[1].replace(/\/\/[^\n]*/g, "");
}
export function surfaces(root, overrides = {}) {
  const read = path => overrides[resolve(root, path)] ?? readFileSync(resolve(root, path), "utf8");
  const parseCommands = path => {
    const body = table(read(path), "SURFACE");
    const row = /\(\s*"([a-z0-9_]+)"\s*,\s*Class::(\w+)\s*,?\s*\)\s*,?/g;
    if (body.replace(row, "").trim()) throw new Error("Unsupported Rust command table syntax");
    return [...body.matchAll(row)].map(m => [m[1], m[2]]);
  };
  const desktop = parseCommands("src-tauri/src/remote/surface.rs");
  const mobile = parseCommands("src-mobile/src/surface.rs");
  if (JSON.stringify(desktop) !== JSON.stringify(mobile)) throw new Error("Remote class tables differ");
  if (!desktop.length || new Set(desktop.map(x => x[0])).size !== desktop.length) throw new Error("Empty/duplicate remote commands");
  if (desktop.some(([, c]) => !["Read", "Write", "Destructive", "Local"].includes(c))) throw new Error("Unknown remote command class");
  const parseEvents = path => {
    const body = table(read(path), "EVENT_NAMES");
    const row = /"([a-z0-9-]+)"\s*,?/g;
    if (body.replace(row, "").trim()) throw new Error("Unsupported Rust event table syntax");
    return [...body.matchAll(row)].map(m => m[1]);
  };
  const events = parseEvents("src-tauri/src/remote/events.rs");
  if (!events.length || new Set(events).size !== events.length || JSON.stringify(events) !== JSON.stringify(parseEvents("src-mobile/src/events.rs"))) {
    throw new Error("Empty, duplicate or mismatched remote events");
  }
  return { commands: desktop.filter(([, c]) => c !== "Local").map(([n]) => n).sort(), events: events.sort() };
}

export function generate(root = process.cwd(), overrides = {}) {
  const configPath = resolve(root, "tsconfig.json");
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  if (config.error) throw new Error("Cannot read TypeScript config");
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, root);
  const host = ts.createCompilerHost(parsed.options);
  const originalRead = host.readFile.bind(host);
  host.readFile = path => overrides[resolve(path)] ?? originalRead(path);
  if (parsed.errors.length) throw new Error("Invalid TypeScript configuration");
  const apiPath = resolve(root, "src/api/tauri.ts");
  const mapPath = resolve(root, "src/api/wireTypes.ts");
  const program = ts.createProgram([apiPath, mapPath], parsed.options, host);
  const checker = program.getTypeChecker();
  const builder = schemaBuilder(checker);
  const wanted = surfaces(root, overrides);
  const remote = program.getSourceFile(resolve(root, "src/api/remote.ts"));
  let localNames;
  function findLocal(n) {
    if (ts.isVariableDeclaration(n) && n.name.getText() === "CLIENT_COMMANDS") {
      const array = n.initializer?.arguments?.[0];
      if (!array || !ts.isArrayLiteralExpression(array) || array.elements.some(e => !ts.isStringLiteral(e))) {
        throw new Error("Unsupported CLIENT_COMMANDS declaration");
      }
      localNames = array.elements.map(e => e.text);
    }
    ts.forEachChild(n, findLocal);
  }
  findLocal(remote);
  if (!localNames?.length || localNames.some(n => wanted.commands.includes(n))) {
    throw new Error("Missing client-local registry or command classified both remote and client-local");
  }
  const commands = Object.create(null);
  const events = Object.create(null);
  const types = new Map();
  const origins = new Map();
  function add(name, type) {
    if (!types.has(name)) types.set(name, []);
    const list = types.get(name);
    if (!list.includes(type)) list.push(type);
  }
  function visit(n) {
    if (ts.isCallExpression(n) && ts.isIdentifier(n.expression) && n.expression.text === "call") {
      if (!n.typeArguments || n.typeArguments.length !== 1 || !n.arguments.length) throw new Error("Untyped transport call");
      const names = arg => {
        if (ts.isStringLiteral(arg)) return [arg.text];
        if (ts.isConditionalExpression(arg)) return [...names(arg.whenTrue), ...names(arg.whenFalse)];
        throw new Error("Unsupported dynamic transport command");
      };
      for (const name of names(n.arguments[0])) {
        if (wanted.commands.includes(name)) {
          add(name, checker.getTypeFromTypeNode(n.typeArguments[0]));
          origins.set(name, `${apiPath}:${n.getSourceFile().getLineAndCharacterOfPosition(n.getStart()).line + 1}`);
        }
      }
    }
    ts.forEachChild(n, visit);
  }
  visit(program.getSourceFile(apiPath));
  const map = program.getSourceFile(mapPath);
  for (const declaration of map.statements) {
    if (!ts.isInterfaceDeclaration(declaration)) continue;
    const kind = declaration.name.text;
    if (kind !== "RemoteEvents" && kind !== "RemoteOnlyReplies") continue;
    for (const member of checker.getTypeAtLocation(declaration).getProperties()) {
      const name = member.getName();
      const type = checker.getTypeOfSymbolAtLocation(member, member.valueDeclaration);
      if (kind === "RemoteOnlyReplies") {
        if (!wanted.commands.includes(name)) throw new Error(`Non-remote explicit reply ${name}`);
        add(name, type);
      } else {
        if (!wanted.events.includes(name)) throw new Error(`Non-remote event ${name}`);
        events[name] = builder.ref(type);
      }
    }
  }
  for (const name of wanted.commands) {
    const alternatives = types.get(name);
    if (!alternatives?.length) throw new Error(`Uncovered remote command ${name}`);
    let refs;
    try { refs = [...new Set(alternatives.map(builder.ref))]; }
    catch (e) { throw new Error(`${origins.get(name) ?? mapPath}: ${name}: ${e.message}`); }
    if (refs.length === 1) commands[name] = refs[0];
    else {
      commands[name] = builder.nodes.length;
      builder.nodes.push({ t: "union", of: refs });
    }
  }
  for (const name of wanted.events) if (!(name in events)) throw new Error(`Uncovered remote event ${name}`);
  const graph = { commands, events, nodes: builder.finish() };
  const json = `{\n"commands": ${JSON.stringify(graph.commands, null, 2)},\n"events": ${JSON.stringify(graph.events, null, 2)},\n"nodes": [\n${graph.nodes.map(n => `  ${JSON.stringify(n)}`).join(",\n")}\n]\n}`;
  return `// Generated by scripts/wire-contract.mjs. Do not edit; run make wire-contract.\nimport type { WireGraph } from "./wireValidator";\n\nexport const wireGraph: WireGraph = ${json};\n`;
}

export function checkGenerated(expected, actual) {
  if (expected !== actual) throw new Error("Remote wire contract is stale; run make wire-contract and commit the result.");
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const output = resolve("src/api/wireContract.generated.ts");
    const generated = generate();
    if (process.argv.includes("--check")) checkGenerated(generated, readFileSync(output, "utf8"));
    else writeFileSync(output, generated);
    console.log(`Remote wire contract ${process.argv.includes("--check") ? "checked" : "generated"}.`);
  } catch (e) {
    console.error(e.message);
    process.exitCode = 1;
  }
}
