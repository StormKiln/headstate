import { describe, expect, it } from "vitest";
// `?raw`, not `node:fs`, for the reason `mirroredConstants.test.ts` gives:
// the project carries no `@types/node`.
import modelRs from "../../src-tauri/src/measurement/model.rs?raw";
import reportPy from "../../scripts/measurement-report.py?raw";
import measurementTs from "../types/measurement.ts?raw";

/// The mounted-review footer is one closed set written three times: the
/// Rust `Footer` enum, the TypeScript `ClientMeasurement` mirror and the
/// offline report's allowed tokens. A value missing from any one of them
/// is a record the recorder or the report rejects as malformed, so the
/// three are read here and compared rather than restated (#1996).

const snake = (name: string) => name.replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase();

function rustFooters(): string[] {
  const body = /closed!\(Footer \{([^}]*)\}\);/.exec(modelRs)?.[1];
  if (body === undefined) throw new Error("closed!(Footer { … }) not found in model.rs");
  return body
    .split("\n")
    .map((line) => line.replace(/\/\/.*$/, "").trim())
    .flatMap((line) => line.split(","))
    .map((v) => v.trim())
    .filter(Boolean)
    .map(snake);
}

function tsFooters(): string[] {
  const union = /\bfooter: ([^;]*);/.exec(measurementTs)?.[1];
  if (union === undefined) throw new Error("footer union not found in measurement.ts");
  return [...union.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
}

function pyFooters(): string[] {
  const tokens = /'footer':'([a-z_ ]+)'/.exec(reportPy)?.[1];
  if (tokens === undefined) throw new Error("'footer' tokens not found in measurement-report.py");
  return tokens.split(" ").filter(Boolean);
}

describe("mounted-review footer values agree across languages", () => {
  it("Rust, TypeScript and the report list the same footers", () => {
    const rust = rustFooters();
    expect(rust.length).toBeGreaterThan(10);
    expect([...tsFooters()].sort()).toEqual([...rust].sort());
    expect([...pyFooters()].sort()).toEqual([...rust].sort());
  });

  it("carries not_asked as its own value beside not_checked (#1996)", () => {
    for (const values of [rustFooters(), tsFooters(), pyFooters()]) {
      expect(values).toContain("not_asked");
      expect(values).toContain("not_checked");
    }
  });
});
