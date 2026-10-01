/// Compact compiler-derived JSON schema interpreter. It inspects decoded
/// network values without cloning, coercing, stripping additive fields or
/// exposing any payload values/record keys in diagnostics.
export type WireNode =
  | { t: "string" | "number" | "boolean" | "undefined" | "void" | "never" | "json" }
  | { t: "literal"; v: string | number | boolean | null }
  | { t: "array"; item: number }
  | { t: "tuple"; items: number[]; min: number }
  | { t: "object"; props: [string, number, boolean][]; index?: number }
  | { t: "union"; of: number[]; tag?: string; cases?: [string | number | boolean | null, number][] };
export interface WireGraph {
  commands: Record<string, number>;
  events: Record<string, number>;
  nodes: WireNode[];
}

/// Shared across alternatives: an ambiguous union cannot reset its budget.
/// 10M operations leaves headroom for the large synthetic PR/worktree/transcript
/// cases in wireValidator.test.ts. Depth is nesting, not array length. Network
/// JSON cannot contain cycles; injected cyclic known fields fail explicitly.
const MAX_OPERATIONS = 10_000_000;
const MAX_DEPTH = 128;
const own = (v: object, key: string) => Object.prototype.hasOwnProperty.call(v, key);
const object = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

export interface Validation {
  field: string | null;
  operations: number;
}

export function validateWire(nodes: WireNode[], root: number, value: unknown): Validation {
  let operations = 0;
  let tooComplex = false;
  // In-progress recursion is never treated as completed validation.
  const active = new WeakMap<object, Set<number>>();
  const passed = new WeakMap<object, Set<number>>();
  const failed = new WeakMap<object, Map<number, string>>();
  const jsonActive = new WeakSet<object>();
  function at(path: string, key: string) {
    // key is compiler-owned, never a dictionary key or a payload tag.
    return path.length < 120 ? `${path}.${key}` : path;
  }
  function json(v: unknown, path: string, depth: number): string | null {
    if (++operations > MAX_OPERATIONS || depth > MAX_DEPTH) {
      tooComplex = true;
      return "$ (response too complex)";
    }
    if (v === null || typeof v === "string" || typeof v === "boolean") return null;
    if (typeof v === "number") return Number.isFinite(v) ? null : path;
    if (typeof v !== "object" || jsonActive.has(v)) return path;
    jsonActive.add(v);
    let failure: string | null = null;
    // Do not materialize a second copy of a large array/dictionary.
    for (const key in v) {
      if (!own(v, key)) continue;
      failure = json((v as Record<string, unknown>)[key], at(path, "[entry]"), depth + 1);
      if (failure) break;
    }
    jsonActive.delete(v);
    return failure;
  }
  function visit(id: number, v: unknown, path: string, depth: number): string | null {
    if (++operations > MAX_OPERATIONS || depth > MAX_DEPTH) {
      tooComplex = true;
      return "$ (response too complex)";
    }
    const node = nodes[id];
    if (!node) return "$ (unknown schema)";
    const isObject = v !== null && typeof v === "object";
    if (isObject) {
      if (passed.get(v)?.has(id)) return null;
      const prior = failed.get(v)?.get(id);
      if (prior !== undefined) return `${path}${prior}`;
      if (active.get(v)?.has(id)) return path;
      let pending = active.get(v);
      if (!pending) { pending = new Set(); active.set(v, pending); }
      pending.add(id);
    }
    let failure: string | null = null;
    switch (node.t) {
      case "string": case "boolean": failure = typeof v === node.t ? null : path; break;
      case "number": failure = typeof v === "number" && Number.isFinite(v) ? null : path; break;
      case "literal": failure = v === node.v ? null : path; break;
      case "undefined": failure = v === undefined ? null : path; break;
      // Rust serializes () as null. Preserve the existing undefined IPC stub
      // convention too; no other value is an acknowledgement.
      case "void": failure = v == null ? null : path; break;
      case "never": failure = path; break;
      case "json": failure = json(v, path, depth); break;
      case "array":
        if (!Array.isArray(v)) { failure = path; break; }
        for (let i = 0; i < v.length; i++) {
          failure = visit(node.item, v[i], `${path}[]`, depth + 1);
          if (failure) break;
        }
        break;
      case "tuple":
        if (!Array.isArray(v) || v.length < node.min || v.length > node.items.length) { failure = path; break; }
        for (let i = 0; i < v.length; i++) {
          failure = visit(node.items[i], v[i], `${path}[${i}]`, depth + 1);
          if (failure) break;
        }
        break;
      case "object":
        if (!object(v)) { failure = path; break; }
        for (const [key, child, optional] of node.props) {
          if (!own(v, key)) {
            if (optional) continue;
            failure = at(path, key); break;
          }
          failure = visit(child, v[key], at(path, key), depth + 1);
          if (failure) break;
        }
        if (!failure && node.index !== undefined) {
          for (const key in v) {
            if (!own(v, key)) continue;
            failure = visit(node.index, v[key], at(path, "[entry]"), depth + 1);
            if (failure) break;
          }
        }
        break;
      case "union": {
        if (node.tag && node.cases && object(v)) {
          const selected = own(v, node.tag) ? node.cases.find(([tag]) => v[node.tag!] === tag) : undefined;
          failure = selected ? visit(selected[1], v, path, depth + 1) : at(path, node.tag);
          break;
        }
        failure = path;
        for (const child of node.of) {
          const result = visit(child, v, path, depth + 1);
          if (result === null) { failure = null; break; }
          // Keep the most specific safe schema field, never a payload value.
          if (result.length > failure.length) failure = result;
          if (tooComplex) { failure = "$ (response too complex)"; break; }
        }
        break;
      }
      default: failure = "$ (unknown schema)";
    }
    if (isObject) {
      active.get(v)?.delete(id);
      if (failure === null) {
        let done = passed.get(v);
        if (!done) { done = new Set(); passed.set(v, done); }
        done.add(id);
      } else if (!tooComplex && failure.startsWith(path)) {
        // Relative schema-only paths permit reuse for aliases without storing
        // payload keys. Failed union subtrees must be cached too: otherwise
        // a recursive ambiguous union can revisit the same subtree 2^depth.
        let rejected = failed.get(v);
        if (!rejected) { rejected = new Map(); failed.set(v, rejected); }
        rejected.set(id, failure.slice(path.length));
      }
    }
    return failure;
  }
  const field = visit(root, value, "$", 0);
  return { field: tooComplex ? "$ (response too complex)" : field, operations };
}
