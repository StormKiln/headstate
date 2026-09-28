// The Worktrees page's browser harness build (#1582): the app's own Vite
// config, pointed at `harness/worktrees.html` instead of the app. Built
// only by `make bench-worktrees-browser`; the app's bundle never includes
// the harness page.
//
// Two differences from the app build, both for the measurement:
// - `react-dom/client` resolves to React's PROFILING build, the only
//   production build that calls a `<Profiler>`'s `onRender`, which is how
//   the harness times each commit;
// - nothing is minified, so a CPU profile names the page's own functions
//   (`Row`, `sortWorktrees`, `prForWorktree`) rather than one-letter ones.
import { defineConfig, mergeConfig } from "vite";
import base from "./vite.config";

export default defineConfig((env) =>
  mergeConfig(base(env), {
    resolve: { alias: [{ find: /^react-dom\/client$/, replacement: "react-dom/profiling" }] },
    build: {
      outDir: "dist-harness-worktrees",
      emptyOutDir: true,
      minify: false,
      rollupOptions: { input: new URL("./harness/worktrees.html", import.meta.url).pathname },
    },
  }),
);
