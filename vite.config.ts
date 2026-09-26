// From "vitest/config", not "vite": the `test` block below is a Vitest key,
// and Vite's own `defineConfig` rejects it.
import { defineConfig } from "vitest/config";
import { loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// `new URL` + `import.meta.url` resolves paths without needing
// `@types/node` for `node:path`/`__dirname`/`process.cwd()`.
const root = new URL(".", import.meta.url).pathname;

/// Gives refractor's entity decoder its DOM-free build (#1482).
///
/// refractor, the syntax highlighter, runs ONLY in a web worker
/// (`src/lib/highlight.worker.ts`), and reaches
/// `decode-named-character-reference` through `parse-entities`. That
/// package's `browser` export decodes entities by writing them into
/// `document.createElement("i")` -- and a worker has no `document`, so
/// the first highlight threw and every block stayed plain. Vite resolves
/// with the `browser` condition for worker code too; this sends that ONE
/// importer to the package's plain build instead. micromark, which the
/// main thread's markdown uses, keeps the DOM build (smaller: no entity
/// table), because only `parse-entities` is redirected.
///
/// In BOTH plugin lists: the worker BUILD applies only `worker.plugins`,
/// while the dev server serves worker modules through the main pipeline.
/// Fails the build loudly if the plain build moves, rather than
/// shipping a worker that silently never highlights.
const workerSafeEntities = {
  name: "worker-safe-entities",
  enforce: "pre" as const,
  async resolveId(
    this: { resolve: (s: string, i?: string, o?: { skipSelf: boolean }) => Promise<{ id: string } | null> },
    source: string,
    importer: string | undefined,
  ) {
    if (source !== "decode-named-character-reference" || !importer?.includes("/parse-entities/")) {
      return null;
    }
    const dom = await this.resolve(source, importer, { skipSelf: true });
    const plain = dom?.id.replace(/index\.dom\.js$/, "index.js");
    const found = plain && plain !== dom?.id ? await this.resolve(plain, importer, { skipSelf: true }) : null;
    if (!found) throw new Error(`worker-safe-entities: no DOM-free build beside ${dom?.id ?? source}`);
    return found.id;
  },
};

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), workerSafeEntities],
  resolve: { alias: { "@": new URL("./src", import.meta.url).pathname } },
  // Which transport `src/api/transport.ts` picks. Defined here rather
  // than defaulted at the use site so a build with the variable unset
  // is the desktop build, and so the value is a compile-time constant
  // the bundler can fold. `loadEnv` sees both `.env*` files and the
  // process environment, so `VITE_TARGET=mobile yarn build` works.
  define: {
    "import.meta.env.VITE_TARGET": JSON.stringify(
      loadEnv(mode, root, "VITE_").VITE_TARGET ?? "desktop",
    ),
  },
  // Tauri expects a fixed port and fails if it is taken.
  //
  // `host` comes from TAURI_DEV_HOST, which `tauri ios dev --host` sets
  // to the machine's public network address: a phone on the same LAN
  // cannot reach `localhost`, so a device run needs the dev server
  // bound to an address it can actually route to. Unset -- every
  // desktop run, and CI -- it stays on Vite's default loopback bind
  // rather than exposing the dev server on the network by accident.
  //
  // Read through `loadEnv` rather than `process.env`, for the same
  // reason as VITE_TARGET below: the project deliberately carries no
  // `@types/node`, and `loadEnv` sees the process environment anyway.
  // The prefix is the full variable name because `loadEnv` filters by
  // prefix and this one does not start with VITE_.
  server: {
    port: 1420,
    strictPort: true,
    host: loadEnv(mode, root, "TAURI_DEV_HOST").TAURI_DEV_HOST ?? false,
  },
  build: { target: "safari15", sourcemap: true },
  // ES-module workers, not Vite's default IIFE: the syntax-highlighting
  // worker (`src/lib/highlight.worker.ts`, #1482) loads each grammar as
  // its own chunk on demand, and an IIFE bundle cannot code-split. The
  // build target already assumes Safari 15, which runs module workers.
  worker: { format: "es", plugins: () => [workerSafeEntities] },
  test: {
    // Vitest's default glob walks the whole tree, and this project keeps
    // git worktrees at `.worktrees/<branch>/`. A bare `vitest run` then
    // collected every sibling branch's tests too -- 4837 tests across 482
    // files instead of this checkout's ~500 -- and stale branches fail
    // against current code, so `make test-ui` disagreed with CI (which
    // has no worktrees) and the local signal was worthless.
    //
    // Anchoring `include` to `src/` is the whole fix; an explicit
    // `.worktrees` exclude was measured to change nothing and left out.
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    environment: "jsdom",
    // One jsdom per WORKER, not one per test file.
    //
    // The default `forks` pool builds a fresh environment for each of
    // the 141 test files, and that construction -- not the tests --
    // was the majority of the run: 152-159s of tracked time, 56%,
    // against a 27-28s wall-clock (environments overlap across
    // workers, so the total exceeds the duration). `vmThreads` runs
    // each file in its own V8 context inside a reused environment, so
    // the jsdom cost is paid once per worker instead of 141 times.
    // Measured locally: 27.9s -> 8.5s, and vitest's "jsdom was created
    // 141 times" advisory stops firing. Same 1549 tests across the
    // same 141 files, verified by diffing the full per-test JSON
    // report between the two pools rather than trusting the totals --
    // a config that quietly stopped collecting files would also look
    // like a speedup.
    //
    // `isolate: false` is the other option vitest suggests and is
    // faster still, but it shares ONE environment across files rather
    // than giving each its own context. This suite mocks modules
    // heavily (`vi.mock`, `vi.hoisted`) and several files mutate
    // module-level state, so that sharing leaks: tried, and 481 tests
    // failed, the Tauri IPC stub from one file bleeding into another's
    // `window.__TAURI_INTERNALS__`. Rejected on that evidence, not on
    // principle. `vmThreads` keeps the per-file isolation those mocks
    // depend on, which is why it is the one that works here.
    //
    // See #677. This does not replace `testTimeout` below: this makes
    // the runner faster, the timeout covers a runner that is slow
    // anyway.
    pool: "vmThreads",
    globals: true,
    // 15s, not vitest's default 5s.
    //
    // Not because any test is slow: 58% of this suite's wall-clock is
    // jsdom CONSTRUCTION -- 141 environments, one per test file, about
    // 154s of the 98s run (they overlap across workers). The default
    // budget is measured against a test whose own work is a fraction of
    // what surrounds it.
    //
    // A fully synchronous test in WorktreesPage.test.tsx -- render, then
    // assert, no await anywhere -- hit "Test timed out in 5000ms" on a
    // loaded Windows runner and blocked the v5.5.0 release, while 1548
    // of 1549 tests passed and a re-run of the same commit went green.
    // The whole 88-test file takes 2911ms when the runner is healthy:
    // inside the budget, with no margin, and the two runs of that one
    // commit differed by 1.5x on their own.
    //
    // The cost of raising it is that a genuinely hung test reports after
    // 15s instead of 5s. The cost of leaving it is a release blocked by
    // a test unrelated to the change being released. See #677, which
    // also tracks reducing the jsdom cost itself -- the actual wound,
    // of which this timeout is only the bleeding.
    testTimeout: 15_000,
    // Coverage is available ON DEMAND (`yarn vitest run --coverage`) and
    // is NOT a gate. Kept deliberately, and the reasoning matters because
    // #853 proposed removing it outright.
    //
    // The argument for removal was a stale `coverage/` directory that
    // reported 91.81% over 403 lines across 32 files -- against 130
    // non-test source files totalling 33,394 lines (measured at this
    // commit; #853 recorded 132/33,390 when it was filed), so it
    // described about 1.2% of the frontend, from a run three weeks older
    // than the commit beside it. Anyone opening it to ask "is this area covered?" got a
    // confident number about almost none of the code.
    //
    // But the artifact was the problem, not the config: `coverage` is
    // already in `.gitignore` and was never tracked, so there is nothing
    // to delete from the repo -- the misleading directory was local to one
    // machine and a fresh run overwrites it.
    //
    // `thresholds` is deliberately NOT set, and no CI job runs this.
    // A threshold over the whole frontend would have to be set at
    // today's real number to pass, which nobody has measured, and a
    // threshold is a ratchet: it fails PRs that add well-tested code
    // beside untested code. The project's actual testing rule is the one
    // the suite already enforces -- 1,199 Rust tests and a frontend suite
    // that must be green -- not a percentage. Removing the block instead
    // would mean `--coverage` errors out for someone asking a legitimate
    // one-off question ("did my new file get exercised?"), which is the
    // only use this has ever had.
    //
    // So: no implied measurement, because nothing is committed and
    // nothing is gated; and the tool still works when asked.
    coverage: { provider: "v8", reporter: ["text", "json-summary"] },
  },
}));
