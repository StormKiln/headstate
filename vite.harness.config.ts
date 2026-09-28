// The browser harness build: the app's own Vite config, pointed at the
// harness pages instead of the app. `harness/transcript.html` is the
// transcript viewer's (#1487, #1480), for `make bench-transcript-browser`;
// `harness/shell.html` is the app shell's (#1583), for
// `make check-shell-scroll`. The app's bundle never includes either.
import { defineConfig, mergeConfig } from "vite";
import base from "./vite.config";

export default defineConfig((env) =>
  mergeConfig(base(env), {
    build: {
      outDir: "dist-harness",
      emptyOutDir: true,
      rollupOptions: {
        input: {
          transcript: new URL("./harness/transcript.html", import.meta.url).pathname,
          shell: new URL("./harness/shell.html", import.meta.url).pathname,
        },
      },
    },
  }),
);
