import { describe, expect, it } from "vitest";
import pkg from "../../package.json";

/// `cn` comes from `@/lib/utils`, never from the npm package `cn` (#1558).
///
/// # What happened
///
/// `yarn shadcn add tabs` wrote `import { cn } from "cn"` and added `cn` to
/// `package.json` and `yarn.lock`. Our config was not the cause:
/// `components.json` maps `utils` to `@/lib/utils`, and the CLI (4.21.0)
/// rewrites every registry path it knows onto our aliases. The shadcn
/// REGISTRY changed under it. Every `base-nova` item, and the other styles
/// sampled, now ships `import { cn } from "cn"` plus a `cn` dependency.
/// That is shadcn's own drop-in replacement for clsx + tailwind-merge. A
/// bare package name is not an alias, so the CLI copies it through and
/// installs the package.
///
/// It also COMPILES, which is why a test has to catch it. The shadcn CLI
/// itself depends on `cn@^0.2.4`, and yarn hoists it to `node_modules/cn`,
/// so the stray import resolves even with no direct dependency.
///
/// No `components.json` setting prevents this. So the guard is here: an
/// unreviewed dependency must not arrive as a side effect of adding a
/// component (the supply-chain policy), and a second `cn` must not
/// silently replace the one in `src/lib/utils.ts`. `make shadcn-add`
/// undoes both.
///
/// # Why `import.meta.glob`
///
/// It works the same way `emptyStateGuard.test.ts` does, and for the same
/// reason: a glob covers the next component somebody adds, and a list of
/// files would not. This file is skipped BY PATH, because its own prose
/// and fixtures contain the pattern.
const sources = import.meta.glob(["../**/*.ts", "../**/*.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const SELF = "./cnImport.test.ts";

/// A `from "cn"` / `from 'cn'` specifier, a bare or dynamic `import "cn"`,
/// or `require("cn")`. `cn/...` subpaths count too. Anchored on the quote
/// so `@/lib/cn`, `clsx` or `cnx` never match.
const CN_IMPORT = /(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)["']cn(?:\/[^"']*)?["']/;

function cnImportOffenders(files: Record<string, string>): string[] {
  const out: string[] = [];
  for (const [path, text] of Object.entries(files)) {
    if (path === SELF) continue;
    text
      .replace(/\r\n/g, "\n")
      .split("\n")
      .forEach((line, i) => {
        const code = line.trimStart();
        if (code.startsWith("//") || code.startsWith("*")) return;
        if (CN_IMPORT.test(line)) out.push(`${path}:${i + 1}: ${line.trim()}`);
      });
  }
  return out.sort();
}

type Manifest = Partial<
  Record<
    "dependencies" | "devDependencies" | "peerDependencies" | "optionalDependencies",
    Record<string, string>
  >
>;

function cnDependencyOffenders(manifest: Manifest): string[] {
  const out: string[] = [];
  for (const field of [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
  ] as const) {
    const deps = manifest[field];
    if (deps && Object.hasOwn(deps, "cn")) out.push(`package.json ${field}.cn`);
  }
  return out;
}

const FIX =
  'import `cn` from "@/lib/utils" and `yarn remove cn`. Add shadcn ' +
  "components with `make shadcn-add C=<name>`, which does both (#1558).";

describe("cn comes from @/lib/utils, not the npm package (#1558)", () => {
  it("no source file imports the `cn` package", () => {
    expect(cnImportOffenders(sources), FIX).toEqual([]);
  });

  it("package.json does not depend on the `cn` package", () => {
    expect(cnDependencyOffenders(pkg as Manifest), FIX).toEqual([]);
  });

  // The floor: a glob that matched nothing would pass vacuously. The
  // shadcn components and utils.ts itself must be in view.
  it("the scan sees the shadcn components and lib/utils", () => {
    expect(Object.keys(sources)).toContain("../components/ui/tabs.tsx");
    expect(Object.keys(sources)).toContain("./utils.ts");
    expect(sources["../components/ui/tabs.tsx"]).toMatch(/from "@\/lib\/utils"/);
  });

  // Both directions: the shapes the registry writes are caught...
  it("catches every spelling the CLI or a hand edit produces", () => {
    for (const line of [
      'import { cn } from "cn"',
      "import { cn } from 'cn'",
      'import { cn as merge } from "cn";',
      'export { cn } from "cn"',
      'import "cn"',
      'const m = await import("cn")',
      'const { cn } = require("cn")',
      'import { cn } from "cn/merge"',
    ]) {
      expect(cnImportOffenders({ "x.ts": line }), line).toHaveLength(1);
    }
    expect(cnImportOffenders({ "x.ts": 'import { cn } from "cn"\r\n' })).toHaveLength(1);
    expect(cnDependencyOffenders({ dependencies: { cn: "^0.3.0" } })).toHaveLength(1);
    expect(cnDependencyOffenders({ devDependencies: { cn: "^0.3.0" } })).toHaveLength(1);
  });

  // ...and the safe ones are not.
  it("stays silent on the safe shapes", () => {
    for (const line of [
      'import { cn } from "@/lib/utils"',
      'import { cn } from "./utils"',
      'import { clsx } from "clsx"',
      'import { cnx } from "cnx"',
      'import x from "@acme/cn"',
      '// shadcn writes import { cn } from "cn"; rewrite it',
      ' * e.g. import { cn } from "cn"',
    ]) {
      expect(cnImportOffenders({ "x.ts": line }), line).toEqual([]);
    }
    expect(cnDependencyOffenders({ dependencies: { clsx: "^2" } })).toEqual([]);
    expect(cnDependencyOffenders({})).toEqual([]);
  });
});
