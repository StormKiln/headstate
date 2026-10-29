// My Jira issues on a phone (#2047): the app's own Vite config, pointed at
// `harness/jira-mobile.html` instead of the app. Built only by
// `make jira-mobile-screenshots`, with VITE_TARGET=mobile so the page runs
// the phone's transport (`remote.ts`) exactly as the companion does. Its
// own output directory, so it never mixes with the desktop harness build
// in `dist-harness`. The app's bundle never includes the harness page.
import { defineConfig, mergeConfig } from "vite";
import base from "./vite.config";

export default defineConfig((env) =>
  mergeConfig(base(env), {
    build: {
      outDir: "dist-harness-jira",
      emptyOutDir: true,
      rollupOptions: { input: new URL("./harness/jira-mobile.html", import.meta.url).pathname },
    },
  }),
);
