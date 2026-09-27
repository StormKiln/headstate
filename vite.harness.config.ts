// The transcript viewer's browser harness build (#1487, #1480): the app's
// own Vite config, pointed at `harness/transcript.html` instead of the
// app. Built only by `make bench-transcript-browser`; the app's bundle
// never includes the harness page.
import { defineConfig, mergeConfig } from "vite";
import base from "./vite.config";

export default defineConfig((env) =>
  mergeConfig(base(env), {
    build: {
      outDir: "dist-harness",
      emptyOutDir: true,
      rollupOptions: { input: new URL("./harness/transcript.html", import.meta.url).pathname },
    },
  }),
);
