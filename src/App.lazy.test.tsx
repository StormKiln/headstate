// `?raw` rather than `node:fs`: this project deliberately carries no
// `@types/node` (`vite.config.ts` says so and resolves its own paths with
// `import.meta.url` for the same reason), so `readFileSync` does not
// typecheck here. Vite's raw import gives the file's text with no types
// needed, and it is the same text the bundler sees.
import appSource from "./App.tsx?raw";
import { describe, expect, it } from "vitest";

/// The chart-carrying routes stay OFF the launch chunk (#838).
///
/// # Why this is a source test and not a bundle test
///
/// The property that matters is a build output: `recharts` must not be in
/// the chunk `index.html` loads. But `dist/` is a build artifact that a
/// `vitest run` does not produce -- a test that read it would pass
/// vacuously on a clean checkout and fail confusingly on a stale one, which
/// is worse than no test. The source is what the build is a function of, so
/// this asserts the source property that produces it: the two
/// recharts-reachable routes are `lazy`, and nothing imports them eagerly.
///
/// The measured consequence of getting this wrong, from #838's own
/// measurements on this tree: the launch chunk goes from 945,919 bytes back
/// to 1,378,820, and the median time to React's first commit from ~25ms
/// back to ~33ms (21 interleaved loads per side, headless Chrome, cache
/// disabled). Small in absolute terms and the whole point of the app: a
/// tray app's value proposition is a fast badge.
///
/// # Why the TEXT and not the module
///
/// The question is about the shape of the import graph, which is exactly
/// what importing `App` would erase -- a `lazy()` call and a static import
/// both hand you a component. Reading the source is the only way to see the
/// difference from inside a test.
/// Every route that must stay off the launch chunk, and the specifier it
/// is loaded by.
///
/// Hoisted out of the `it.each` so the Suspense-count assertion below can
/// be derived from its LENGTH rather than from a literal. A hard-coded
/// count keeps passing when a route is added without a boundary, which is
/// a crash on first navigation to that view.
const LAZY_ROUTES: [name: string, specifier: string][] = [
  // This route owns both providers and imports the charted GitHub page.
  ["ProviderStatsPage", "./components/ProviderStatsPage"],
  ["SystemHealthPage", "./components/SystemHealthPage"],
  // #921. The entry that this guard's own design makes easy to forget: it
  // reads `App.tsx?raw` and checks SOURCE SHAPE, so a new charting route
  // absent from this table is not merely untested -- it passes, while the
  // launch chunk silently grows by the charting library. There is no
  // bundle-size gate behind this one (`vite.config.ts` has no
  // `manualChunks`, by documented choice), so this list IS the gate.
  //
  // `ClaudeOverviewPage` reaches `recharts` through `stats/SessionsChart`
  // -> `ui/chart`, which is the same path `StatsPage` reaches it by.
  ["ClaudeOverviewPage", "./components/ClaudeOverviewPage"],
  // #1075. Reaches `recharts` through `stats/PluginCallsChart` ->
  // `ui/chart`, the same path as the three above.
  ["ClaudePluginsPage", "./components/ClaudePluginsPage"],
];

describe("the launch chunk", () => {
  const app = appSource;

  /// Both routes must come through `lazy(() => import(...))`.
  ///
  /// Named per route rather than asserted as a count, so a regression says
  /// WHICH page came back onto the launch path.
  it.each(LAZY_ROUTES)("loads %s as its own chunk", (name, path) => {
    // The `lazy` declaration, with the dynamic import inside it. Matched
    // together rather than separately: a `lazy` over a statically-imported
    // component compiles and renders perfectly, and splits nothing.
    const declaration = new RegExp(
      `const ${name}\\s*=\\s*lazy\\(\\s*\\(\\)\\s*=>\\s*[\\s\\S]{0,120}?import\\("${path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}"\\)`,
    );
    expect(
      declaration.test(app),
      `${name} must be declared as lazy(() => import("${path}")); a static ` +
        `import puts recharts back on the launch path`,
    ).toBe(true);

    // And there must be no STATIC import of it anywhere in the file. A
    // leftover eager import alongside the lazy one would satisfy the check
    // above and still pull the module into the launch chunk, because the
    // bundler follows the static edge.
    expect(
      new RegExp(`^import\\s*\\{[^}]*\\b${name}\\b[^}]*\\}\\s*from`, "m").test(app),
      `${name} must not also be imported statically -- the static edge wins ` +
        `and the lazy() boundary then splits nothing`,
    ).toBe(false);
  });

  /// Each lazy route needs a Suspense boundary, or React throws at render.
  ///
  /// Asserted because the failure is not a compile error: a `lazy` component
  /// with no boundary above it throws only when that route is first
  /// navigated to, which is a crash in the one place nothing else covers.
  it("wraps each lazy route in Suspense", () => {
    const boundaries = app.match(/<Suspense\b/g) ?? [];
    // Counted against the TABLE rather than a literal, so adding a route
    // above raises this floor automatically. The literal `2` that stood
    // here was correct and inert: it would have kept passing with a third
    // lazy route added and no boundary for it, which is a crash on first
    // navigation and nothing else covers it.
    expect(
      boundaries.length,
      "each lazy route needs its own Suspense boundary; a lazy component " +
        "with none above it throws when that view is first opened",
    ).toBeGreaterThanOrEqual(LAZY_ROUTES.length);
  });

  /// `recharts` must not be reachable from `App.tsx` except through the
  /// lazy boundaries.
  ///
  /// The direct statement of what #838 is about. `App.tsx` is the launch
  /// chunk's root, so an import of `recharts` -- or of `ui/chart`, which is
  /// only a wrapper over it -- anywhere in this file defeats the split
  /// regardless of how the two routes are loaded.
  it("never reaches a charting library from the shell", () => {
    expect(app).not.toMatch(/^import\s*\{[^}]*\b(?:StatsPage|GitLabStatsPage)\b[^}]*\}\s*from/m);
    expect(app).not.toMatch(/from\s*"recharts"/);
    expect(app).not.toMatch(/from\s*"[^"]*ui\/chart"/);
  });
});
