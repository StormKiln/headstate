import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";

export default tseslint.config(
  // Both crates' build directories: an iOS build leaves tauri-codegen's
  // compressed `.js` assets under src-mobile/target, which eslint
  // otherwise tries to parse.
  //
  // `.worktrees` is the same hazard `vite.config.ts` documents for tests:
  // this project keeps git worktrees at `.worktrees/<branch>/`, each a
  // complete copy of `src/`, and with `projectService` enabled eslint
  // builds a full TypeScript program PER sibling checkout. Memory then
  // scales with how many branches happen to be open rather than with the
  // code being linted -- measured here at 1264 files and 3.79 GB peak RSS
  // against a 4 GB heap with three siblings present, versus 316 files and
  // 2.16 GB with this entry. The warnings were wrong too: 60 reported,
  // only 15 of them about this checkout.
  //
  // CI is unaffected either way -- it checks out fresh and has no
  // `.worktrees` -- which is why the failure survived: the one place that
  // would have reported it could not see it. #983
  {
    ignores: [
      "dist",
      "src-tauri/target",
      "src-mobile/target",
      "src-mobile/gen/apple/build",
      "coverage",
      ".remember",
      ".worktrees",
    ],
  },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      // Type information, for the two rules below and nothing else.
      // `projectService` is what lets typescript-eslint ask the compiler
      // whether an expression is a Promise -- a question no amount of
      // syntax can answer, which is why these two rules need it and the
      // rest of this config did not.
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    plugins: { "react-hooks": reactHooks, "react-refresh": reactRefresh },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      // TWO type-aware rules, deliberately not the strict set. #892
      // measured `strictTypeChecked` at 1040 findings here, 73% of them
      // `no-confusing-void-expression` and `restrict-template-expressions`
      // -- stylistic volume, and #853 is this repo's standing evidence
      // that a gate crying wolf gets switched off. These two measured 18
      // and 2, every one of them a real async-correctness gap, and all 20
      // are fixed in the commit that turns them on.
      //
      // What they buy: a rejected promise with no handler is an unhandled
      // rejection, which in this app means a silent dead control (the
      // `dockerRunningContainers` click, which never opened its dialog)
      // or a console full of noise during the debugging sessions where
      // the console matters -- the same failure `safeUnlisten` already
      // exists to stop on the teardown side.
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": "error",
    },
  },
);
