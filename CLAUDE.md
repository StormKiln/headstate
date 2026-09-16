# Headstate

A Tauri 2 desktop app: Rust backend in `src-tauri/`, React 19 + TypeScript
frontend in `src/`. The iOS companion in `src-mobile/` renders **the same
`src/` frontend** — so a frontend change is also a mobile change.

This file carries only what is true everywhere and expensive to miss. Anything
narrower lives in that directory's own `CLAUDE.md`; anything a test can check
lives in `src-tauri/src/invariants.rs` instead of here.

## Verifying

**`make lint`, not `yarn lint`.** `yarn lint` is `eslint .` and misses knip and
clippy, which is what CI runs and what breaks PRs. Full gate in the `verify`
skill.

## Rules that have shipped as defects

**Absent is not zero.** A reading you could not take is not a reading of zero.
Rendering it as zero is the most legible possible lie, because a chart invites
the eye to read shape. Stated throughout the code (`claude/cli.rs:128`,
`claude/sessions.rs:444`); shipped as a real defect in #846.

**Partial is not nothing.** When a bounded operation runs out of budget, what it
already retrieved is an answer. Discarding it to return one error throws away
measured data and turns a slow success into a total failure. #1044: a timed-out
stats board dropped every PR it had fetched, because `tokio::time::timeout`
drops the future and everything it owns.

**Qualify, or suppress.** Only-low → qualify ("at least N", "28 of 37").
Possibly-wrong → suppress. Never print a confident number that might be wrong.

**Pending and Unknown are different states.** "Not checked yet" renders as a
skeleton; "checked, could not decide" renders as a failure. Collapsing them
caused #1042, where a column skeletoned forever because nothing moved it out of
Pending.

**Never say "we did not ask" as "they did not answer".** #1050: a stats load the
process declined to issue reported that GitHub had not answered. Both halves
were wrong, and the retry offered could not work.

## Working in this repo

**Never work directly on `main`.** Branch or make a worktree before the first
edit. ~100 sibling worktrees live here, so the main checkout is often on another
branch — and a failed `git checkout` does **not** stop a chained `reset --hard`.
Never chain them.

**`PathBuf::join`, never `format!("{}/…")`.** Windows `canonicalize` returns
verbatim `\\?\C:\` paths; this has caused six Windows-only failures.

**Finishing means CI green**, not pushed. A `MERGED` status does not prove your
commit landed — grep the content on `main`.

## Skills

- `verify` — the real gate before pushing
- `release` — tag-driven releases, and why a burned tag cannot be recovered
- `epic` — epics with linked sub-issues, and the integrity checks they need
- `guard` — adding an invariant, sabotage-proven in both directions
