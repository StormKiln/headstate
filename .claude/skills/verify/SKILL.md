---
name: verify
description: Use before pushing, opening a PR, or claiming work is done in the Headstate repo - runs the real gate (make lint, not yarn lint) and reports counts rather than adjectives
---

# Verify

## `make lint`, not `yarn lint`

`yarn lint` is `eslint .` and nothing else. **knip and clippy run only under
`make lint`**, which is what CI's lint job invokes.

Measured on a clean tree, with one unimported export added:

```
yarn lint  -> exit 0
make lint  -> exit 2
yarn knip  -> Unused exports (1)  knipProbeUnusedExport  src/lib/stats.ts
```

Same tree, opposite verdicts. This gap broke several PRs in the 5.19/5.20
cycle: the local check passed, CI failed minutes later.

## The gate

Run all of these. Report the numbers.

```bash
cd src-tauri && cargo test --lib     # baseline moves; say what it moved to
cd .. && yarn vitest run
make lint                            # knip + clippy + eslint + privacy
make test-mobile                     # when src-mobile/ or the shared frontend changed
```

`src/` is rendered by BOTH the desktop app and the iOS companion, so a frontend
change is a mobile change.

## Before `make lint` will run at all

- **A fresh worktree needs `yarn install --immutable`** or `lint-ui` dies with
  "Couldn't find the node_modules state file". Hit twice in one cycle.
- **`cargo fmt`** first. `lint-rust` fails on formatting, which is the cheapest
  possible CI failure to have avoided.

## Report counts, not adjectives

"1659 passed, 0 failed (baseline 1658)" is information. "Tests pass" is not —
it hides a suite that silently stopped running a file, and it cannot be checked
by the person reading it.

If a gate fails, say which one and paste the failing lines. If you skipped a
gate, say that too.

## `check-cache-budget.py` reads a shared, draining resource

`lint-deps` measures the Actions cache against its budget, and that figure
changes between runs without anyone touching the repo. A failure there is not
a permanent property of the branch — re-read it before concluding anything, and
never tell someone "that one always fails".

## Finishing means CI green

Pushing is not finishing. A merge queue can eject a PR silently, and a `MERGED`
status does not prove your commit landed — verify by grepping the content on
`main`.

Before merging, check **every** check-run attempt, not just the latest:

```bash
gh api "repos/OWNER/REPO/commits/SHA/check-runs?per_page=100&filter=all" \
  -q '[.check_runs[]|.conclusion]|group_by(.)|map({(.[0]//"null"):length})|add'
```

See the `release` skill for why a single failed attempt is permanent.
